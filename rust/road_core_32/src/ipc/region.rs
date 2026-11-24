//! Typed view over the shared-memory region: header, frame triple buffer,
//! input/command/event rings.

use core::sync::atomic::Ordering;

use super::layout::{
    AudioBlock, FrameSlotHeader, InputEvent, RegionLayout, RoadHeader, EVENT_SELF_TEST,
    ROAD_ABI_VERSION, ROAD_MAGIC, SLOT_FREE, SLOT_READY, SLOT_READING, SLOT_WRITING, STATUS_EMPTY,
    STATUS_RUNNING,
};
use super::ring::SpscRing;
use super::shm::SharedMemory;

/// A writable frame slot handed to the renderer.
#[derive(Clone, Copy, Debug)]
pub struct FrameWriter {
    pub slot: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub ptr: *mut u8,
    pub len: usize,
}

/// A readable frame slot handed to the presenter.
#[derive(Clone, Copy, Debug)]
pub struct FrameView {
    pub slot: usize,
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub ptr: *const u8,
    pub len: usize,
    pub published_ns: u64,
}

/// Statistics mirrored to Kotlin through the C-ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct RoadStats {
    pub status: u32,
    pub flags: u32,
    pub pid: u32,
    pub width: u32,
    pub height: u32,
    pub heartbeat_seq: u32,
    pub heartbeat_age_ms: u32,
    pub frame_count: u64,
    pub presented_count: u64,
    pub dropped_frames: u32,
    pub input_pushed: u32,
    pub input_consumed: u32,
    pub input_dropped: u32,
    pub cmd_pushed: u32,
    pub cmd_consumed: u32,
    pub cmd_dropped: u32,
    pub event_pushed: u32,
    pub event_consumed: u32,
    pub event_dropped: u32,
    pub last_input_x: u32,
    pub last_input_y: u32,
    pub last_input_kind: u32,
    pub last_input_seq: u32,
    pub audio_pushed: u32,
    pub audio_consumed: u32,
    pub audio_dropped: u32,
    pub heap_used: u64,
    pub heap_peak: u64,
    pub heap_limit: u64,
    pub render_us_avg: u32,
    pub render_us_max: u32,
    pub self_test: u32,
    pub crash_signal: u32,
    pub crash_addr: u64,
    pub crash_pc: u64,
    pub crash_lr: u64,
    /// Bytes staged by the last completed asset stream.
    pub asset_bytes: u32,
    /// CRC32 of the staged asset, as computed by the companion.
    pub asset_crc: u32,
    /// One of the `ASSET_STATUS_*` codes in [`crate::assets`].
    pub asset_status: u32,
}

/// Errors returned when attaching to a foreign region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachError {
    BadMagic(u32),
    BadAbi { found: u32, expected: u32 },
    BadHeaderSize(u32),
    BadDimensions { width: u32, height: u32 },
    TooSmall { needed: usize, found: usize },
    TooLarge { size: usize, max: usize },
}

/// A mapped Road Loader shared region.
pub struct Region {
    shm: SharedMemory,
    layout: RegionLayout,
    input: SpscRing<InputEvent>,
    commands: SpscRing<super::layout::Command>,
    events: SpscRing<super::layout::Event>,
    audio: SpscRing<AudioBlock>,
}

// SAFETY: the region is shared by design. The frame triple buffer and the rings
// use atomics for every cross-thread hand-off; the raw pointers are stable for
// the lifetime of the mapping.
unsafe impl Send for Region {}
unsafe impl Sync for Region {}

