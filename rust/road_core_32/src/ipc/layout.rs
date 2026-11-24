//! Binary layout of the shared-memory region.
//!
//! The layout is fixed and mirrored by `road_loader_32/include/pl/ipc/Layout.h`
//! (for documentation and host tests). Only this Rust module interprets the
//! bytes; C++/Kotlin go through the C-ABI in `lib.rs`, so the two sides cannot
//! drift.

use core::sync::atomic::{AtomicU32, AtomicU64};

use crate::util::align_up;

/// "ROAD" in ASCII, little endian.
pub const ROAD_MAGIC: u32 = 0x524F_4144;
/// Bumped whenever the layout below changes incompatibly.
pub const ROAD_ABI_VERSION: u32 = 1;

pub const HEADER_REGION_SIZE: usize = 4096;
pub const FRAME_HEADER_REGION_SIZE: usize = 4096;
pub const FRAME_SLOT_COUNT: usize = 3;
pub const RING_HEADER_SIZE: usize = 64;

pub const SLOT_FREE: u32 = 0;
pub const SLOT_WRITING: u32 = 1;
pub const SLOT_READY: u32 = 2;
pub const SLOT_READING: u32 = 3;

/// Companion lifecycle, mirrored in `RoadStatus.kt`.
pub const STATUS_EMPTY: u32 = 0;
pub const STATUS_INITIALIZING: u32 = 1;
pub const STATUS_RUNNING: u32 = 2;
pub const STATUS_STOPPED: u32 = 3;
pub const STATUS_CRASHED: u32 = 4;
pub const STATUS_SAFE_MODE: u32 = 5;

pub const FLAG_SAFE_MODE: u32 = 1 << 0;
pub const FLAG_RENDER_TEST_SCENE: u32 = 1 << 1;
pub const FLAG_SELF_TEST: u32 = 1 << 2;
pub const FLAG_HEADLESS: u32 = 1 << 3;
pub const FLAG_SHADERS: u32 = 1 << 4;

pub const INPUT_KIND_DOWN: u32 = 1;
pub const INPUT_KIND_MOVE: u32 = 2;
pub const INPUT_KIND_UP: u32 = 3;
pub const INPUT_KIND_CANCEL: u32 = 4;
pub const INPUT_KIND_KEY: u32 = 5;
pub const INPUT_KIND_BUTTON: u32 = 6;
pub const INPUT_KIND_STICK: u32 = 7;

pub const EVENT_SELF_TEST: u32 = 1;
pub const EVENT_LOG: u32 = 2;
pub const EVENT_MESH_READY: u32 = 3;
pub const EVENT_ASSET_CHUNK: u32 = 4;
pub const EVENT_AUDIO_FORMAT: u32 = 5;

pub const CMD_SET_VOLUME: u32 = 1;
pub const CMD_PATCH_OFFSET: u32 = 2;
pub const CMD_SET_RENDER_DISTANCE: u32 = 3;
pub const CMD_REQUEST_SELF_TEST: u32 = 4;
pub const CMD_SHUTDOWN: u32 = 5;
pub const CMD_ASSET_CHUNK: u32 = 6;
pub const CMD_SET_SHADERS: u32 = 7;
pub const CMD_SET_SAFE_MODE: u32 = 8;

/// Default ring capacities. They are powers of two and fixed for the lifetime of
/// the region, which is what keeps the 32-bit process from growing unbounded.
pub const DEFAULT_AUDIO_CAPACITY: u32 = 64;
/// 32-byte header + 4064 bytes of PCM (~21 ms of 48 kHz stereo f32).
pub const AUDIO_BLOCK_SIZE: usize = 4096;
pub const AUDIO_DATA_SIZE: usize = AUDIO_BLOCK_SIZE - 32;
pub const DEFAULT_INPUT_CAPACITY: u32 = 1024;
pub const DEFAULT_COMMAND_CAPACITY: u32 = 256;
pub const DEFAULT_EVENT_CAPACITY: u32 = 256;
pub const DEFAULT_WIDTH: u32 = 960;
pub const DEFAULT_HEIGHT: u32 = 540;

/// One touch/controller sample. Exactly 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InputEvent {
    pub seq: u32,
    pub kind: u32,
    pub pointer_id: u32,
    pub action: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub size: f32,
    pub timestamp_ns: u64,
    pub button: u32,
    pub key_code: u32,
    pub reserved: [u32; 3],
}

/// One control message (host -> companion). Exactly 256 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Command {
    pub kind: u32,
    pub flags: u32,
    pub seq: u32,
    pub length: u32,
    pub value: u64,
    pub payload: [u8; 232],
}

impl Default for Command {
    fn default() -> Self {
        Self { kind: 0, flags: 0, seq: 0, length: 0, value: 0, payload: [0; 232] }
    }
}

/// One block of interleaved f32 PCM (companion -> host). Exactly 4096 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioBlock {
    pub seq: u32,
    pub frames: u32,
    pub channels: u32,
    pub sample_rate: u32,
    pub timestamp_ns: u64,
    pub length: u32,
    pub flags: u32,
    pub data: [u8; AUDIO_DATA_SIZE],
}

