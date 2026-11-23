//! Road Loader companion core.
//!
//! This crate is compiled twice:
//! * `armeabi-v7a` for the 32-bit companion process (`:road32`), where it owns
//!   the render/input/heartbeat loop, the Rust math, the greedy mesher, the
//!   collision solver and the audio bridge;
//! * `arm64-v8a` for the 64-bit UI, where only the host-side IPC helpers are
//!   used.
//!
//! Everything the C++ JNI bridge needs is exported with the C ABI at the bottom
//! of this file.

#![allow(clippy::missing_safety_doc)]

mod alloc;
mod assets;
mod audio;
mod crash;
mod entities;
mod ipc;
mod math;
mod mesh;
mod physics;
mod render;
mod runtime;
mod shader;
mod util;

use core::alloc::Layout;
use core::ffi::{c_char, c_void};
use core::ptr;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::sync::{Arc, Mutex, OnceLock};

use ipc::layout::{
    AudioBlock, Command, Event, InputEvent, RoadHeader, AUDIO_BLOCK_SIZE, DEFAULT_HEIGHT,
    DEFAULT_WIDTH,
};
use ipc::region::{Region, RoadStats};

#[global_allocator]
static GLOBAL: alloc::RoadAllocator = alloc::RoadAllocator;

/// Version encoded as `major << 16 | minor`.
const CORE_VERSION: u32 = (2 << 16) | 1;

/// Frame descriptor handed to the presenter.
#[repr(C)]
pub struct RoadFrameInfo {
    pub ptr: *mut u8,
    pub len: usize,
    pub seq: u64,
    pub slot: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub published_ns: u64,
}

impl Default for RoadFrameInfo {
    fn default() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
            seq: 0,
            slot: 0,
            width: 0,
            height: 0,
            stride: 0,
            published_ns: 0,
        }
    }
}

/// Opaque host-side session (one shared region owned by the 64-bit UI).
pub struct HostSession {
    region: Region,
}

static COMPANION: OnceLock<Mutex<Option<Arc<Region>>>> = OnceLock::new();
static PATCHES: OnceLock<Mutex<BTreeMap<String, usize>>> = OnceLock::new();
static INPUT_SINK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// True when the game hooks own the input ring instead of the demo loop.
pub fn input_sink_enabled() -> bool {
    INPUT_SINK.load(core::sync::atomic::Ordering::Acquire)
}

fn companion_slot() -> &'static Mutex<Option<Arc<Region>>> {
    COMPANION.get_or_init(|| Mutex::new(None))
}

fn patches() -> &'static Mutex<BTreeMap<String, usize>> {
    PATCHES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

// ---------------------------------------------------------------------------
// Host side (64-bit UI process)
// ---------------------------------------------------------------------------

/// Creates a shared region of `width x height` RGBA frames.
#[no_mangle]
pub extern "C" fn road_host_create(width: u32, height: u32) -> *mut HostSession {
    let width = if width == 0 { DEFAULT_WIDTH } else { width };
    let height = if height == 0 { DEFAULT_HEIGHT } else { height };
    match Region::create(width, height) {
        Ok(region) => Box::into_raw(Box::new(HostSession { region })),
        Err(_) => ptr::null_mut(),
    }
}

/// Duplicates the region's file descriptor. The caller owns the returned fd.
/// Duplicates the region descriptor so the UI can hand it to another process
/// through Binder without losing its own copy.
#[no_mangle]
pub extern "C" fn road_host_dup_fd(session: *mut HostSession) -> i32 {
    if session.is_null() {
        return -1;
    }
    // SAFETY: `session` was created by `road_host_create`.
    let session = unsafe { &*session };
    match session.region.duplicate_fd() {
        Some(fd) => fd,
        None => -1,
    }
}

#[no_mangle]
pub extern "C" fn road_host_fd(session: *mut HostSession) -> i32 {
    if session.is_null() {
        return -1;
    }
    // SAFETY: `session` was created by `road_host_create`.
    let session = unsafe { &*session };
    session.region.duplicate_fd().unwrap_or(-1)
}

/// Size of the mapping that must be passed to the companion.
#[no_mangle]
pub extern "C" fn road_host_region_size(session: *mut HostSession) -> u64 {
    if session.is_null() {
        return 0;
    }
    // SAFETY: `session` was created by `road_host_create`.
    let session = unsafe { &*session };
    session.region.layout().total_size as u64
}

