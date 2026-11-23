//! Lock-free single-producer/single-consumer ring buffer placed inside the
//! shared mapping.
//!
//! `ringbuf`'s `HeapRb` cannot be used here: its storage is allocated by Rust's
//! allocator and cannot be placement-constructed into a foreign mapping. This
//! ring stores its control block (`RingHeader`) in the mapping and treats the
//! following `capacity * size_of::<T>()` bytes as the element array, so both
//! processes address the same bytes.

use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;

use super::layout::RingHeader;

/// A ring over a caller-provided control block and data area.
pub struct SpscRing<T> {
    header: *mut RingHeader,
    data: *mut T,
    capacity: u32,
    mask: u32,
}

// SAFETY: the ring is designed to be shared between exactly one producer thread
// and one consumer thread (possibly in different processes). `T: Send` is the
// only requirement for moving values through it.
unsafe impl<T: Send> Send for SpscRing<T> {}
unsafe impl<T: Send> Sync for SpscRing<T> {}

impl<T: Copy> SpscRing<T> {
    /// # Safety
    /// `header` must point to an initialised [`RingHeader`] with `capacity`
    /// elements, and `data` must point to `capacity` writable `T` slots that
    /// stay valid for the lifetime of the ring.
    pub unsafe fn from_raw(header: *mut RingHeader, data: *mut T, capacity: u32) -> Self {
        debug_assert!(capacity.is_power_of_two());
        Self { header, data, capacity, mask: capacity - 1 }
    }

    /// Creates a ring over a header placed immediately before the element array
    /// in a single contiguous allocation.
    ///
    /// # Safety
    /// `region` must point to at least `RING_HEADER_SIZE + capacity * size_of::<T>()`
    /// writable bytes, aligned to `align_of::<RingHeader>()`.
    pub unsafe fn from_region(region: *mut u8, capacity: u32) -> Self {
        debug_assert!(capacity.is_power_of_two());
        let header = region as *mut RingHeader;
        // SAFETY: caller guarantees the header region is writable and aligned.
        unsafe { (&*header).init(capacity) };
        let data = unsafe { region.add(super::layout::RING_HEADER_SIZE) } as *mut T;
        Self { header, data, capacity, mask: capacity - 1 }
    }

