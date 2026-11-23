//! Global allocator for the companion process.
//!
//! The whole process (Rust *and* the C++ game code, through the C-ABI shims in
//! this module) allocates through mimalloc. On armeabi-v7a the address space is
//! small and long sessions fragment the default dlmalloc heap badly, so a single
//! allocator with per-thread arenas keeps the working set predictable.
//!
//! A hard soft-limit is enforced here as well: once `ROAD_HEAP_LIMIT` bytes are
//! live, new allocations fail instead of letting the kernel OOM-kill the
//! process (Phase 15).

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use mimalloc::MiMalloc;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIMIT_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);
static FAILED_ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// Default soft limit for the 32-bit process: 1.5 GiB, the point at which the
/// UI starts unloading distant chunks (Phase 15.2).
pub const DEFAULT_HEAP_LIMIT: usize = 1536 * 1024 * 1024;

fn record_alloc(size: usize) {
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

fn record_free(size: usize) {
    LIVE_BYTES.fetch_sub(size, Ordering::Relaxed);
}

fn over_limit(size: usize) -> bool {
    exceeds_limit(
        LIVE_BYTES.load(Ordering::Relaxed),
        size,
        LIMIT_BYTES.load(Ordering::Relaxed),
    )
}

fn exceeds_limit(live: usize, size: usize, limit: usize) -> bool {
    if limit == usize::MAX {
        return false;
    }
    live.saturating_add(size) > limit
}

/// mimalloc-backed allocator that accounts every byte it hands out.
pub struct RoadAllocator;

// SAFETY: every method forwards to mimalloc, which satisfies the `GlobalAlloc`
// contract, and the accounting only uses atomics.
unsafe impl GlobalAlloc for RoadAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if over_limit(layout.size()) {
            FAILED_ALLOCS.fetch_add(1, Ordering::Relaxed);
            return core::ptr::null_mut();
        }
        // SAFETY: delegated to the system allocator with the same layout.
        let ptr = unsafe { MiMalloc.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if over_limit(layout.size()) {
            FAILED_ALLOCS.fetch_add(1, Ordering::Relaxed);
            return core::ptr::null_mut();
        }
        // SAFETY: delegated to the system allocator with the same layout.
        let ptr = unsafe { MiMalloc.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator with `layout`.
        unsafe { MiMalloc.dealloc(ptr, layout) };
        record_free(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size > layout.size() && over_limit(new_size - layout.size()) {
            FAILED_ALLOCS.fetch_add(1, Ordering::Relaxed);
            return core::ptr::null_mut();
        }
        // SAFETY: `ptr` came from this allocator with `layout` and `new_size` is
        // the requested size of the new block.
        let new_ptr = unsafe { MiMalloc.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                record_alloc(new_size - layout.size());
            } else {
                record_free(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

/// Snapshot of the allocator counters published to the UI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocStats {
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub limit_bytes: u64,
    pub failed_allocs: u64,
    pub rss_bytes: u64,
}

pub fn stats() -> AllocStats {
    AllocStats {
        live_bytes: LIVE_BYTES.load(Ordering::Relaxed) as u64,
        peak_bytes: PEAK_BYTES.load(Ordering::Relaxed) as u64,
        limit_bytes: LIMIT_BYTES.load(Ordering::Relaxed) as u64,
        failed_allocs: FAILED_ALLOCS.load(Ordering::Relaxed) as u64,
        rss_bytes: process_rss_bytes(),
    }
}

/// Resident set size of this process, which is what the kernel actually
/// charges against the 32-bit address-space budget.
pub fn process_rss_bytes() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let mut fields = statm.split_whitespace();
    let _total = fields.next();
    match fields.next().and_then(|value| value.parse::<u64>().ok()) {
        Some(pages) => pages.saturating_mul(4096),
        None => 0,
    }
}

pub fn set_limit(limit: usize) {
    LIMIT_BYTES.store(normalize_limit(limit), Ordering::Relaxed);
}

fn normalize_limit(limit: usize) -> usize {
    limit.max(1)
}

pub fn limit() -> usize {
    LIMIT_BYTES.load(Ordering::Relaxed)
}

/// Resets the peak counter; used by the memory panel and tests.
pub fn reset_peak() {
    PEAK_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_tracks_allocations() {
        // The counters are process-wide and tests run in parallel, so only
        // absolute facts are asserted: a live block is accounted for, and the
        // peak is never below the current value.
        let block = vec![0u8; 64 * 1024];
        let during = stats().live_bytes;
        assert!(during >= 64 * 1024, "live={during}");
        assert!(stats().peak_bytes >= during);
        assert!(stats().rss_bytes > 0, "rss should be readable on Linux/Android");
        drop(block);
    }

    #[test]
    fn soft_limit_logic_is_exact() {
        assert!(!exceeds_limit(100, 100, 200));
        assert!(exceeds_limit(100, 101, 200));
        assert!(!exceeds_limit(100, 100, usize::MAX));
        assert!(exceeds_limit(usize::MAX - 1, 8, 1024));
    }

    #[test]
    fn limit_round_trips() {
        // A tiny live limit would make parallel tests fail to allocate, so only
        // the normalisation is checked directly and the round trip uses a limit
        // no test can reach.
        assert_eq!(normalize_limit(0), 1);
        let original = limit();
        set_limit(2 * 1024 * 1024 * 1024);
        assert_eq!(limit(), 2 * 1024 * 1024 * 1024);
        set_limit(original);
    }
}
