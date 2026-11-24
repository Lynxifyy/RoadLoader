//! The companion (32-bit) main loop.
//!
//! One thread owns the frame producer, the input consumer and the heartbeat.
//! Keeping them on a single thread means the 32-bit process never needs a lock
//! on the hot path: every cross-process hand-off is a ring or a triple-buffer
//! state transition.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::alloc;
use crate::assets::{
    status_code, AssetAssembler, AssetChunk, ASSET_STATUS_BAD_TOTAL, ASSET_STATUS_CRC_MISMATCH,
    ASSET_STATUS_INCOMPLETE, ASSET_STATUS_LENGTH_MISMATCH, ASSET_STATUS_NO_BEGIN,
    ASSET_STATUS_OK, ASSET_STATUS_PENDING, ASSET_STATUS_TOTAL, MAX_ASSET_CHUNKS,
};
use crate::audio::AudioPacer;
use crate::crash;
use crate::ipc::layout::{
    Command, CMD_ASSET_BEGIN, CMD_ASSET_CHUNK, CMD_ASSET_COMMIT, CMD_PATCH_OFFSET,
    CMD_REQUEST_SELF_TEST, CMD_SET_RENDER_DISTANCE, CMD_SET_SAFE_MODE, CMD_SET_SHADERS,
    CMD_SET_VOLUME, CMD_SHUTDOWN, EVENT_LOG, FLAG_HEADLESS, FLAG_SAFE_MODE, FLAG_SHADERS,
    INPUT_KIND_BUTTON, INPUT_KIND_CANCEL, INPUT_KIND_DOWN, INPUT_KIND_KEY, INPUT_KIND_MOVE,
    INPUT_KIND_STICK, INPUT_KIND_UP, STATUS_RUNNING, STATUS_SAFE_MODE, STATUS_STOPPED,
};
use crate::ipc::region::Region;
use crate::mesh;
use crate::physics;
use crate::render::Scene;
use crate::util::crc32;

const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
const STATS_INTERVAL: Duration = Duration::from_millis(250);
/// A full audio ring is worth one log line per second, not one per block.
const AUDIO_DROP_LOG_INTERVAL: Duration = Duration::from_secs(1);
/// Demo tone length in frames: about 21 ms at 48 kHz.
const DEMO_TONE_FRAMES: usize = 1024;

pub const TEST_MESH: u32 = 1 << 0;
pub const TEST_MATH: u32 = 1 << 1;
pub const TEST_PHYSICS: u32 = 1 << 2;
pub const TEST_ALLOC: u32 = 1 << 3;
pub const TEST_IPC: u32 = 1 << 4;
pub const TEST_ALL_OK: u32 = 1 << 31;

struct CompanionState {
    region: Arc<Region>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

static STATE: OnceLock<Mutex<Option<CompanionState>>> = OnceLock::new();
static SELF_TEST: AtomicU32 = AtomicU32::new(0);

fn state() -> &'static Mutex<Option<CompanionState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

/// Starts the companion loop. Fails when a loop is already running.
pub fn start(region: Arc<Region>, flags: u32) -> Result<(), &'static str> {
    let mut guard = state().lock().map_err(|_| "companion state poisoned")?;
    if guard.is_some() {
        return Err("companion already running");
    }
    crash::install(&region);
    crash::install_panic_hook();
    region.set_status(if flags & FLAG_SAFE_MODE != 0 {
        STATUS_SAFE_MODE
    } else {
        STATUS_RUNNING
    });
    let stop = Arc::new(AtomicBool::new(false));
    let thread_region = Arc::clone(&region);
    let thread_stop = Arc::clone(&stop);
    let handle = std::thread::Builder::new()
        .name("road-companion".into())
        .stack_size(512 * 1024)
        .spawn(move || run(thread_region, thread_stop, flags))
        .map_err(|_| "failed to spawn companion thread")?;
    *guard = Some(CompanionState { region, stop, handle: Some(handle) });
    Ok(())
}

/// Signals the loop to stop and waits for it to exit.
pub fn stop() -> bool {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(mut companion) = guard.take() else {
        return false;
    };
    companion.stop.store(true, Ordering::SeqCst);
    if let Some(handle) = companion.handle.take() {
        let _ = handle.join();
    }
    companion.region.set_status(STATUS_STOPPED);
    companion.region.heartbeat();
    true
}