    /// Attaches to an already initialised ring without resetting it.
    ///
    /// # Safety
    /// Same requirements as [`Self::from_region`], and the header must have been
    /// initialised with the same capacity.
    pub unsafe fn attach_region(region: *mut u8, capacity: u32) -> Self {
        let header = region as *mut RingHeader;
        debug_assert_eq!(unsafe { (*header).capacity }, capacity);
        let data = unsafe { region.add(super::layout::RING_HEADER_SIZE) } as *mut T;
        Self { header, data, capacity, mask: capacity - 1 }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Producer side: appends one value. Returns `false` (and counts a drop)
    /// when the ring is full.
    pub fn push(&self, value: T) -> bool {
        // SAFETY: the header and element array are valid for the lifetime of
        // the ring, and only the producer writes `tail`.
        unsafe {
            let header = &*self.header;
            let tail = header.tail.load(Ordering::Relaxed);
            let head = header.head.load(Ordering::Acquire);
            if tail.wrapping_sub(head) >= self.capacity {
                header.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            core::ptr::write(self.data.add((tail & self.mask) as usize), value);
            header.tail.store(tail.wrapping_add(1), Ordering::Release);
        }
        true
    }

    /// Consumer side: removes one value.
    pub fn pop(&self) -> Option<T> {
        // SAFETY: the header and element array are valid for the lifetime of
        // the ring, and only the consumer writes `head`.
        unsafe {
            let header = &*self.header;
            let head = header.head.load(Ordering::Relaxed);
            let tail = header.tail.load(Ordering::Acquire);
            if head == tail {
                return None;
            }
            let value = core::ptr::read(self.data.add((head & self.mask) as usize));
            header.head.store(head.wrapping_add(1), Ordering::Release);
            Some(value)
        }
    }

    pub fn len(&self) -> u32 {
        // SAFETY: both fields are plain atomics inside a live header.
        unsafe {
            let header = &*self.header;
            let tail = header.tail.load(Ordering::Acquire);
            let head = header.head.load(Ordering::Acquire);
            tail.wrapping_sub(head)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn dropped(&self) -> u32 {
        // SAFETY: the header is live for the lifetime of the ring.
        unsafe { (*self.header).dropped.load(Ordering::Relaxed) }
    }
}

/// A heap-backed ring used by tests and by in-process staging queues.
pub struct OwnedRing<T> {
    storage: Vec<MaybeUninit<T>>,
    // The control block must outlive the raw ring view.
    header: Box<RingHeader>,
    ring: SpscRing<T>,
}

impl<T: Copy> OwnedRing<T> {
    pub fn new(capacity: u32) -> Self {
        assert!(capacity.is_power_of_two());
        let mut storage: Vec<MaybeUninit<T>> = Vec::with_capacity(capacity as usize);
        // SAFETY: we never read uninitialised slots: the ring only exposes
        // values that were written by `push`.
        unsafe { storage.set_len(capacity as usize) };
        let header = Box::new(RingHeader {
            head: core::sync::atomic::AtomicU32::new(0),
            tail: core::sync::atomic::AtomicU32::new(0),
            capacity,
            mask: capacity - 1,
            dropped: core::sync::atomic::AtomicU32::new(0),
            reserved: [0; 11],
        });
        let header_ptr = Box::into_raw(header);
        // SAFETY: `header_ptr` is a valid RingHeader and `storage` holds
        // `capacity` writable slots.
        let ring = unsafe { SpscRing::from_raw(header_ptr, storage.as_mut_ptr() as *mut T, capacity) };
        Self { storage, header: unsafe { Box::from_raw(header_ptr) }, ring }
    }

    pub fn push(&self, value: T) -> bool {
        self.ring.push(value)
    }

    pub fn pop(&self) -> Option<T> {
        self.ring.pop()
    }

    pub fn len(&self) -> u32 {
        self.ring.len()
    }

    pub fn dropped(&self) -> u32 {
        self.ring.dropped()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn capacity(&self) -> u32 {
        self.ring.capacity()
    }

    fn _keep_alive(&self) -> (&Vec<MaybeUninit<T>>, &RingHeader) {
        (&self.storage, &self.header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_order_is_preserved() {
        let ring = OwnedRing::<u64>::new(8);
        for value in 0..8u64 {
            assert!(ring.push(value));
        }
        assert_eq!(ring.len(), 8);
        for value in 0..8u64 {
            assert_eq!(ring.pop(), Some(value));
        }
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn full_ring_drops_and_counts() {
        let ring = OwnedRing::<u32>::new(4);
        for value in 0..4 {
            assert!(ring.push(value));
        }
        assert!(!ring.push(99));
        assert!(!ring.push(100));
        assert_eq!(ring.dropped(), 2);
        assert_eq!(ring.pop(), Some(0));
        assert!(ring.push(4));
        assert_eq!(ring.len(), 4);
    }

    #[test]
    fn wraps_around_many_times() {
        let ring = OwnedRing::<u32>::new(16);
        let mut next_push = 0u32;
        let mut next_pop = 0u32;
        for _ in 0..10_000 {
            for _ in 0..3 {
                if ring.push(next_push) {
                    next_push += 1;
                }
            }
            while let Some(value) = ring.pop() {
                assert_eq!(value, next_pop);
                next_pop += 1;
            }
        }
        assert_eq!(ring.dropped(), 0);
    }

    #[test]
    fn single_producer_single_consumer_across_threads() {
        let ring = std::sync::Arc::new(OwnedRing::<u64>::new(1024));
        let producer_ring = std::sync::Arc::clone(&ring);
        let producer = std::thread::spawn(move || {
            let mut sent = 0u64;
            while sent < 100_000 {
                if producer_ring.push(sent) {
                    sent += 1;
                }
            }
        });
        let mut received = 0u64;
        while received < 100_000 {
            if let Some(value) = ring.pop() {
                assert_eq!(value, received);
                received += 1;
            }
        }
        producer.join().expect("producer thread");
        assert_eq!(ring.len(), 0);
        let _ = ring._keep_alive();
    }
}