impl Region {
    /// Creates and initialises a region of `width x height` RGBA frames.
    pub fn create(width: u32, height: u32) -> Result<Self, &'static str> {
        let layout = RegionLayout::new(width, height);
        let shm = SharedMemory::create(layout.total_size).map_err(|_| "shm create failed")?;
        // SAFETY: the mapping is at least `layout.total_size` bytes and is
        // exclusively owned until the region is published to the peer.
        unsafe { core::ptr::write_bytes(shm.as_ptr(), 0, shm.size()) };
        let region = unsafe { Self::from_parts(shm, layout, true) };
        region.initialize_header();
        Ok(region)
    }

    /// Attaches to a region created by the peer.
    pub fn attach(fd: i32, size: usize) -> Result<Self, AttachError> {
        // The size comes from the peer process; refuse it before it is mapped.
        if size > super::layout::MAX_REGION_BYTES {
            return Err(AttachError::TooLarge { size, max: super::layout::MAX_REGION_BYTES });
        }
        let shm = SharedMemory::from_fd(fd, size).map_err(|_| AttachError::TooSmall {
            needed: size,
            found: 0,
        })?;
        if shm.size() < core::mem::size_of::<RoadHeader>() {
            return Err(AttachError::TooSmall { needed: size, found: shm.size() });
        }
        // SAFETY: the mapping is large enough for the header (checked above).
        let header = unsafe { &*(shm.as_ptr() as *const RoadHeader) };
        if header.magic != ROAD_MAGIC {
            return Err(AttachError::BadMagic(header.magic));
        }
        if header.abi_version != ROAD_ABI_VERSION {
            return Err(AttachError::BadAbi { found: header.abi_version, expected: ROAD_ABI_VERSION });
        }
        if header.header_size as usize != core::mem::size_of::<RoadHeader>() {
            return Err(AttachError::BadHeaderSize(header.header_size));
        }
        let width = header.width.load(Ordering::Relaxed);
        let height = header.height.load(Ordering::Relaxed);
        if width < 16 || height < 16 || width > 8192 || height > 8192 {
            return Err(AttachError::BadDimensions { width, height });
        }
        let layout = RegionLayout::new(width, height);
        if shm.size() < layout.total_size {
            return Err(AttachError::TooSmall { needed: layout.total_size, found: shm.size() });
        }
        // SAFETY: validated magic/version/size above; `from_parts` rebuilds the
        // rings over the existing control blocks without resetting them.
        Ok(unsafe { Self::from_parts(shm, layout, false) })
    }

    /// # Safety
    /// `shm` must be mapped at least `layout.total_size` bytes and contain a
    /// zeroed (when `initialize`) or valid (when attaching) region.
    unsafe fn from_parts(shm: SharedMemory, layout: RegionLayout, initialize: bool) -> Self {
        let base = shm.as_ptr();
        // SAFETY: offsets are inside the mapping by construction.
        let input_region = unsafe { base.add(layout.input_offset) };
        let command_region = unsafe { base.add(layout.command_offset) };
        let event_region = unsafe { base.add(layout.event_offset) };
        let audio_region = unsafe { base.add(layout.audio_offset) };
        let (input, commands, events, audio) = if initialize {
            // SAFETY: each ring region is large enough for its header and data.
            unsafe {
                (
                    SpscRing::from_region(input_region, layout.input_capacity),
                    SpscRing::from_region(command_region, layout.command_capacity),
                    SpscRing::from_region(event_region, layout.event_capacity),
                    SpscRing::from_region(audio_region, layout.audio_capacity),
                )
            }
        } else {
            // SAFETY: the peer initialised the rings with the same capacities.
            unsafe {
                (
                    SpscRing::attach_region(input_region, layout.input_capacity),
                    SpscRing::attach_region(command_region, layout.command_capacity),
                    SpscRing::attach_region(event_region, layout.event_capacity),
                    SpscRing::attach_region(audio_region, layout.audio_capacity),
                )
            }
        };
        Self { shm, layout, input, commands, events, audio }
    }

    fn initialize_header(&self) {
        // SAFETY: `create` owns the region until it is returned to the caller.
        let header = unsafe { self.header_mut() };
        header.magic = ROAD_MAGIC;
        header.abi_version = ROAD_ABI_VERSION;
        header.header_size = core::mem::size_of::<RoadHeader>() as u32;
        header.status.store(STATUS_EMPTY, Ordering::Relaxed);
        header.pid.store(std::process::id(), Ordering::Relaxed);
        header.width.store(self.layout.width, Ordering::Relaxed);
        header.height.store(self.layout.height, Ordering::Relaxed);
        header.stride.store(self.layout.stride, Ordering::Relaxed);
        header.heap_limit.store(crate::alloc::limit() as u64, Ordering::Relaxed);
        for slot in 0..super::layout::FRAME_SLOT_COUNT {
            // SAFETY: the slot header lives inside the mapping.
            let slot_header = unsafe { self.slot_header(slot) };
            slot_header.init(self.layout.width, self.layout.height, self.layout.stride);
        }
    }

    pub fn header(&self) -> &RoadHeader {
        // SAFETY: the mapping starts with a RoadHeader and outlives the region.
        unsafe { &*(self.shm.as_ptr() as *const RoadHeader) }
    }

    /// Mutable header access, only used while the region is exclusively owned.
    ///
    /// # Safety
    /// The caller must guarantee no other thread is reading the header.
    unsafe fn header_mut(&self) -> &mut RoadHeader {
        // SAFETY: the mapping starts with a RoadHeader and outlives the region.
        unsafe { &mut *(self.shm.as_ptr() as *mut RoadHeader) }
    }

    unsafe fn slot_header(&self, slot: usize) -> &FrameSlotHeader {
        debug_assert!(slot < super::layout::FRAME_SLOT_COUNT);
        let offset = core::mem::offset_of!(RoadHeader, frame_slots)
            + slot * core::mem::size_of::<FrameSlotHeader>();
        // SAFETY: the caller guarantees `slot` is in range; the slot header is
        // inside the header region of the live mapping.
        unsafe { &*(self.shm.as_ptr().add(offset) as *const FrameSlotHeader) }
    }

    pub fn layout(&self) -> RegionLayout {
        self.layout
    }

    pub fn fd(&self) -> i32 {
        self.shm.fd()
    }

    /// Base address of the mapping.
    pub fn base_ptr(&self) -> *mut u8 {
        self.shm.as_ptr()
    }

    pub fn duplicate_fd(&self) -> Option<i32> {
        self.shm.duplicate_fd().ok()
    }

    // ---- lifecycle -------------------------------------------------------

    pub fn set_status(&self, status: u32) {
        self.header().status.store(status, Ordering::Release);
    }

    pub fn status(&self) -> u32 {
        self.header().status.load(Ordering::Acquire)
    }

    pub fn set_flags(&self, flags: u32) {
        self.header().flags.store(flags, Ordering::Release);
    }

    pub fn flags(&self) -> u32 {
        self.header().flags.load(Ordering::Acquire)
    }

    pub fn set_pid(&self, pid: u32) {
        self.header().pid.store(pid, Ordering::Release);
    }

    pub fn heartbeat(&self) {
        let header = self.header();
        header.heartbeat_ns.store(crate::util::monotonic_ns(), Ordering::Release);
        header.heartbeat_seq.fetch_add(1, Ordering::Release);
    }

    pub fn heartbeat_age_ms(&self) -> u32 {
        let last = self.header().heartbeat_ns.load(Ordering::Acquire);
        if last == 0 {
            return u32::MAX;
        }
        (crate::util::elapsed_ns(last) / 1_000_000).min(u32::MAX as u64) as u32
    }

    pub fn publish_stats(&self, alloc: crate::alloc::AllocStats, render_us_avg: u32, render_us_max: u32) {
        let header = self.header();
        // The UI needs the memory the kernel charges, not just the tracked
        // Rust allocations; RSS includes the shared frame slots.
        header.heap_used.store(alloc.rss_bytes.max(alloc.live_bytes), Ordering::Relaxed);
        header.heap_peak.store(alloc.peak_bytes.max(alloc.rss_bytes), Ordering::Relaxed);
        header.heap_limit.store(alloc.limit_bytes, Ordering::Relaxed);
        header.render_us_avg.store(render_us_avg, Ordering::Relaxed);
        header.render_us_max.store(render_us_max, Ordering::Relaxed);
    }

    pub fn stats(&self) -> RoadStats {
        let header = self.header();
        RoadStats {
            status: header.status.load(Ordering::Acquire),
            flags: header.flags.load(Ordering::Relaxed),
            pid: header.pid.load(Ordering::Relaxed),
            width: header.width.load(Ordering::Relaxed),
            height: header.height.load(Ordering::Relaxed),
            heartbeat_seq: header.heartbeat_seq.load(Ordering::Acquire),
            heartbeat_age_ms: self.heartbeat_age_ms(),
            frame_count: header.frame_count.load(Ordering::Relaxed),
            presented_count: header.presented_count.load(Ordering::Relaxed),
            dropped_frames: header.dropped_frames.load(Ordering::Relaxed),
            input_pushed: header.input_pushed.load(Ordering::Relaxed),
            input_consumed: header.input_consumed.load(Ordering::Relaxed),
            input_dropped: header.input_dropped.load(Ordering::Relaxed),
            cmd_pushed: header.cmd_pushed.load(Ordering::Relaxed),
            cmd_consumed: header.cmd_consumed.load(Ordering::Relaxed),
            cmd_dropped: header.cmd_dropped.load(Ordering::Relaxed),
            event_pushed: header.event_pushed.load(Ordering::Relaxed),
            event_consumed: header.event_consumed.load(Ordering::Relaxed),
            event_dropped: header.event_dropped.load(Ordering::Relaxed),
            last_input_x: header.last_input_x.load(Ordering::Relaxed),
            last_input_y: header.last_input_y.load(Ordering::Relaxed),
            last_input_kind: header.last_input_kind.load(Ordering::Relaxed),
            last_input_seq: header.last_input_seq.load(Ordering::Relaxed),
            audio_pushed: header.audio_pushed.load(Ordering::Relaxed),
            audio_consumed: header.audio_consumed.load(Ordering::Relaxed),
            audio_dropped: header.audio_dropped.load(Ordering::Relaxed),
            heap_used: header.heap_used.load(Ordering::Relaxed),
            heap_peak: header.heap_peak.load(Ordering::Relaxed),
            heap_limit: header.heap_limit.load(Ordering::Relaxed),
            render_us_avg: header.render_us_avg.load(Ordering::Relaxed),
            render_us_max: header.render_us_max.load(Ordering::Relaxed),
            self_test: header.self_test.load(Ordering::Relaxed),
            crash_signal: header.crash_signal.load(Ordering::Acquire),
            crash_addr: header.crash_addr.load(Ordering::Relaxed),
            crash_pc: header.crash_pc.load(Ordering::Relaxed),
            crash_lr: header.crash_lr.load(Ordering::Relaxed),
            asset_bytes: header.asset_bytes.load(Ordering::Relaxed),
            asset_crc: header.asset_crc.load(Ordering::Relaxed),
            asset_status: header.asset_status.load(Ordering::Relaxed),
        }
    }

    /// Publishes the outcome of an asset stream so the UI can poll it.
    pub fn set_asset_result(&self, bytes: u32, crc: u32, status: u32) {
        let header = self.header();
        header.asset_bytes.store(bytes, Ordering::Relaxed);
        header.asset_crc.store(crc, Ordering::Relaxed);
        // Release: the byte count and CRC must be visible before the status.
        header.asset_status.store(status, Ordering::Release);
    }

    // ---- frame triple buffer --------------------------------------------

    /// Reserves a free slot for the renderer.
    pub fn begin_frame(&self) -> Option<FrameWriter> {
        let header = self.header();
        for slot in 0..super::layout::FRAME_SLOT_COUNT {
            // SAFETY: slot index is bounded by the loop.
            let slot_header = unsafe { self.slot_header(slot) };
            if slot_header
                .state
                .compare_exchange(SLOT_FREE, SLOT_WRITING, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Some(FrameWriter {
                    slot,
                    width: self.layout.width,
                    height: self.layout.height,
                    stride: self.layout.stride,
                    ptr: unsafe { self.shm.as_ptr().add(self.layout.frame_slot_offset(slot)) },
                    len: self.layout.frame_bytes,
                });
            }
        }
        header.dropped_frames.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Publishes a slot previously returned by [`Self::begin_frame`].
    pub fn publish_frame(&self, slot: usize) -> u64 {
        let header = self.header();
        let seq = header.frame_count.fetch_add(1, Ordering::Relaxed) + 1;
        // SAFETY: `slot` came from `begin_frame`.
        let slot_header = unsafe { self.slot_header(slot) };
        slot_header.seq.store(seq, Ordering::Relaxed);
        slot_header.published_ns.store(crate::util::monotonic_ns(), Ordering::Relaxed);
        slot_header.state.store(SLOT_READY, Ordering::Release);
        seq
    }

    /// Claims the newest ready frame for presentation.
    pub fn acquire_frame(&self) -> Option<FrameView> {
        let mut best: Option<FrameView> = None;
        for slot in 0..super::layout::FRAME_SLOT_COUNT {
            // SAFETY: slot index is bounded by the loop.
            let slot_header = unsafe { self.slot_header(slot) };
            if slot_header.state.load(Ordering::Acquire) != SLOT_READY {
                continue;
            }
            let seq = slot_header.seq.load(Ordering::Relaxed);
            if best.as_ref().is_some_and(|current| current.seq >= seq) {
                continue;
            }
            if slot_header
                .state
                .compare_exchange(SLOT_READY, SLOT_READING, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                if let Some(previous) = best.take() {
                    // A newer frame won; return the older claim immediately.
                    self.release_frame(previous.slot);
                }
                best = Some(FrameView {
                    slot,
                    seq,
                    width: slot_header.width,
                    height: slot_header.height,
                    stride: slot_header.stride,
                    ptr: unsafe { self.shm.as_ptr().add(self.layout.frame_slot_offset(slot)) },
                    len: self.layout.frame_bytes,
                    published_ns: slot_header.published_ns.load(Ordering::Relaxed),
                });
            }
        }
        best
    }

    /// Returns a presented slot to the renderer.
    pub fn release_frame(&self, slot: usize) {
        if slot >= super::layout::FRAME_SLOT_COUNT {
            return;
        }
        // SAFETY: slot index validated above.
        let slot_header = unsafe { self.slot_header(slot) };
        slot_header.state.store(SLOT_FREE, Ordering::Release);
        self.header().presented_count.fetch_add(1, Ordering::Relaxed);
    }

    // ---- rings -----------------------------------------------------------

    pub fn push_input(&self, mut event: InputEvent) -> bool {
        let header = self.header();
        event.seq = header.input_pushed.load(Ordering::Relaxed) + 1;
        let pushed = self.input.push(event);
        if pushed {
            header.input_pushed.store(event.seq, Ordering::Relaxed);
            header.last_input_x.store(event.x.to_bits(), Ordering::Relaxed);
            header.last_input_y.store(event.y.to_bits(), Ordering::Relaxed);
            header.last_input_kind.store(event.kind, Ordering::Relaxed);
            header.last_input_seq.store(event.seq, Ordering::Relaxed);
        } else {
            header.input_dropped.fetch_add(1, Ordering::Relaxed);
        }
        pushed
    }

    pub fn pop_input(&self) -> Option<InputEvent> {
        let event = self.input.pop()?;
        self.header().input_consumed.fetch_add(1, Ordering::Relaxed);
        Some(event)
    }

    pub fn input_len(&self) -> u32 {
        self.input.len()
    }

    pub fn push_command(&self, mut command: super::layout::Command) -> bool {
        let header = self.header();
        command.seq = header.cmd_pushed.load(Ordering::Relaxed) + 1;
        let pushed = self.commands.push(command);
        if pushed {
            header.cmd_pushed.store(command.seq, Ordering::Relaxed);
        } else {
            header.cmd_dropped.fetch_add(1, Ordering::Relaxed);
        }
        pushed
    }

    pub fn pop_command(&self) -> Option<super::layout::Command> {
        let command = self.commands.pop()?;
        self.header().cmd_consumed.fetch_add(1, Ordering::Relaxed);
        Some(command)
    }

    pub fn push_event(&self, mut event: super::layout::Event) -> bool {
        let header = self.header();
        event.seq = header.event_pushed.load(Ordering::Relaxed) + 1;
        let pushed = self.events.push(event);
        if pushed {
            header.event_pushed.store(event.seq, Ordering::Relaxed);
        } else {
            header.event_dropped.fetch_add(1, Ordering::Relaxed);
        }
        pushed
    }

    pub fn pop_event(&self) -> Option<super::layout::Event> {
        let event = self.events.pop()?;
        self.header().event_consumed.fetch_add(1, Ordering::Relaxed);
        Some(event)
    }

    pub fn push_audio(&self, mut block: AudioBlock) -> bool {
        let header = self.header();
        block.seq = header.audio_pushed.load(Ordering::Relaxed) + 1;
        let pushed = self.audio.push(block);
        if pushed {
            header.audio_pushed.store(block.seq, Ordering::Relaxed);
        } else {
            header.audio_dropped.fetch_add(1, Ordering::Relaxed);
        }
        pushed
    }

    pub fn pop_audio(&self) -> Option<AudioBlock> {
        let block = self.audio.pop()?;
        self.header().audio_consumed.fetch_add(1, Ordering::Relaxed);
        Some(block)
    }

    pub fn log(&self, level: u32, message: &str) {
        let mut event = super::layout::Event { kind: super::layout::EVENT_LOG, flags: level, ..Default::default() };
        let bytes = message.as_bytes();
        let length = bytes.len().min(event.payload.len());
        event.payload[..length].copy_from_slice(&bytes[..length]);
        event.length = length as u32;
        let _ = self.push_event(event);
    }

    pub fn report_self_test(&self, mask: u32, details: &str) {
        let header = self.header();
        header.self_test.store(mask, Ordering::Release);
        let mut event = super::layout::Event { kind: EVENT_SELF_TEST, ..Default::default() };
        let bytes = details.as_bytes();
        let length = bytes.len().min(event.payload.len());
        event.payload[..length].copy_from_slice(&bytes[..length]);
        event.length = length as u32;
        event.value = u64::from(mask);
        let _ = self.push_event(event);
    }

    pub fn mark_running(&self) {
        self.set_status(STATUS_RUNNING);
        self.heartbeat();
    }

    /// Writes a crash record into the header from a signal handler.
    ///
    /// # Safety
    /// Only async-signal-safe operations are used: atomics and a bounded copy.
    pub unsafe fn record_crash(&self, signal: u32, code: u32, addr: u64, pc: u64, lr: u64, message: &[u8]) {
        let header = self.header();
        let length = message.len().min(header.crash_message.len());
        // SAFETY: the message buffer is part of the live mapping; this is the
        // only writer and it runs after the process is already faulting.
        unsafe {
            let dst = (self.shm.as_ptr() as *mut u8)
                .add(core::mem::offset_of!(RoadHeader, crash_message));
            core::ptr::copy_nonoverlapping(message.as_ptr(), dst, length);
        }
        header.crash_signal.store(signal, Ordering::SeqCst);
        header.crash_addr.store(addr, Ordering::SeqCst);
        header.crash_pc.store(pc, Ordering::SeqCst);
        header.crash_lr.store(lr, Ordering::SeqCst);
        header.status.store(super::layout::STATUS_CRASHED, Ordering::SeqCst);
        header.crash_code.store(code, Ordering::SeqCst);
    }

    pub fn crash_message(&self) -> String {
        let header = self.header();
        let end = header.crash_message.iter().position(|&b| b == 0).unwrap_or(header.crash_message.len());
        String::from_utf8_lossy(&header.crash_message[..end]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::layout::{EVENT_LOG, STATUS_RUNNING};

    fn region() -> Region {
        Region::create(64, 48).expect("region")
    }

    #[test]
    fn audio_crosses_two_mappings() {
        let producer = Region::create(64, 48).expect("region");
        let fd = producer.duplicate_fd().expect("duplicate fd");
        let size = producer.layout().total_size;
        let consumer = Region::attach(fd, size).expect("attach");
        let block = AudioBlock {
            channels: 2,
            sample_rate: 48_000,
            length: 16,
            ..Default::default()
        };
        assert!(producer.push_audio(block));
        let received = consumer.pop_audio().expect("audio block crosses mappings");
        assert_eq!(received.channels, 2);
        assert_eq!(received.length, 16);
    }

    #[test]
    fn create_initialises_header_and_slots() {
        let region = region();
        let header = region.header();
        assert_eq!(header.magic, ROAD_MAGIC);
        assert_eq!(header.abi_version, ROAD_ABI_VERSION);
        assert_eq!(header.width.load(Ordering::Relaxed), 64);
        assert_eq!(header.height.load(Ordering::Relaxed), 48);
        assert_eq!(region.stats().dropped_frames, 0);
        assert!(region.acquire_frame().is_none());
    }

    #[test]
    fn frames_are_published_and_released() {
        let region = region();
        let writer = region.begin_frame().expect("slot");
        assert_eq!(writer.len, region.layout().frame_bytes);
        // SAFETY: the slot is exclusively owned until published.
        unsafe { core::ptr::write_bytes(writer.ptr, 0x7F, writer.len) };
        let seq = region.publish_frame(writer.slot);
        assert_eq!(seq, 1);
        let view = region.acquire_frame().expect("frame");
        assert_eq!(view.seq, 1);
        assert_eq!(view.width, 64);
        // A claimed slot cannot be claimed twice.
        assert!(region.acquire_frame().is_none());
        region.release_frame(view.slot);
        assert!(region.acquire_frame().is_none());
        assert_eq!(region.stats().presented_count, 1);
    }

    #[test]
    fn triple_buffer_never_overwrites_a_claimed_slot() {
        let region = region();
        let first = region.begin_frame().unwrap();
        region.publish_frame(first.slot);
        let claimed = region.acquire_frame().unwrap();
        let second = region.begin_frame().unwrap();
        assert_ne!(first.slot, second.slot);
        region.publish_frame(second.slot);
        let third = region.begin_frame().unwrap();
        assert_ne!(third.slot, first.slot);
        assert_ne!(third.slot, second.slot);
        // All three slots are now in use, so the next acquire must fail closed.
        assert!(region.begin_frame().is_none());
        assert_eq!(region.stats().dropped_frames, 1);
        region.release_frame(claimed.slot);
        assert!(region.begin_frame().is_some());
    }

    #[test]
    fn input_ring_round_trips() {
        let region = region();
        let event = InputEvent { kind: 1, x: 12.5, y: 7.25, ..Default::default() };
        assert!(region.push_input(event));
        let received = region.pop_input().expect("input");
        assert_eq!(received.kind, 1);
        assert_eq!(received.x, 12.5);
        assert_eq!(received.seq, 1);
        let stats = region.stats();
        assert_eq!(stats.input_pushed, 1);
        assert_eq!(stats.input_consumed, 1);
        assert_eq!(stats.last_input_x, 12.5f32.to_bits());
    }

    #[test]
    fn events_carry_logs_and_self_test() {
        let region = region();
        region.log(2, "hello");
        region.report_self_test(0x8000_0007, "mesh=ok math=ok");
        let log = region.pop_event().expect("log");
        assert_eq!(log.kind, EVENT_LOG);
        assert_eq!(&log.payload[..5], b"hello");
        let test = region.pop_event().expect("self test");
        assert_eq!(test.value, 0x8000_0007);
        assert_eq!(region.stats().self_test, 0x8000_0007);
    }

    #[test]
    fn heartbeat_advances() {
        let region = region();
        region.heartbeat();
        let first = region.stats().heartbeat_seq;
        region.heartbeat();
        assert!(region.stats().heartbeat_seq > first);
        assert!(region.heartbeat_age_ms() < 1000);
    }

    #[test]
    fn status_transitions_are_visible() {
        let region = region();
        assert_eq!(region.status(), STATUS_EMPTY);
        region.mark_running();
        assert_eq!(region.status(), STATUS_RUNNING);
        assert_eq!(region.stats().status, STATUS_RUNNING);
    }

    #[test]
    fn attach_rejects_a_region_larger_than_the_cap() {
        let region = region();
        let fd = region.duplicate_fd().expect("dup");
        let err = Region::attach(fd, super::super::layout::MAX_REGION_BYTES + 1);
        match err.err() {
            Some(AttachError::TooLarge { size, max }) => {
                assert_eq!(size, super::super::layout::MAX_REGION_BYTES + 1);
                assert_eq!(max, super::super::layout::MAX_REGION_BYTES);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        // SAFETY: we own the duplicated descriptor.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn attach_rejects_foreign_magic() {
        let region = region();
        let fd = region.duplicate_fd().expect("dup");
        // SAFETY: the mapping is ours and the header is writable.
        unsafe {
            let header = region.header() as *const RoadHeader as *mut RoadHeader;
            (*header).magic = 0;
        }
        let err = Region::attach(fd, region.layout().total_size);
        assert_eq!(err.err(), Some(AttachError::BadMagic(0)));
        // SAFETY: we own the duplicated descriptor.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn attach_reads_peer_frames() {
        let producer = region();
        let fd = producer.duplicate_fd().expect("dup");
        let consumer = Region::attach(fd, producer.layout().total_size).expect("attach");
        let writer = producer.begin_frame().unwrap();
        // SAFETY: the slot is exclusively owned by the producer until published.
        unsafe { core::ptr::write_bytes(writer.ptr, 0xAB, 16) };
        producer.publish_frame(writer.slot);
        let view = consumer.acquire_frame().expect("frame");
        // SAFETY: the slot is claimed by this consumer.
        assert_eq!(unsafe { *view.ptr }, 0xAB);
        consumer.release_frame(view.slot);
        // SAFETY: we own the duplicated descriptor.
        unsafe { libc::close(fd) };
    }
}