/// Acquires the newest frame. Returns 1 on success, 0 when no frame is ready,
/// and -1 on invalid arguments.
#[no_mangle]
pub extern "C" fn road_host_poll_frame(session: *mut HostSession, out: *mut RoadFrameInfo) -> i32 {
    if session.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: both pointers are non-null and owned by the caller.
    let session = unsafe { &*session };
    let out = unsafe { &mut *out };
    match session.region.acquire_frame() {
        Some(view) => {
            *out = RoadFrameInfo {
                ptr: view.ptr as *mut u8,
                len: view.len,
                seq: view.seq,
                slot: view.slot as u32,
                width: view.width,
                height: view.height,
                stride: view.stride,
                published_ns: view.published_ns,
            };
            1
        }
        None => {
            *out = RoadFrameInfo::default();
            0
        }
    }
}

/// Returns the pixel pointer of a frame slot without changing its state.
///
/// The pointer stays valid for the lifetime of the session; the UI uses it to
/// wrap each slot in a direct `ByteBuffer` exactly once.
#[no_mangle]
pub unsafe extern "C" fn road_host_frame_buffer(
    session: *mut HostSession,
    slot: u32,
    out_len: *mut usize,
) -> *mut u8 {
    if session.is_null() || slot as usize >= ipc::layout::FRAME_SLOT_COUNT {
        return ptr::null_mut();
    }
    // SAFETY: `session` was created by `road_host_create`.
    let session = unsafe { &*session };
    let layout = session.region.layout();
    if !out_len.is_null() {
        // SAFETY: the caller provided a writable usize.
        unsafe { *out_len = layout.frame_bytes };
    }
    // SAFETY: the slot offset is inside the mapping.
    unsafe { session.region.base_ptr().add(layout.frame_slot_offset(slot as usize)) }
}

#[no_mangle]
pub extern "C" fn road_host_release_frame(session: *mut HostSession, slot: u32) {
    if session.is_null() {
        return;
    }
    // SAFETY: `session` was created by `road_host_create`.
    let session = unsafe { &*session };
    session.region.release_frame(slot as usize);
}

#[no_mangle]
pub extern "C" fn road_host_push_input(session: *mut HostSession, event: *const InputEvent) -> i32 {
    if session.is_null() || event.is_null() {
        return -1;
    }
    // SAFETY: `session` is live and `event` points at a valid InputEvent.
    let session = unsafe { &*session };
    let event = unsafe { *event };
    i32::from(!session.region.push_input(event))
}

#[no_mangle]
pub extern "C" fn road_host_push_command(session: *mut HostSession, command: *const Command) -> i32 {
    if session.is_null() || command.is_null() {
        return -1;
    }
    // SAFETY: `session` is live and `command` points at a valid Command.
    let session = unsafe { &*session };
    let command = unsafe { *command };
    i32::from(!session.region.push_command(command))
}

