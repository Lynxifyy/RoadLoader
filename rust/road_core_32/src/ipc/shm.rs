//! Shared-memory transport.
//!
//! On Android 11+ (`memfd_create` available) we use an anonymous `memfd`; on
//! older releases we fall back to `/dev/ashmem`. Both give a file descriptor
//! that can be passed to the companion process through a
//! `ParcelFileDescriptor` and mapped with `PROT_READ | PROT_WRITE` in both
//! processes.

use core::ffi::c_void;
use std::io;
use std::os::unix::io::RawFd;

const ASHMEM_SET_NAME: libc::c_ulong = 0x4100_7701;
const ASHMEM_SET_SIZE: libc::c_ulong = 0x4008_7703;
const ASHMEM_GET_SIZE: libc::c_ulong = 0x7704;
const MFD_CLOEXEC: libc::c_uint = 0x0001;

#[cfg(target_arch = "x86_64")]
const SYS_MEMFD_CREATE: libc::c_long = 319;
#[cfg(target_arch = "x86")]
const SYS_MEMFD_CREATE: libc::c_long = 356;
#[cfg(target_arch = "aarch64")]
const SYS_MEMFD_CREATE: libc::c_long = 279;
#[cfg(target_arch = "arm")]
const SYS_MEMFD_CREATE: libc::c_long = 385;

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

fn memfd_create(name: &[u8]) -> io::Result<RawFd> {
    // SAFETY: `name` is NUL-terminated by the caller and the syscall only reads it.
    let fd = unsafe {
        libc::syscall(
            SYS_MEMFD_CREATE,
            name.as_ptr() as *const libc::c_char,
            MFD_CLOEXEC,
        ) as RawFd
    };
    if fd < 0 {
        Err(last_error())
    } else {
        Ok(fd)
    }
}

fn ashmem_create(size: usize) -> io::Result<RawFd> {
    // SAFETY: plain open(2) with a static, NUL-terminated path.
    let fd = unsafe {
        libc::open(
            b"/dev/ashmem\0".as_ptr() as *const libc::c_char,
            libc::O_RDWR | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(last_error());
    }
    let name = b"road-loader-ipc\0";
    // SAFETY: `fd` is a live ashmem descriptor; the ioctls only read the name
    // buffer and the size value.
    let name_rc = unsafe { libc::ioctl(fd, ASHMEM_SET_NAME as _, name.as_ptr()) };
    if name_rc < 0 {
        let err = last_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }
    // SAFETY: see above; `size` is passed by value as the ioctl argument.
    let size_rc = unsafe { libc::ioctl(fd, ASHMEM_SET_SIZE as _, size) };
    if size_rc < 0 {
        let err = last_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }
    Ok(fd)
}

/// A writable mapping of a shared-memory object.
pub struct SharedMemory {
    fd: RawFd,
    base: *mut u8,
    size: usize,
    owns_fd: bool,
}

// SAFETY: the mapping is process-wide and the struct carries no thread-affine
// state; concurrent access is governed by the ring/frame protocols.
unsafe impl Send for SharedMemory {}
unsafe impl Sync for SharedMemory {}

impl SharedMemory {
    /// Creates a new shared-memory object and maps it read/write.
    pub fn create(size: usize) -> io::Result<Self> {
        let size = crate::util::align_up(size, 4096);
        let (fd, owns_fd) = match memfd_create(b"road-loader-ipc\0") {
            Ok(fd) => (fd, true),
            Err(_) => (ashmem_create(size)?, true),
        };
        // memfd needs an explicit size; ashmem was sized by the ioctl.
        // SAFETY: `fd` is a live descriptor owned by this object.
        let truncate_rc = unsafe { libc::ftruncate(fd, size as libc::off_t) };
        if truncate_rc != 0 {
            let err = last_error();
            // ashmem rejects ftruncate but is already sized; only fail when the
            // resulting size is wrong.
            let actual = mapping_size(fd).unwrap_or(0);
            if actual < size {
                unsafe { libc::close(fd) };
                return Err(err);
            }
        }
        // SAFETY: `fd` is live and the size was set above.
        let base = unsafe {
            libc::mmap(
                core::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            let err = last_error();
            // SAFETY: closing the descriptor we own.
            unsafe { libc::close(fd) };
            return Err(err);
        }
        Ok(Self { fd, base: base as *mut u8, size, owns_fd })
    }

    /// Maps an existing descriptor received from the other process.
    ///
    /// The descriptor is *not* closed on drop: the caller (the JNI bridge or the
    /// service) owns it, and the mapping stays valid after it is closed.
    pub fn from_fd(fd: RawFd, size: usize) -> io::Result<Self> {
        if fd < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "negative fd"));
        }
        let actual = mapping_size(fd)?;
        if actual < size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("shared memory too small: {actual} < {size}"),
            ));
        }
        // SAFETY: `fd` is a live, shared-memory capable descriptor supplied by
        // the caller and `size` was validated against fstat.
        let base = unsafe {
            libc::mmap(
                core::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(last_error());
        }
        Ok(Self { fd, base: base as *mut u8, size, owns_fd: false })
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// Duplicates the descriptor so it can be handed to another process while
    /// this object keeps its own copy.
    pub fn duplicate_fd(&self) -> io::Result<RawFd> {
        // SAFETY: `self.fd` is a live descriptor.
        let dup = unsafe { libc::fcntl(self.fd, libc::F_DUPFD_CLOEXEC, 0) };
        if dup < 0 {
            Err(last_error())
        } else {
            Ok(dup)
        }
    }

    /// Copies bytes in from a host-side buffer. Used by the asset streamer.
    pub fn write_at(&self, offset: usize, bytes: &[u8]) -> Result<(), &'static str> {
        if offset.checked_add(bytes.len()).map_or(true, |end| end > self.size) {
            return Err("write out of bounds");
        }
        // SAFETY: bounds were checked against the mapping size and the source
        // and destination cannot overlap (source is a Rust slice).
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.base.add(offset),
                bytes.len(),
            );
        }
        Ok(())
    }

    /// Copies bytes out to a host-side buffer.
    pub fn read_at(&self, offset: usize, out: &mut [u8]) -> Result<(), &'static str> {
        if offset.checked_add(out.len()).map_or(true, |end| end > self.size) {
            return Err("read out of bounds");
        }
        // SAFETY: bounds were checked above.
        unsafe {
            core::ptr::copy_nonoverlapping(self.base.add(offset), out.as_mut_ptr(), out.len());
        }
        Ok(())
    }
}