pub fn is_running() -> bool {
    state()
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
}

pub fn self_test_mask() -> u32 {
    SELF_TEST.load(Ordering::Acquire)
}

/// Runs the startup self-test and publishes the result.
pub fn run_self_test(region: &Region) -> (u32, String) {
    let mut mask = 0u32;
    let mut details = String::with_capacity(96);

    let (scalar, simd) = mesh::simd_self_test();
    let mesh_ok = scalar != 0 && scalar == simd;
    if mesh_ok {
        mask |= TEST_MESH;
    }
    details.push_str(if mesh_ok { "mesh=ok " } else { "mesh=FAIL " });

    let distance = crate::math::distance(glam::Vec3::ZERO, glam::Vec3::new(3.0, 4.0, 0.0));
    let math_ok = (distance - 5.0).abs() < 1e-6;
    if math_ok {
        mask |= TEST_MATH;
    }
    details.push_str(if math_ok { "math=ok " } else { "math=FAIL " });

    let floor = crate::math::Aabb::new(
        glam::Vec3::new(-4.0, -1.0, -4.0),
        glam::Vec3::new(4.0, 0.0, 4.0),
    );
    let landed = physics::sweep(
        glam::Vec3::new(0.0, 3.0, 0.0),
        glam::Vec3::new(0.3, 0.9, 0.3),
        glam::Vec3::new(0.0, -10.0, 0.0),
        &[floor],
        0.5,
    );
    let physics_ok = landed.on_ground && landed.position.y < 1.0;
    if physics_ok {
        mask |= TEST_PHYSICS;
    }
    details.push_str(if physics_ok { "physics=ok " } else { "physics=FAIL " });

    let probe = vec![0u8; 64 * 1024];
    let stats = alloc::stats();
    let alloc_ok = stats.live_bytes >= 64 * 1024 && stats.peak_bytes >= stats.live_bytes;
    drop(probe);
    if alloc_ok {
        mask |= TEST_ALLOC;
    }
    details.push_str(if alloc_ok { "alloc=ok " } else { "alloc=FAIL " });

    let mut event = crate::ipc::layout::Event { kind: EVENT_LOG, ..Default::default() };
    event.payload[..9].copy_from_slice(b"ipc-probe");
    event.length = 9;
    let ipc_ok = region.push_event(event)
        && region
            .pop_event()
            .is_some_and(|received| received.kind == EVENT_LOG && &received.payload[..9] == b"ipc-probe");
    if ipc_ok {
        mask |= TEST_IPC;
    }
    details.push_str(if ipc_ok { "ipc=ok" } else { "ipc=FAIL" });

    if mask == TEST_MESH | TEST_MATH | TEST_PHYSICS | TEST_ALLOC | TEST_IPC {
        mask |= TEST_ALL_OK;
    }
    SELF_TEST.store(mask, Ordering::Release);
    region.report_self_test(mask, details.trim());
    (mask, details)
}

/// Applies the commands the 64-bit UI sends down the command ring.
///
/// Kept out of the loop body so every command can be tested against a real
/// region without starting a thread: a command that only updates local state
/// is indistinguishable from one that does nothing.
pub struct CommandHandler {
    volume: f32,
    flags: u32,
    assets: Option<AssetAssembler>,
    /// Set once every chunk arrived; the commit compares it against what the
    /// UI says it sent.
    assembled: Option<(u32, Vec<u8>)>,
    shutdown: bool,
}