impl Default for AudioBlock {
    fn default() -> Self {
        Self {
            seq: 0,
            frames: 0,
            channels: 0,
            sample_rate: 0,
            timestamp_ns: 0,
            length: 0,
            flags: 0,
            data: [0; AUDIO_DATA_SIZE],
        }
    }
}

/// One notification (companion -> host). Exactly 256 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub kind: u32,
    pub flags: u32,
    pub seq: u32,
    pub length: u32,
    pub value: u64,
    pub payload: [u8; 232],
}

impl Default for Event {
    fn default() -> Self {
        Self { kind: 0, flags: 0, seq: 0, length: 0, value: 0, payload: [0; 232] }
    }
}

const _: () = assert!(core::mem::size_of::<InputEvent>() == 64);
const _: () = assert!(core::mem::size_of::<Command>() == 256);
const _: () = assert!(core::mem::size_of::<Event>() == 256);
const _: () = assert!(core::mem::size_of::<AudioBlock>() == AUDIO_BLOCK_SIZE);

/// Single-producer/single-consumer ring control block. Exactly 64 bytes.
#[repr(C, align(64))]
pub struct RingHeader {
    /// Consumer index (advanced by the reader).
    pub head: AtomicU32,
    /// Producer index (advanced by the writer).
    pub tail: AtomicU32,
    pub capacity: u32,
    pub mask: u32,
    pub dropped: AtomicU32,
    pub reserved: [u32; 11],
}

impl RingHeader {
    pub fn init(&self, capacity: u32) {
        self.head.store(0, core::sync::atomic::Ordering::Relaxed);
        self.tail.store(0, core::sync::atomic::Ordering::Relaxed);
        self.dropped.store(0, core::sync::atomic::Ordering::Relaxed);
        // The immutable fields are written once, before the ring is published to
        // the peer process; `init` only runs on the creating side.
        let this = self as *const Self as *mut Self;
        // SAFETY: see above; no other thread has access yet.
        unsafe {
            (*this).capacity = capacity;
            (*this).mask = capacity - 1;
        }
    }
}

/// Per-frame triple-buffer slot header. Exactly 64 bytes.
#[repr(C, align(64))]
pub struct FrameSlotHeader {
    pub state: AtomicU32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub seq: AtomicU64,
    pub published_ns: AtomicU64,
    pub reserved: [u32; 8],
}

impl FrameSlotHeader {
    pub fn init(&self, width: u32, height: u32, stride: u32) {
        self.state.store(SLOT_FREE, core::sync::atomic::Ordering::Relaxed);
        self.seq.store(0, core::sync::atomic::Ordering::Relaxed);
        self.published_ns.store(0, core::sync::atomic::Ordering::Relaxed);
        // Immutable after creation; written before the peer maps the region.
        let this = self as *const Self as *mut Self;
        // SAFETY: see above; no other thread has access yet.
        unsafe {
            (*this).width = width;
            (*this).height = height;
            (*this).stride = stride;
        }
    }
}

/// The region header. Lives at offset 0 of the shared mapping.
#[repr(C, align(64))]
pub struct RoadHeader {
    pub magic: u32,
    pub abi_version: u32,
    pub header_size: u32,
    pub status: AtomicU32,
    pub heartbeat_seq: AtomicU32,
    pub flags: AtomicU32,
    pub pid: AtomicU32,
    pub width: AtomicU32,
    pub height: AtomicU32,
    pub stride: AtomicU32,
    pub frame_count: AtomicU64,
    pub presented_count: AtomicU64,
    pub dropped_frames: AtomicU32,
    pub input_pushed: AtomicU32,
    pub input_consumed: AtomicU32,
    pub input_dropped: AtomicU32,
    pub cmd_pushed: AtomicU32,
    pub cmd_consumed: AtomicU32,
    pub cmd_dropped: AtomicU32,
    pub event_pushed: AtomicU32,
    pub event_consumed: AtomicU32,
    pub event_dropped: AtomicU32,
    pub audio_pushed: AtomicU32,
    pub audio_consumed: AtomicU32,
    pub audio_dropped: AtomicU32,
    pub heap_used: AtomicU64,
    pub heap_peak: AtomicU64,
    pub heap_limit: AtomicU64,
    pub heartbeat_ns: AtomicU64,
    pub last_input_x: AtomicU32,
    pub last_input_y: AtomicU32,
    pub last_input_kind: AtomicU32,
    pub last_input_seq: AtomicU32,
    pub render_us_avg: AtomicU32,
    pub render_us_max: AtomicU32,
    pub self_test: AtomicU32,
    pub crash_signal: AtomicU32,
    pub crash_code: AtomicU32,
    pub crash_addr: AtomicU64,
    pub crash_pc: AtomicU64,
    pub crash_lr: AtomicU64,
    pub crash_message: [u8; 160],
    pub frame_slots: [FrameSlotHeader; FRAME_SLOT_COUNT],
    pub input_ring: RingHeader,
    pub command_ring: RingHeader,
    pub event_ring: RingHeader,
    pub audio_ring: RingHeader,
    pub reserved: [u32; 32],
}