fn mapping_size(fd: RawFd) -> io::Result<usize> {
    // SAFETY: `stat` is a plain output buffer for fstat(2).
    let mut stat: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `fd` is a live descriptor and `stat` is writable.
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(last_error());
    }
    let size = stat.st_size as usize;
    if size == 0 {
        // SAFETY: ashmem exposes its size through this ioctl.
        let ashmem_size = unsafe { libc::ioctl(fd, ASHMEM_GET_SIZE as _) };
        if ashmem_size > 0 {
            return Ok(ashmem_size as usize);
        }
    }
    Ok(size)
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        // SAFETY: `base`/`size` describe the mapping created in `create` or
        // `from_fd`, and `fd` is only closed when this object owns it.
        unsafe {
            libc::munmap(self.base as *mut c_void, self.size);
            if self.owns_fd {
                libc::close(self.fd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_round_trip() {
        let shm = SharedMemory::create(8192).expect("create");
        assert!(shm.size() >= 8192);
        shm.write_at(128, b"road-loader").expect("write");
        let mut out = [0u8; 11];
        shm.read_at(128, &mut out).expect("read");
        assert_eq!(&out, b"road-loader");
    }

    #[test]
    fn two_mappings_share_the_same_pages() {
        let shm = SharedMemory::create(4096).expect("create");
        let fd = shm.duplicate_fd().expect("dup");
        let second = SharedMemory::from_fd(fd, 4096).expect("attach");
        shm.write_at(0, &[1, 2, 3, 4]).expect("write");
        let mut out = [0u8; 4];
        second.read_at(0, &mut out).expect("read");
        assert_eq!(out, [1, 2, 3, 4]);
        // SAFETY: we own the duplicated descriptor.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn out_of_bounds_is_rejected() {
        let shm = SharedMemory::create(4096).expect("create");
        assert!(shm.write_at(4090, &[0u8; 16]).is_err());
        assert!(shm.write_at(usize::MAX, &[0u8; 2]).is_err());
    }

    #[test]
    fn attach_validates_size() {
        let shm = SharedMemory::create(4096).expect("create");
        let fd = shm.duplicate_fd().expect("dup");
        let result = SharedMemory::from_fd(fd, 1 << 20);
        assert!(result.is_err());
        assert_eq!(result.err().map(|error| error.kind()), Some(io::ErrorKind::InvalidInput));
        // SAFETY: we own the duplicated descriptor.
        unsafe { libc::close(fd) };
    }
}