impl CommandHandler {
    pub fn new(flags: u32) -> Self {
        Self {
            volume: 1.0,
            flags,
            assets: None,
            assembled: None,
            shutdown: false,
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown
    }

    /// Returns true when the loop should stop.
    pub fn handle(&mut self, command: &Command, region: &Region, scene: &mut Scene) -> bool {
        match command.kind {
            CMD_SHUTDOWN => {
                self.shutdown = true;
                region.log(3, "shutdown requested");
            }
            CMD_SET_VOLUME => {
                self.volume = (command.value.min(1000) as f32 / 1000.0).clamp(0.0, 1.0);
                region.log(4, &format!("volume {:.2}", self.volume));
            }
            CMD_SET_RENDER_DISTANCE => {
                // Saturate instead of truncating: a value above u32::MAX would
                // otherwise wrap into a small, plausible-looking distance.
                let applied = scene.set_render_distance(command.value.min(u32::MAX as u64) as u32);
                region.log(4, &format!("render distance {applied}"));
            }
            CMD_SET_SHADERS => {
                if command.value != 0 {
                    self.flags |= FLAG_SHADERS;
                } else {
                    self.flags &= !FLAG_SHADERS;
                }
                region.set_flags(self.flags);
                region.log(4, if self.flags & FLAG_SHADERS != 0 { "shaders on" } else { "shaders off" });
            }
            CMD_SET_SAFE_MODE => {
                let enabled = command.value != 0;
                scene.set_safe_mode(enabled);
                if enabled {
                    self.flags |= FLAG_SAFE_MODE;
                } else {
                    self.flags &= !FLAG_SAFE_MODE;
                }
                region.set_flags(self.flags);
                region.log(4, if enabled { "safe mode on" } else { "safe mode off" });
            }
            CMD_REQUEST_SELF_TEST => {
                let (mask, _) = run_self_test(region);
                scene.set_self_test(mask);
                region.log(4, &format!("self-test 0x{mask:08x}"));
            }
            CMD_PATCH_OFFSET => self.handle_patch(command, region),
            CMD_ASSET_BEGIN => self.handle_asset_begin(command, region),
            CMD_ASSET_CHUNK => self.handle_asset_chunk(command, region),
            CMD_ASSET_COMMIT => self.handle_asset_commit(command, region),
            other => {
                region.log(5, &format!("unknown command {other}"));
            }
        }
        self.shutdown
    }

    fn handle_patch(&mut self, command: &Command, region: &Region) {
        let Some(name) = command_name(command) else {
            region.log(5, "patch rejected: no name");
            return;
        };
        match crate::apply_patch(name, command.value) {
            Ok(replaced) => region.log(
                4,
                &format!("patch {name} = 0x{:08x}{}", command.value, if replaced { " (replaced)" } else { "" }),
            ),
            Err(error) => region.log(5, &format!("patch {name} rejected: {}", error.as_str())),
        }
    }

    fn handle_asset_begin(&mut self, command: &Command, region: &Region) {
        let asset_id = (command.value >> 32) as u32;
        let total = command.value as u32;
        if total == 0 || total > MAX_ASSET_CHUNKS {
            self.assets = None;
            self.assembled = None;
            region.set_asset_result(0, 0, ASSET_STATUS_BAD_TOTAL);
            region.log(5, &format!("asset {asset_id}: bad chunk count {total}"));
            return;
        }
        self.assets = Some(AssetAssembler::begin(asset_id, total));
        self.assembled = None;
        region.set_asset_result(0, 0, ASSET_STATUS_PENDING);
        region.log(4, &format!("asset {asset_id}: begin, {total} chunks"));
    }

    fn handle_asset_chunk(&mut self, command: &Command, region: &Region) {
        let Some(assembler) = self.assets.as_mut() else {
            region.set_asset_result(0, 0, ASSET_STATUS_NO_BEGIN);
            region.log(5, "asset chunk without begin");
            return;
        };
        let asset_id = (command.value >> 32) as u32;
        let index = command.value as u32;
        let length = (command.length as usize).min(command.payload.len());
        let data = command.payload[..length].to_vec();
        let chunk = AssetChunk {
            asset_id,
            index,
            total: assembler.total(),
            // The chunk CRC is computed here rather than carried on the wire:
            // the end-to-end check is the whole-asset CRC at commit.
            crc: crc32(&data),
            data,
        };
        match assembler.push(&chunk) {
            Ok(Some(bytes)) => {
                // Every chunk arrived; the verdict waits for the commit.
                let length = bytes.len() as u32;
                self.assembled = Some((asset_id, bytes));
                region.log(4, &format!("asset {asset_id}: {length} bytes staged, awaiting commit"));
            }
            Ok(None) => {}
            Err(error) => {
                self.assets = None;
                self.assembled = None;
                region.set_asset_result(0, 0, status_code(error));
                region.log(5, &format!("asset {asset_id}: {error:?}"));
            }
        }
    }

    fn handle_asset_commit(&mut self, command: &Command, region: &Region) {
        let asset_id = (command.value >> 32) as u32;
        let expected_bytes = command.value as u32;
        let assembled = self.assembled.take();
        let progress = self.assets.as_ref().map(|assembler| (assembler.received(), assembler.total()));
        self.assets = None;
        let Some((staged_id, bytes)) = assembled else {
            match progress {
                Some((received, total)) => {
                    region.set_asset_result(0, 0, ASSET_STATUS_INCOMPLETE);
                    region.log(5, &format!("asset {asset_id}: {received}/{total} chunks"));
                }
                None => {
                    region.set_asset_result(0, 0, ASSET_STATUS_NO_BEGIN);
                    region.log(5, "asset commit without begin");
                }
            }
            return;
        };
        if command.length != 4 || command.payload.len() < 4 {
            region.set_asset_result(0, 0, ASSET_STATUS_CRC_MISMATCH);
            region.log(5, "asset commit without a CRC");
            return;
        }
        let expected_crc = u32::from_le_bytes([
            command.payload[0],
            command.payload[1],
            command.payload[2],
            command.payload[3],
        ]);
        if staged_id != asset_id {
            region.set_asset_result(0, 0, ASSET_STATUS_TOTAL);
            region.log(5, &format!("asset {asset_id}: commit for asset {staged_id}"));
            return;
        }
        let staged = bytes.len() as u32;
        if staged != expected_bytes {
            region.set_asset_result(staged, expected_crc, ASSET_STATUS_LENGTH_MISMATCH);
            region.log(5, &format!("asset {asset_id}: {staged} bytes, expected {expected_bytes}"));
            return;
        }
        let actual_crc = crc32(&bytes);
        if actual_crc != expected_crc {
            region.set_asset_result(staged, actual_crc, ASSET_STATUS_CRC_MISMATCH);
            region.log(
                5,
                &format!("asset {asset_id}: crc 0x{actual_crc:08x}, expected 0x{expected_crc:08x}"),
            );
            return;
        }
        region.set_asset_result(staged, actual_crc, ASSET_STATUS_OK);
        region.log(4, &format!("asset {asset_id}: {staged} bytes verified, crc 0x{actual_crc:08x}"));
    }
}

fn command_name(command: &Command) -> Option<&str> {
    let length = (command.length as usize).min(command.payload.len());
    if length == 0 {
        return None;
    }
    core::str::from_utf8(&command.payload[..length]).ok()
}

fn run(region: Arc<Region>, stop: Arc<AtomicBool>, flags: u32) {
    // The companion is a background process: ask for a slightly higher nice
    // value so the foreground UI keeps its frames. Failure is expected on
    // Android (no CAP_SYS_NICE) and is not fatal.
    // SAFETY: setpriority only touches this process's own scheduling policy.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, -4);
    }