/// Byte offsets of every sub-region inside the mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionLayout {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub frame_bytes: usize,
    pub frames_offset: usize,
    pub input_offset: usize,
    pub input_capacity: u32,
    pub command_offset: usize,
    pub command_capacity: u32,
    pub event_offset: usize,
    pub event_capacity: u32,
    pub audio_offset: usize,
    pub audio_capacity: u32,
    pub total_size: usize,
}

impl RegionLayout {
    pub fn new(width: u32, height: u32) -> Self {
        let width = width.max(16);
        let height = height.max(16);
        let stride = width * 4;
        let frame_bytes = align_up((stride as usize) * (height as usize), 4096);
        let frames_offset = align_up(HEADER_REGION_SIZE + FRAME_HEADER_REGION_SIZE, 4096);
        let input_offset = frames_offset + frame_bytes * FRAME_SLOT_COUNT;
        let command_offset = input_offset + RING_HEADER_SIZE + DEFAULT_INPUT_CAPACITY as usize * 64;
        let event_offset =
            command_offset + RING_HEADER_SIZE + DEFAULT_COMMAND_CAPACITY as usize * 256;
        let audio_offset = event_offset + RING_HEADER_SIZE + DEFAULT_EVENT_CAPACITY as usize * 256;
        let total_size = align_up(
            audio_offset + RING_HEADER_SIZE + DEFAULT_AUDIO_CAPACITY as usize * AUDIO_BLOCK_SIZE,
            4096,
        );
        Self {
            width,
            height,
            stride,
            frame_bytes,
            frames_offset,
            input_offset,
            input_capacity: DEFAULT_INPUT_CAPACITY,
            command_offset,
            command_capacity: DEFAULT_COMMAND_CAPACITY,
            event_offset,
            event_capacity: DEFAULT_EVENT_CAPACITY,
            audio_offset,
            audio_capacity: DEFAULT_AUDIO_CAPACITY,
            total_size,
        }
    }

    pub fn frame_slot_offset(&self, slot: usize) -> usize {
        self.frames_offset + slot * self.frame_bytes
    }
}

/// Minimum mapping size for a given frame size, used to validate attachments.
pub fn required_size(width: u32, height: u32) -> usize {
    RegionLayout::new(width, height).total_size
}

/// Largest region a companion will map.
///
/// The region is supplied by the peer process, so the size is not trusted. A
/// 3840x2160 region is about 95 MiB (three 33 MiB slots plus the rings), which
/// is the largest resolution the layout accepts; anything past this would let a
/// caller make the 32-bit process reserve memory it can never use.
pub const MAX_REGION_BYTES: usize = 128 * 1024 * 1024;

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn the_cap_covers_the_largest_accepted_resolution() {
        let uhd = RegionLayout::new(3840, 2160);
        assert!(uhd.total_size <= MAX_REGION_BYTES, "size={}", uhd.total_size);
    }

    #[test]
    fn the_cap_is_below_a_32_bit_address_space() {
        assert!(MAX_REGION_BYTES < 512 * 1024 * 1024);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_header_is_one_cache_line() {
        assert_eq!(core::mem::size_of::<RingHeader>(), 64);
        assert_eq!(core::mem::align_of::<RingHeader>(), 64);
    }

    #[test]
    fn frame_slot_header_is_one_cache_line() {
        assert_eq!(core::mem::size_of::<FrameSlotHeader>(), 64);
    }

    #[test]
    fn header_fits_its_region() {
        assert!(core::mem::size_of::<RoadHeader>() <= HEADER_REGION_SIZE);
    }

    #[test]
    fn layout_is_64_byte_aligned_and_ordered() {
        let layout = RegionLayout::new(1280, 720);
        assert_eq!(layout.frames_offset % 64, 0);
        assert_eq!(layout.input_offset % 64, 0);
        assert_eq!(layout.command_offset % 64, 0);
        assert_eq!(layout.event_offset % 64, 0);
        assert_eq!(layout.total_size % 4096, 0);
        assert!(layout.frames_offset + layout.frame_bytes * FRAME_SLOT_COUNT <= layout.input_offset);
        assert!(layout.input_offset < layout.command_offset);
        assert!(layout.command_offset < layout.event_offset);
        assert!(layout.event_offset < layout.audio_offset);
        assert!(layout.audio_offset < layout.total_size);
    }

    #[test]
    fn layout_rejects_degenerate_dimensions_by_clamping() {
        let layout = RegionLayout::new(0, 0);
        assert_eq!(layout.width, 16);
        assert_eq!(layout.height, 16);
    }

    #[test]
    fn default_region_is_reasonable_for_32_bit() {
        let layout = RegionLayout::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
        // 3 slots of 960x540 RGBA plus the rings must stay below 16 MiB.
        assert!(layout.total_size < 16 * 1024 * 1024, "size={}", layout.total_size);
    }
}
