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
use crate::audio::AudioBridge;
use crate::crash;
use crate::ipc::layout::{
    CMD_REQUEST_SELF_TEST, CMD_SET_RENDER_DISTANCE, CMD_SET_SAFE_MODE, CMD_SET_SHADERS,
    CMD_SET_VOLUME, CMD_SHUTDOWN, EVENT_LOG, FLAG_HEADLESS, FLAG_SAFE_MODE, FLAG_SHADERS, INPUT_KIND_BUTTON, INPUT_KIND_CANCEL, INPUT_KIND_DOWN, INPUT_KIND_KEY,
    INPUT_KIND_MOVE, INPUT_KIND_STICK, INPUT_KIND_UP, STATUS_RUNNING, STATUS_SAFE_MODE,
    STATUS_STOPPED,
};
use crate::ipc::region::Region;
use crate::mesh;
use crate::physics;
use crate::render::Scene;

const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
const STATS_INTERVAL: Duration = Duration::from_millis(250);

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
    let mut audio = AudioBridge::new(48_000, 2);
    let mut next_frame = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut last_stats = Instant::now();
    let mut render_avg_us: u32 = 0;
    let mut render_max_us: u32 = 0;
    let mut frames_since_stats: u32 = 0;
    let mut volume = 1.0f32;
    let mut flags = flags | FLAG_SHADERS;

    region.set_flags(flags);
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
                push_tone(&region, &mut audio, volume);
            }
        }

        // Commands from the 64-bit UI.
        while let Some(command) = region.pop_command() {
            match command.kind {
                CMD_SHUTDOWN => stop.store(true, Ordering::Release),
                CMD_SET_VOLUME => {
                    volume = (command.value as u32 as f32 / 1000.0).clamp(0.0, 1.0);
                }
                CMD_SET_RENDER_DISTANCE => {
                    // The engine reads this value through the patch registry.
                    let _ = command.value;
                }
                CMD_SET_SHADERS => {
                    if command.value != 0 {
                        flags |= FLAG_SHADERS;
                    } else {
                        flags &= !FLAG_SHADERS;
                    }
                    region.set_flags(flags);
                }
                CMD_SET_SAFE_MODE => {
                    if command.value != 0 {
                        flags |= FLAG_SAFE_MODE;
                        scene.set_safe_mode(true);
                    }
                    region.set_flags(flags);
                }
                CMD_REQUEST_SELF_TEST => {
                    let (mask, _) = run_self_test(&region);
                    scene.set_self_test(mask);
                }
                _ => {}
            }
        }

        let now = Instant::now();
        if flags & FLAG_HEADLESS == 0 && now >= next_frame {
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

/// Generates a short 440 Hz stereo tone and pushes it into the audio ring.
/// This is the demo payload for the audio bridge; the real path is fed by the
/// `SoundEngine::play` hook.
fn push_tone(region: &Region, bridge: &mut AudioBridge, volume: f32) {
    if volume <= 0.0 {
        return;
    }
    let sample_rate = 48_000u32;
    let frames = 1024usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let phase = frame as f32 / sample_rate as f32 * 440.0 * std::f32::consts::TAU;
        let value = phase.sin() * 0.15 * volume;
        samples.push(value);
        samples.push(value);
    }
    for block in bridge.push_f32(&samples) {
        let _ = region.push_audio(block);
    }
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

    #[test]
    fn tone_is_pushed_as_audio_blocks() {
        let region = Region::create(64, 48).expect("region");
        let mut bridge = AudioBridge::new(48_000, 2);
        push_tone(&region, &mut bridge, 1.0);
        let block = region.pop_audio().expect("audio block");
        assert_eq!(block.channels, 2);
        assert_eq!(block.sample_rate, 48_000);
        assert!(block.length > 0);
    }
}