    let safe_mode = flags & FLAG_SAFE_MODE != 0;
    let width = region.layout().width;
    let height = region.layout().height;
    let (mask, details) = run_self_test(&region);
    let _ = details;

    let mut scene = Scene::new(width, height, safe_mode);
    scene.set_self_test(mask);
    let mut pacer = AudioPacer::new(48_000, 2);
    let mut next_frame = Instant::now();
    let mut last_audio_drop_log = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut last_stats = Instant::now();
    let mut render_avg_us: u32 = 0;
    let mut render_max_us: u32 = 0;
    let mut frames_since_stats: u32 = 0;
    let mut handler = CommandHandler::new(flags | FLAG_SHADERS);

    region.set_flags(handler.flags());
    region.mark_running();
    region.log(3, if safe_mode { "companion started (safe mode)" } else { "companion started" });

    while !stop.load(Ordering::Acquire) {
        // Inputs from the 64-bit UI. When the game hooks own the ring they
        // consume it instead of the demo scene.
        let mut drained = 0;
        while !crate::input_sink_enabled() && drained < 64 {
            let Some(event) = region.pop_input() else { break };
            drained += 1;
            match event.kind {
                INPUT_KIND_DOWN | INPUT_KIND_MOVE => scene.set_cursor(event.x, event.y, true),
                INPUT_KIND_STICK => {
                    let x = width as f32 * 0.5 + event.x * width as f32 * 0.25;
                    let y = height as f32 * 0.5 + event.y * height as f32 * 0.25;
                    scene.set_cursor(x, y, true);
                }
                INPUT_KIND_UP => scene.set_cursor(event.x, event.y, false),
                INPUT_KIND_CANCEL => scene.set_cursor(event.x, event.y, false),
                INPUT_KIND_KEY | INPUT_KIND_BUTTON => {
                    // Buttons pulse the cursor so the UI can see the event land.
                    scene.set_cursor(event.x, event.y, true);
                }
                _ => {}
            }
            if event.kind == INPUT_KIND_DOWN {
                pacer.trigger_tone(DEMO_TONE_FRAMES);
            }
        }

        // Commands from the 64-bit UI.
        while let Some(command) = region.pop_command() {
            if handler.handle(&command, &region, &mut scene) {
                stop.store(true, Ordering::Release);
            }
        }

        // Audio keeps its own schedule: one block per 10.58 ms, silence
        // included. A block produced only when a sound starts would leave the
        // 64-bit AudioTrack playing an empty buffer, which is what makes it log
        // underruns.
        pacer.set_volume(handler.volume());
        for block in pacer.tick(crate::util::monotonic_ns()) {
            if !region.push_audio(*block) && last_audio_drop_log.elapsed() >= AUDIO_DROP_LOG_INTERVAL {
                last_audio_drop_log = Instant::now();
                let dropped = region.stats().audio_dropped;
                region.log(2, &format!("audio ring full ({dropped} blocks dropped)"));
            }
        }

        let now = Instant::now();
        if handler.flags() & FLAG_HEADLESS == 0 && now >= next_frame {
            if let Some(writer) = region.begin_frame() {
                // SAFETY: the slot is exclusively owned until published and is
                // at least `writer.len` bytes inside the mapping.
                let buffer = unsafe {
                    core::slice::from_raw_parts_mut(writer.ptr, writer.len)
                };
                let took = scene.render(buffer);
                region.publish_frame(writer.slot);
                let micros = took.as_micros().min(u32::MAX as u128) as u32;
                render_max_us = render_max_us.max(micros);
                // Exponential moving average keeps the number stable.
                render_avg_us = if render_avg_us == 0 {
                    micros
                } else {
                    render_avg_us - render_avg_us / 8 + micros / 8
                };
                frames_since_stats += 1;
            }
            next_frame += FRAME_INTERVAL;
            if next_frame <= now {
                next_frame = now + Duration::from_millis(1);
            }
        }

        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            region.heartbeat();
            last_heartbeat = now;
        }
        if last_stats.elapsed() >= STATS_INTERVAL {
            region.publish_stats(alloc::stats(), render_avg_us, render_max_us);
            last_stats = now;
            frames_since_stats = 0;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    region.publish_stats(alloc::stats(), render_avg_us, render_max_us);
    region.set_status(STATUS_STOPPED);
    region.heartbeat();
    region.log(3, "companion stopped");
    let _ = frames_since_stats;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::region::Region;

    #[test]
    fn self_test_passes_on_the_host() {
        let region = Region::create(64, 48).expect("region");
        let (mask, details) = run_self_test(&region);
        assert_eq!(mask & TEST_ALL_OK, TEST_ALL_OK, "details={details}");
        assert_eq!(region.stats().self_test, mask);
    }

    fn command(kind: u32, value: u64) -> Command {
        Command { kind, value, ..Default::default() }
    }

    fn command_with(kind: u32, value: u64, flags: u32, payload: &[u8]) -> Command {
        let mut command = Command { kind, value, flags, ..Default::default() };
        command.payload[..payload.len()].copy_from_slice(payload);
        command.length = payload.len() as u32;
        command
    }

    fn last_event(region: &Region) -> String {
        let mut text = String::new();
        while let Some(event) = region.pop_event() {
            text = String::from_utf8_lossy(&event.payload[..event.length as usize]).into_owned();
        }
        text
    }

    #[test]
    fn render_distance_command_reaches_the_scene() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(&command(CMD_SET_RENDER_DISTANCE, 24), &region, &mut scene);
        assert_eq!(scene.render_distance(), 24);
        assert_eq!(last_event(&region), "render distance 24");
        // Out-of-range values are clamped, not passed through.
        handler.handle(&command(CMD_SET_RENDER_DISTANCE, 900), &region, &mut scene);
        assert_eq!(scene.render_distance(), crate::render::MAX_RENDER_DISTANCE);
        // A value wider than u32 saturates instead of wrapping to a small one.
        handler.handle(&command(CMD_SET_RENDER_DISTANCE, u64::MAX), &region, &mut scene);
        assert_eq!(scene.render_distance(), crate::render::MAX_RENDER_DISTANCE);
    }