#[no_mangle]
pub extern "C" fn road_host_pop_event(session: *mut HostSession, out: *mut Event) -> i32 {
    if session.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: both pointers are valid for writes.
    let session = unsafe { &*session };
    let out = unsafe { &mut *out };
    match session.region.pop_event() {
        Some(event) => {
            *out = event;
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn road_host_pop_audio(session: *mut HostSession, out: *mut AudioBlock) -> i32 {
    if session.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: both pointers are valid for writes.
    let session = unsafe { &*session };
    let out = unsafe { &mut *out };
    match session.region.pop_audio() {
        Some(block) => {
            *out = block;
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn road_host_stats(session: *mut HostSession, out: *mut RoadStats) -> i32 {
    if session.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: both pointers are valid for writes.
    let session = unsafe { &*session };
    let out = unsafe { &mut *out };
    *out = session.region.stats();
    0
}

/// Returns the size of `RoadStats` so the C++ mirror can be verified at runtime.
#[no_mangle]
pub extern "C" fn road_stats_size() -> u32 {
    core::mem::size_of::<RoadStats>() as u32
}

/// Returns the size of `RoadFrameInfo` for the same reason.
#[no_mangle]
pub extern "C" fn road_frame_info_size() -> u32 {
    core::mem::size_of::<RoadFrameInfo>() as u32
}

/// Returns the size of `RoadHeader` so the C++ side can sanity-check the ABI.
#[no_mangle]
pub extern "C" fn road_header_size() -> u32 {
    core::mem::size_of::<RoadHeader>() as u32
}

#[no_mangle]
pub extern "C" fn road_input_event_size() -> u32 {
    core::mem::size_of::<InputEvent>() as u32
}

#[no_mangle]
pub extern "C" fn road_command_size() -> u32 {
    core::mem::size_of::<Command>() as u32
}

#[no_mangle]
pub extern "C" fn road_event_size() -> u32 {
    core::mem::size_of::<Event>() as u32
}

#[no_mangle]
pub extern "C" fn road_audio_block_size() -> u32 {
    AUDIO_BLOCK_SIZE as u32
}

#[no_mangle]
pub extern "C" fn road_host_destroy(session: *mut HostSession) {
    if session.is_null() {
        return;
    }
    // SAFETY: `session` was created by `road_host_create` and is freed once.
    unsafe { drop(Box::from_raw(session)) };
}

// ---------------------------------------------------------------------------
// Companion side (32-bit process)
// ---------------------------------------------------------------------------

/// Attaches the companion to a shared region received from the UI.
///
/// Returns 0 on success, -1 on a bad descriptor/region, -2 when the region
/// belongs to a different ABI and -3 when the loop cannot be started.
#[no_mangle]
pub extern "C" fn road_companion_attach(
    fd: i32,
    size: usize,
    flags: u32,
    heap_limit: u64,
) -> i32 {
    let limit = if heap_limit == 0 { alloc::DEFAULT_HEAP_LIMIT as u64 } else { heap_limit };
    alloc::set_limit(limit as usize);
    let region = match Region::attach(fd, size) {
        Ok(region) => Arc::new(region),
        Err(ipc::region::AttachError::BadAbi { .. }) => return -2,
        Err(_) => return -1,
    };
    region.set_pid(std::process::id());
    region.set_status(ipc::layout::STATUS_INITIALIZING);
    {
        let mut slot = match companion_slot().lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_some() {
            return -3;
        }
        *slot = Some(Arc::clone(&region));
    }
    match runtime::companion::start(Arc::clone(&region), flags) {
        Ok(()) => 0,
        Err(_) => {
            let mut slot = match companion_slot().lock() {
                Ok(slot) => slot,
                Err(poisoned) => poisoned.into_inner(),
            };
            *slot = None;
            -3
        }
    }
}

/// Stops the companion loop. Returns 1 when a loop was running.
#[no_mangle]
pub extern "C" fn road_companion_stop() -> i32 {
    let stopped = runtime::companion::stop();
    let mut slot = match companion_slot().lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    *slot = None;
    i32::from(stopped)
}

#[no_mangle]
pub extern "C" fn road_companion_is_running() -> i32 {
    i32::from(runtime::companion::is_running())
}

#[no_mangle]
pub extern "C" fn road_companion_self_test() -> u32 {
    runtime::companion::self_test_mask()
}

/// Hands the input ring to the game hooks (`enabled != 0`) or back to the demo
/// loop. Called when `ScreenView::setup` is hooked.
#[no_mangle]
pub extern "C" fn road_companion_set_input_sink(enabled: i32) {
    INPUT_SINK.store(enabled != 0, core::sync::atomic::Ordering::Release);
}

/// Pops one input event for the game hook. Returns 1 on success, 0 when empty
/// and -1 when the companion is not attached.
#[no_mangle]
pub unsafe extern "C" fn road_companion_pop_input(out: *mut InputEvent) -> i32 {
    if out.is_null() {
        return -1;
    }
    let slot = match companion_slot().lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(region) = slot.as_ref() else {
        return -1;
    };
    match region.pop_input() {
        Some(event) => {
            // SAFETY: the caller provided a writable InputEvent.
            unsafe { *out = event };
            1
        }
        None => 0,
    }
}

/// Copies an RGBA frame captured by the `swapBuffers` hook into the shared
/// triple buffer. `stride` is in bytes. Returns 0 on success.
#[no_mangle]
pub unsafe extern "C" fn road_companion_publish_rgba(
    pixels: *const u8,
    width: u32,
    height: u32,
    stride: u32,
) -> i32 {
    if pixels.is_null() || width == 0 || height == 0 || stride < width * 4 {
        return -1;
    }
    let slot = match companion_slot().lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(region) = slot.as_ref() else {
        return -1;
    };
    let Some(writer) = region.begin_frame() else {
        return -2;
    };
    let copy_width = width.min(writer.width) as usize;
    let copy_height = height.min(writer.height) as usize;
    let source_row = stride as usize;
    let destination_row = writer.stride as usize;
    // SAFETY: both buffers are valid for their documented sizes; the copy is
    // clamped to the smaller frame on both axes.
    unsafe {
        for row in 0..copy_height {
            core::ptr::copy_nonoverlapping(
                pixels.add(row * source_row),
                writer.ptr.add(row * destination_row),
                copy_width * 4,
            );
        }
    }
    region.publish_frame(writer.slot);
    0
}

#[no_mangle]
pub extern "C" fn road_core_version() -> u32 {
    CORE_VERSION
}

#[no_mangle]
pub extern "C" fn road_abi_version() -> u32 {
    ipc::layout::ROAD_ABI_VERSION
}

/// Sets the allocator soft limit in bytes (0 restores the default).
#[no_mangle]
pub extern "C" fn road_set_heap_limit(bytes: u64) {
    let limit = if bytes == 0 { alloc::DEFAULT_HEAP_LIMIT as u64 } else { bytes };
    alloc::set_limit(limit as usize);
}

/// Size of [`alloc::AllocStats`], so C++ can assert it mirrors the struct.
#[no_mangle]
pub extern "C" fn road_alloc_stats_size() -> u32 {
    core::mem::size_of::<alloc::AllocStats>() as u32
}

#[no_mangle]
pub extern "C" fn road_alloc_stats(out: *mut alloc::AllocStats) -> i32 {
    if out.is_null() {
        return -1;
    }
    // SAFETY: caller provides a writable AllocStats.
    unsafe { *out = alloc::stats() };
    0
}

// ---------------------------------------------------------------------------
// Allocator shims for C++ (operator new / delete)
// ---------------------------------------------------------------------------

const ALLOC_HEADER: usize = 32;

/// Allocates `size` bytes aligned to `align` from the shared mimalloc heap.
///
/// The returned pointer carries a 32-byte header so `road_free` can rebuild the
/// exact `Layout` without relying on sized delete.
#[no_mangle]
pub unsafe extern "C" fn road_alloc(size: usize, align: usize) -> *mut c_void {
    let align = align.max(8).next_power_of_two();
    let total = match size.checked_add(ALLOC_HEADER).and_then(|value| value.checked_add(align)) {
        Some(total) => total,
        None => return ptr::null_mut(),
    };
    let Ok(layout) = Layout::from_size_align(total, align) else {
        return ptr::null_mut();
    };
    // SAFETY: `layout` has a non-zero size.
    let raw = unsafe { std::alloc::alloc(layout) };
    if raw.is_null() {
        return ptr::null_mut();
    }
    let start = raw as usize + ALLOC_HEADER;
    let aligned = (start + align - 1) & !(align - 1);
    let result = aligned as *mut u8;
    // SAFETY: `aligned - raw >= ALLOC_HEADER` and the header lies inside the
    // allocation.
    unsafe {
        (result.sub(32) as *mut usize).write(total);
        (result.sub(24) as *mut usize).write(align);
        (result.sub(16) as *mut usize).write(aligned - raw as usize);
        (result.sub(8) as *mut usize).write(size);
    }
    result as *mut c_void
}

/// Allocates zeroed memory.
#[no_mangle]
pub unsafe extern "C" fn road_calloc(size: usize, align: usize) -> *mut c_void {
    // SAFETY: delegated to `road_alloc`, which returns a valid allocation.
    let result = unsafe { road_alloc(size, align) };
    if !result.is_null() {
        // SAFETY: `result` points at `size` writable bytes.
        unsafe { ptr::write_bytes(result as *mut u8, 0, size) };
    }
    result
}

/// Frees a pointer returned by [`road_alloc`].
#[no_mangle]
pub unsafe extern "C" fn road_free(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    let result = pointer as *mut u8;
    // SAFETY: the header was written by `road_alloc` immediately before the
    // returned pointer.
    unsafe {
        let total = (result.sub(32) as *const usize).read();
        let align = (result.sub(24) as *const usize).read();
        let offset = (result.sub(16) as *const usize).read();
        let raw = result.sub(offset);
        let layout = Layout::from_size_align_unchecked(total, align);
        std::alloc::dealloc(raw, layout);
    }
}

/// Resizes an allocation, preserving `min(old_size, new_size)` bytes.
#[no_mangle]
pub unsafe extern "C" fn road_realloc(pointer: *mut c_void, size: usize, align: usize) -> *mut c_void {
    if pointer.is_null() {
        // SAFETY: forwarded to `road_alloc`.
        return unsafe { road_alloc(size, align) };
    }
    // SAFETY: `pointer` came from `road_alloc`.
    let old_size = unsafe { (pointer as *mut u8).sub(8).cast::<usize>().read() };
    // SAFETY: same contract as above.
    let new_pointer = unsafe { road_alloc(size, align) };
    if new_pointer.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: both allocations are valid for `copy` bytes.
    unsafe {
        ptr::copy_nonoverlapping(pointer as *const u8, new_pointer as *mut u8, old_size.min(size));
        road_free(pointer);
    }
    new_pointer
}

#[no_mangle]
pub unsafe extern "C" fn road_alloc_usable_size(pointer: *mut c_void) -> usize {
    if pointer.is_null() {
        return 0;
    }
    // SAFETY: `pointer` came from `road_alloc`, whose header stores the size.
    unsafe { (pointer as *mut u8).sub(8).cast::<usize>().read() }
}

// ---------------------------------------------------------------------------
// Math / physics / mesh / shader / audio C-ABI (game trampolines)
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn rust_math_distance(
    x1: f32,
    y1: f32,
    z1: f32,
    x2: f32,
    y2: f32,
    z2: f32,
) -> f32 {
    math::distance(glam::Vec3::new(x1, y1, z1), glam::Vec3::new(x2, y2, z2))
}

#[no_mangle]
pub extern "C" fn rust_vec3_length(x: f32, y: f32, z: f32) -> f32 {
    math::length(glam::Vec3::new(x, y, z))
}

/// `min1`/`max1`/`min2`/`max2` point at three `f32` values each.
#[no_mangle]
pub unsafe extern "C" fn rust_aabb_intersects(
    min1: *const f32,
    max1: *const f32,
    min2: *const f32,
    max2: *const f32,
) -> i32 {
    if min1.is_null() || max1.is_null() || min2.is_null() || max2.is_null() {
        return 0;
    }
    // SAFETY: each pointer addresses three readable f32 values.
    let read = |base: *const f32| unsafe {
        glam::Vec3::new(*base, *base.add(1), *base.add(2))
    };
    let a = math::Aabb::new(read(min1), read(max1));
    let b = math::Aabb::new(read(min2), read(max2));
    i32::from(a.intersects(&b))
}

/// Transforms a point by a column-major 4x4 matrix.
#[no_mangle]
pub unsafe extern "C" fn rust_transform_point(matrix: *const f32, point: *const f32, out: *mut f32) -> i32 {
    if matrix.is_null() || point.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: the caller provides 16 matrix floats, 3 point floats and 3 output
    // floats.
    unsafe {
        let mut m = [0.0f32; 16];
        for (index, slot) in m.iter_mut().enumerate() {
            *slot = *matrix.add(index);
        }
        let matrix = glam::Mat4::from_cols_array(&m);
        let p = glam::Vec3::new(*point, *point.add(1), *point.add(2));
        let result = math::transform_point(&matrix, p);
        *out = result.x;
        *out.add(1) = result.y;
        *out.add(2) = result.z;
    }
    0
}

/// Moves an actor with the Rust collision solver.
///
/// `solids` is a flat array of `solids_len` boxes, six `f32` per box
/// (`min.xyz`, `max.xyz`). `out_position` receives three floats, `out_flags`
/// receives bit 0 = on_ground, 1 = hit_x, 2 = hit_y, 3 = hit_z.
#[no_mangle]
pub unsafe extern "C" fn rust_actor_move(
    start: *const f32,
    half_extents: *const f32,
    velocity: *const f32,
    dt: f32,
    solids: *const f32,
    solids_len: u32,
    out_position: *mut f32,
    out_flags: *mut u32,
) -> i32 {
    if start.is_null() || half_extents.is_null() || velocity.is_null() || out_position.is_null() {
        return -1;
    }
    if solids_len > 0 && solids.is_null() {
        return -1;
    }
    // SAFETY: all pointers are non-null and sized by the caller.
    let (start, half, velocity, boxes) = unsafe {
        let read3 = |base: *const f32| glam::Vec3::new(*base, *base.add(1), *base.add(2));
        let mut boxes = Vec::with_capacity(solids_len as usize);
        for index in 0..solids_len as usize {
            let base = solids.add(index * 6);
            boxes.push(math::Aabb::new(read3(base), read3(base.add(3))));
        }
        (read3(start), read3(half_extents), read3(velocity), boxes)
    };
    let result = physics::sweep(start, half, velocity, &boxes, dt);
    // SAFETY: `out_position` addresses three writable floats.
    unsafe {
        *out_position = result.position.x;
        *out_position.add(1) = result.position.y;
        *out_position.add(2) = result.position.z;
        if !out_flags.is_null() {
            let mut flags = 0u32;
            flags |= u32::from(result.on_ground);
            flags |= u32::from(result.hit_x) << 1;
            flags |= u32::from(result.hit_y) << 2;
            flags |= u32::from(result.hit_z) << 3;
            *out_flags = flags;
        }
    }
    0
}

/// Builds a greedy mesh. Returns the vertex count, or a negative error code:
/// -1 invalid arguments, -2 bad dimensions, -3 short block data, -4 out of
/// vertex space.
#[no_mangle]
pub unsafe extern "C" fn rust_mesh_build(
    dims: *const u32,
    blocks: *const u8,
    opaque: *const u8,
    origin: *const f32,
    scale: f32,
    out: *mut mesh::MeshVertex,
    out_capacity: u32,
    use_simd: u32,
) -> i32 {
    if dims.is_null() || blocks.is_null() || opaque.is_null() || out.is_null() {
        return -1;
    }
    // SAFETY: `dims` addresses three u32 values and `opaque` a 256-byte table.
    let dims_array = unsafe { [*dims, *dims.add(1), *dims.add(2)] };
    let mut opaque_table = [0u8; 256];
    // SAFETY: the caller provides a full opacity table.
    unsafe { ptr::copy_nonoverlapping(opaque, opaque_table.as_mut_ptr(), 256) };
    let count = (dims_array[0] as usize) * (dims_array[1] as usize) * (dims_array[2] as usize);
    if count == 0 || count > 64 * 64 * 64 {
        return -2;
    }
    // SAFETY: the caller guarantees `blocks` addresses `count` bytes.
    let blocks_slice = unsafe { core::slice::from_raw_parts(blocks, count) };
    // SAFETY: the caller guarantees `out` addresses `out_capacity` vertices.
    let out_slice = unsafe { core::slice::from_raw_parts_mut(out, out_capacity as usize) };
    let mut origin_array = [0.0f32; 3];
    if !origin.is_null() {
        // SAFETY: the caller provides three floats when `origin` is non-null.
        unsafe {
            origin_array = [*origin, *origin.add(1), *origin.add(2)];
        }
    }
    let input = mesh::MeshInput {
        dims: dims_array,
        blocks: blocks_slice,
        opaque: &opaque_table,
        origin: origin_array,
        scale: if scale > 0.0 { scale } else { 1.0 },
        use_simd: use_simd != 0,
    };
    match mesh::build(&input, out_slice) {
        Ok(written) => written as i32,
        Err(mesh::MeshError::BadDimensions) => -2,
        Err(mesh::MeshError::ShortBlockData) => -3,
        Err(mesh::MeshError::TooManyVertices) => -4,
    }
}

/// Transpiles a shader into `out` (NUL-terminated). Returns the required size in
/// bytes including the terminator, or a negative error code.
#[no_mangle]
pub unsafe extern "C" fn rust_shader_transpile(
    source: *const c_char,
    stage: u32,
    out: *mut c_char,
    out_len: u32,
) -> i32 {
    if source.is_null() {
        return -1;
    }
    // SAFETY: `source` is a NUL-terminated C string.
    let source = unsafe { CStr::from_ptr(source) };
    let Ok(source) = source.to_str() else {
        return -1;
    };
    let stage = if stage == 0 { shader::ShaderStage::Vertex } else { shader::ShaderStage::Fragment };
    let transpiled = match shader::transpile_java_glsl(source, stage) {
        Ok(text) => text,
        Err(shader::ShaderError::Empty) => return -2,
        Err(shader::ShaderError::Unsupported(_)) => return -3,
        Err(shader::ShaderError::NoMain) => return -4,
    };
    let needed = transpiled.len() + 1;
    if out.is_null() || out_len as usize >= needed {
        if !out.is_null() {
            // SAFETY: the caller provided `out_len` bytes and `needed <= out_len`.
            unsafe {
                ptr::copy_nonoverlapping(transpiled.as_ptr(), out as *mut u8, transpiled.len());
                *out.add(transpiled.len()) = 0;
            }
        }
        return needed as i32;
    }
    // Report the size needed and write a truncated, terminated copy.
    // SAFETY: `out_len > 0`, so writing the terminator is in bounds.
    unsafe {
        let copy = transpiled.len().min(out_len as usize - 1);
        ptr::copy_nonoverlapping(transpiled.as_ptr(), out as *mut u8, copy);
        *out.add(copy) = 0;
    }
    -(needed as i32)
}

/// Converts interleaved i16 PCM to f32. Returns the number of samples written.
#[no_mangle]
pub unsafe extern "C" fn rust_audio_convert_i16(
    source: *const i16,
    sample_count: u32,
    destination: *mut f32,
) -> u32 {
    if source.is_null() || destination.is_null() {
        return 0;
    }
    // SAFETY: the caller provides `sample_count` i16 inputs and f32 outputs.
    let input = unsafe { core::slice::from_raw_parts(source, sample_count as usize) };
    let output = unsafe { core::slice::from_raw_parts_mut(destination, sample_count as usize) };
    audio::convert_i16_to_f32(input, output) as u32
}

/// CRC-32 of a byte range, used by the asset chunker.
#[no_mangle]
pub unsafe extern "C" fn rust_crc32(data: *const u8, len: usize) -> u32 {
    if data.is_null() {
        return 0;
    }
    // SAFETY: the caller provides `len` readable bytes.
    let slice = unsafe { core::slice::from_raw_parts(data, len) };
    util::crc32(slice)
}

/// Number of chunks needed to stream `len` bytes.
#[no_mangle]
pub extern "C" fn rust_asset_chunk_count(len: usize) -> u32 {
    if len == 0 {
        0
    } else {
        len.div_ceil(assets::ASSET_CHUNK_DATA) as u32
    }
}

// ---------------------------------------------------------------------------
// Patch registry (signature-scanned offsets)
// ---------------------------------------------------------------------------

fn patch_name(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    // SAFETY: `name` is a NUL-terminated C string.
    let name = unsafe { CStr::from_ptr(name) };
    Some(name.to_string_lossy().into_owned())
}

/// Records a resolved game offset under `name`. Returns 0 on success.
#[no_mangle]
pub extern "C" fn road_patch_offset(name: *const c_char, address: usize) -> i32 {
    let Some(name) = patch_name(name) else {
        return -1;
    };
    if address == 0 {
        return -2;
    }
    let mut patches = match patches().lock() {
        Ok(patches) => patches,
        Err(poisoned) => poisoned.into_inner(),
    };
    patches.insert(name, address);
    0
}

/// Looks a patch up; returns 0 when it is unknown.
#[no_mangle]
pub extern "C" fn road_patch_lookup(name: *const c_char) -> usize {
    let Some(name) = patch_name(name) else {
        return 0;
    };
    let patches = match patches().lock() {
        Ok(patches) => patches,
        Err(poisoned) => poisoned.into_inner(),
    };
    patches.get(&name).copied().unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn road_patch_count() -> u32 {
    let patches = match patches().lock() {
        Ok(patches) => patches,
        Err(poisoned) => poisoned.into_inner(),
    };
    patches.len() as u32
}

#[no_mangle]
pub extern "C" fn road_patch_clear() {
    let mut patches = match patches().lock() {
        Ok(patches) => patches,
        Err(poisoned) => poisoned.into_inner(),
    };
    patches.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_session_round_trip() {
        let session = road_host_create(64, 48);
        assert!(!session.is_null());
        assert!(road_host_region_size(session) > 0);
        let fd = road_host_fd(session);
        assert!(fd >= 0);
        let mut info = RoadFrameInfo::default();
        assert_eq!(road_host_poll_frame(session, &mut info), 0);
        let mut stats = RoadStats::default();
        assert_eq!(road_host_stats(session, &mut stats), 0);
        assert_eq!(stats.width, 64);
        // SAFETY: we own the duplicated fd.
        unsafe { libc::close(fd) };
        road_host_destroy(session);
    }

    #[test]
    fn allocator_shims_align_and_track() {
        // SAFETY: exercises the C-ABI allocator with valid sizes.
        unsafe {
            let pointer = road_alloc(1000, 64);
            assert!(!pointer.is_null());
            assert_eq!(pointer as usize % 64, 0);
            assert!(road_alloc_usable_size(pointer) >= 1000);
            ptr::write_bytes(pointer as *mut u8, 0xAB, 1000);
            let resized = road_realloc(pointer, 4000, 64);
            assert!(!resized.is_null());
            assert_eq!(*(resized as *const u8), 0xAB);
            assert!(road_alloc_usable_size(resized) >= 4000);
            road_free(resized);
            let zeroed = road_calloc(128, 16);
            assert!(!zeroed.is_null());
            assert_eq!(*(zeroed as *const u8), 0);
            road_free(zeroed);
        }
    }

    #[test]
    fn math_c_abi_matches_rust() {
        assert_eq!(rust_math_distance(0.0, 0.0, 0.0, 3.0, 4.0, 0.0), 5.0);
        assert_eq!(rust_vec3_length(3.0, 4.0, 0.0), 5.0);
        let min1 = [0.0f32, 0.0, 0.0];
        let max1 = [1.0f32, 1.0, 1.0];
        let min2 = [0.5f32, 0.5, 0.5];
        let max2 = [2.0f32, 2.0, 2.0];
        // SAFETY: the arrays are valid and live for the call.
        assert_eq!(unsafe { rust_aabb_intersects(min1.as_ptr(), max1.as_ptr(), min2.as_ptr(), max2.as_ptr()) }, 1);
        // SAFETY: null pointers are explicitly rejected.
        assert_eq!(unsafe { rust_aabb_intersects(ptr::null(), max1.as_ptr(), min2.as_ptr(), max2.as_ptr()) }, 0);
    }

    #[test]
    fn actor_move_c_abi_reports_ground() {
        let start = [0.0f32, 3.0, 0.0];
        let half = [0.3f32, 0.9, 0.3];
        let velocity = [0.0f32, -10.0, 0.0];
        let solids = [-4.0f32, -1.0, -4.0, 4.0, 0.0, 4.0];
        let mut out = [0.0f32; 3];
        let mut flags = 0u32;
        // SAFETY: all pointers are valid for the documented lengths.
        let rc = unsafe {
            rust_actor_move(
                start.as_ptr(),
                half.as_ptr(),
                velocity.as_ptr(),
                0.5,
                solids.as_ptr(),
                1,
                out.as_mut_ptr(),
                &mut flags,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(flags & 1, 1);
        assert!(out[1] < 1.0);
    }

    #[test]
    fn mesh_c_abi_returns_vertex_count() {
        let dims = [16u32, 16, 16];
        let mut blocks = vec![0u8; 16 * 16 * 16];
        blocks[8 + 16 * (8 + 16 * 8)] = 1;
        let mut opaque = [0u8; 256];
        for entry in opaque.iter_mut().skip(1) {
            *entry = 1;
        }
        let mut out = vec![mesh::MeshVertex::default(); 1024];
        // SAFETY: buffers match the documented sizes.
        let count = unsafe {
            rust_mesh_build(
                dims.as_ptr(),
                blocks.as_ptr(),
                opaque.as_ptr(),
                ptr::null(),
                1.0,
                out.as_mut_ptr(),
                out.len() as u32,
                1,
            )
        };
        assert_eq!(count, 36);
        // SAFETY: null dims are rejected.
        assert_eq!(
            unsafe { rust_mesh_build(ptr::null(), blocks.as_ptr(), opaque.as_ptr(), ptr::null(), 1.0, out.as_mut_ptr(), 1024, 1) },
            -1
        );
    }

    #[test]
    fn shader_c_abi_reports_size_and_truncation() {
        let source = std::ffi::CString::new("void main() { gl_FragColor = vec4(1.0); }").unwrap();
        let mut buffer = [0i8; 256];
        // SAFETY: the source is NUL-terminated and the buffer is writable.
        let needed = unsafe {
            rust_shader_transpile(source.as_ptr(), 1, buffer.as_mut_ptr(), buffer.len() as u32)
        };
        assert!(needed > 0);
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap();
        assert!(text.contains("pc_fragColor"));
        let mut small = [0i8; 8];
        // SAFETY: a deliberately small buffer reports the required size.
        let needed = unsafe { rust_shader_transpile(source.as_ptr(), 1, small.as_mut_ptr(), 8) };
        assert!(needed < 0);
    }

    #[test]
    fn patch_registry_round_trips() {
        let name = std::ffi::CString::new("Level::tick").unwrap();
        assert_eq!(road_patch_offset(name.as_ptr(), 0x1234), 0);
        assert_eq!(road_patch_lookup(name.as_ptr()), 0x1234);
        assert!(road_patch_count() >= 1);
        assert_eq!(road_patch_offset(name.as_ptr(), 0), -2);
        road_patch_clear();
        assert_eq!(road_patch_lookup(name.as_ptr()), 0);
    }

    #[test]
    fn abi_sizes_are_stable() {
        assert_eq!(road_input_event_size(), 64);
        assert_eq!(road_command_size(), 256);
        assert_eq!(road_event_size(), 256);
        assert_eq!(road_audio_block_size(), 4096);
        assert_eq!(road_stats_size() as usize, core::mem::size_of::<RoadStats>());
        assert_eq!(road_frame_info_size() as usize, core::mem::size_of::<RoadFrameInfo>());
    }

    #[test]
    fn version_is_encoded() {
        assert_eq!(road_core_version(), CORE_VERSION);
        assert_eq!(road_abi_version(), 1);
    }
}
