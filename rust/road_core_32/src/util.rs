//! Small utilities shared by the companion core.

use core::time::Duration;

/// Rounds `value` up to the next multiple of `align` (which must be a power of two).
pub const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// True when `value` is a power of two.
pub const fn is_power_of_two(value: u32) -> bool {
    value != 0 && value & (value - 1) == 0
}

/// CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Monotonic clock in nanoseconds, suitable for cross-process heartbeat checks.
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

/// Monotonic clock as a [`Duration`] since an arbitrary epoch.
pub fn monotonic() -> Duration {
    Duration::from_nanos(monotonic_ns())
}

/// Latency in nanoseconds between `earlier_ns` and now, saturating at zero.
pub fn elapsed_ns(earlier_ns: u64) -> u64 {
    monotonic_ns().saturating_sub(earlier_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_rounds_to_power_of_two() {
        assert_eq!(align_up(0, 4096), 0);
        assert_eq!(align_up(1, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
    }

    #[test]
    fn crc32_matches_reference_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn power_of_two_detection() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(1024));
        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(1000));
    }
}