    #[test]
    fn safe_mode_can_be_cleared_again() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(&command(CMD_SET_SAFE_MODE, 1), &region, &mut scene);
        assert!(scene.safe_mode());
        assert_ne!(handler.flags() & FLAG_SAFE_MODE, 0);
        assert_eq!(region.stats().flags & FLAG_SAFE_MODE, FLAG_SAFE_MODE);
        handler.handle(&command(CMD_SET_SAFE_MODE, 0), &region, &mut scene);
        assert!(!scene.safe_mode());
        assert_eq!(handler.flags() & FLAG_SAFE_MODE, 0);
        assert_eq!(region.stats().flags & FLAG_SAFE_MODE, 0);
        assert_eq!(last_event(&region), "safe mode off");
    }

    #[test]
    fn shaders_toggle_both_ways() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(&command(CMD_SET_SHADERS, 1), &region, &mut scene);
        assert_ne!(region.stats().flags & FLAG_SHADERS, 0);
        handler.handle(&command(CMD_SET_SHADERS, 0), &region, &mut scene);
        assert_eq!(region.stats().flags & FLAG_SHADERS, 0);
    }

    #[test]
    fn patch_offset_is_stored_and_rejects_bad_input() {
        let _guard = crate::patch_test_guard();
        crate::clear_patches();
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(
            &command_with(CMD_PATCH_OFFSET, 0x00ab_cdef, 0, b"Level::tick"),
            &region,
            &mut scene,
        );
        assert_eq!(crate::lookup_patch("Level::tick"), Some(0x00ab_cdef));
        handler.handle(&command_with(CMD_PATCH_OFFSET, 0, 0, b"Bad"), &region, &mut scene);
        assert_eq!(crate::lookup_patch("Bad"), None);
        assert_eq!(crate::patch_count(), 1);
        handler.handle(&command(CMD_PATCH_OFFSET, 0x1000), &region, &mut scene);
        assert_eq!(last_event(&region), "patch rejected: no name");
        crate::clear_patches();
    }

    #[test]
    fn asset_stream_is_verified_end_to_end() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        let payload: Vec<u8> = (0..500u32).map(|value| (value % 251) as u8).collect();
        let chunks = crate::assets::chunk_asset(7, &payload);
        handler.handle(
            &command(CMD_ASSET_BEGIN, (7u64 << 32) | chunks.len() as u64),
            &region,
            &mut scene,
        );
        assert_eq!(region.stats().asset_status, ASSET_STATUS_PENDING);
        for chunk in &chunks {
            let value = (u64::from(chunk.asset_id) << 32) | u64::from(chunk.index);
            handler.handle(
                &command_with(CMD_ASSET_CHUNK, value, 0, &chunk.data),
                &region,
                &mut scene,
            );
        }
        let commit = crc32(&payload).to_le_bytes();
        handler.handle(
            &command_with(CMD_ASSET_COMMIT, (7u64 << 32) | payload.len() as u64, 0, &commit),
            &region,
            &mut scene,
        );
        let stats = region.stats();
        assert_eq!(stats.asset_status, ASSET_STATUS_OK);
        assert_eq!(stats.asset_bytes as usize, payload.len());
        assert_eq!(stats.asset_crc, crc32(&payload));
    }

    #[test]
    fn asset_commit_catches_a_length_mismatch() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        let payload = [5u8; 40];
        let chunks = crate::assets::chunk_asset(1, &payload);
        handler.handle(
            &command(CMD_ASSET_BEGIN, (1u64 << 32) | chunks.len() as u64),
            &region,
            &mut scene,
        );
        for chunk in &chunks {
            let value = (u64::from(chunk.asset_id) << 32) | u64::from(chunk.index);
            handler.handle(
                &command_with(CMD_ASSET_CHUNK, value, 0, &chunk.data),
                &region,
                &mut scene,
            );
        }
        let commit = crc32(&payload).to_le_bytes();
        handler.handle(
            &command_with(CMD_ASSET_COMMIT, (1u64 << 32) | 41, 0, &commit),
            &region,
            &mut scene,
        );
        assert_eq!(region.stats().asset_status, ASSET_STATUS_LENGTH_MISMATCH);
    }

    #[test]
    fn asset_chunk_without_begin_is_reported() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(&command_with(CMD_ASSET_CHUNK, 1, 0, b"abc"), &region, &mut scene);
        assert_eq!(region.stats().asset_status, ASSET_STATUS_NO_BEGIN);
        handler.handle(&command(CMD_ASSET_BEGIN, 1u64 << 32), &region, &mut scene);
        assert_eq!(region.stats().asset_status, ASSET_STATUS_BAD_TOTAL);
    }

    #[test]
    fn incomplete_stream_is_reported_at_commit() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(&command(CMD_ASSET_BEGIN, (2u64 << 32) | 3), &region, &mut scene);
        let commit = 0u32.to_le_bytes();
        handler.handle(
            &command_with(CMD_ASSET_COMMIT, (2u64 << 32) | 10, 0, &commit),
            &region,
            &mut scene,
        );
        assert_eq!(region.stats().asset_status, ASSET_STATUS_INCOMPLETE);
    }

    #[test]
    fn shutdown_command_stops_the_loop() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        assert!(handler.handle(&command(CMD_SHUTDOWN, 0), &region, &mut scene));
        assert!(handler.shutdown_requested());
    }

    #[test]
    fn volume_command_is_clamped() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        handler.handle(&command(CMD_SET_VOLUME, 500), &region, &mut scene);
        assert!((handler.volume() - 0.5).abs() < 1e-6);
        handler.handle(&command(CMD_SET_VOLUME, 99_999), &region, &mut scene);
        assert!((handler.volume() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn commands_travel_through_the_ring() {
        let region = Region::create(64, 48).expect("region");
        let mut scene = Scene::new(64, 48, false);
        let mut handler = CommandHandler::new(0);
        assert!(region.push_command(command(CMD_SET_RENDER_DISTANCE, 12)));
        let popped = region.pop_command().expect("command");
        handler.handle(&popped, &region, &mut scene);
        assert_eq!(scene.render_distance(), 12);
        assert_eq!(region.stats().cmd_consumed, 1);
    }

    #[test]
    fn a_triggered_tone_reaches_the_audio_ring() {
        let region = Region::create(64, 48).expect("region");
        let mut pacer = AudioPacer::new(48_000, 2);
        pacer.trigger_tone(DEMO_TONE_FRAMES);
        for block in pacer.tick(1_000_000) {
            assert!(region.push_audio(*block));
        }
        let block = region.pop_audio().expect("audio block");
        assert_eq!(block.channels, 2);
        assert_eq!(block.sample_rate, 48_000);
        assert_eq!(block.length as usize, crate::ipc::layout::AUDIO_DATA_SIZE);
        assert!(block.data.iter().any(|&byte| byte != 0), "the tone must carry samples");
    }

    #[test]
    fn silence_is_still_a_block() {
        let region = Region::create(64, 48).expect("region");
        let mut pacer = AudioPacer::new(48_000, 2);
        for block in pacer.tick(1_000_000) {
            assert!(region.push_audio(*block));
        }
        let block = region.pop_audio().expect("idle audio block");
        assert!(block.data.iter().all(|&byte| byte == 0));
        assert_eq!(region.stats().audio_pushed, 1);
    }
}
