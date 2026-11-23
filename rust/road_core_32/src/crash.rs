//! Crash capture for the companion process.
//!
//! Faults are written into the shared header before the process dies so the
//! 64-bit UI can show the address, signal and faulting PC instead of a black
//! screen. The handler is deliberately tiny and async-signal-safe: atomics plus
//! a bounded copy, then the default action is restored and the signal re-raised
//! so the tombstone is still produced for logcat.

use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::ffi::c_void;

use crate::ipc::region::Region;

static REGION: AtomicPtr<Region> = AtomicPtr::new(core::ptr::null_mut());
static INSTALLED: AtomicU32 = AtomicU32::new(0);

/// Installs the handlers once and records the region they should report to.
pub fn install(region: &Region) -> bool {
    if INSTALLED.swap(1, Ordering::SeqCst) == 1 {
        REGION.store(region as *const Region as *mut Region, Ordering::SeqCst);
        return true;
    }
    REGION.store(region as *const Region as *mut Region, Ordering::SeqCst);
    for signal in [libc::SIGSEGV, libc::SIGABRT, libc::SIGBUS, libc::SIGFPE, libc::SIGILL] {
        // SAFETY: `install_handler` fills a zeroed sigaction with a valid handler
        // and calls sigaction(2) with it.
        let ok = unsafe { install_handler(signal) };
        if !ok {
            return false;
        }
    }
    true
}

unsafe fn install_handler(signal: libc::c_int) -> bool {
    // SAFETY: `action` is a plain struct; zeroing it is valid.
    let mut action: libc::sigaction = unsafe { core::mem::zeroed() };
    action.sa_sigaction = crash_handler as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
    // SAFETY: `action` is fully initialised and `signal` is a valid signal number.
    unsafe { libc::sigaction(signal, &action, core::ptr::null_mut()) == 0 }
}

extern "C" fn crash_handler(signal: libc::c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    // SAFETY: the kernel passes a valid siginfo_t for SA_SIGINFO handlers.
    let (code, address) = unsafe {
        if info.is_null() {
            (0, 0)
        } else {
            ((*info).si_code, (*info).si_addr() as usize)
        }
    };
    let (pc, lr) = fault_registers(context);
    let region_ptr = REGION.load(Ordering::SeqCst);
    if !region_ptr.is_null() {
        // SAFETY: `region_ptr` points at the live static region installed above.
        let region = unsafe { &*region_ptr };
        // No allocation here: the handler may interrupt the allocator.
        let mut message = [0u8; 48];
        let name = signal_name(signal);
        let mut length = name.len().min(message.len());
        message[..length].copy_from_slice(&name[..length]);
        // Append " code=<n>" using fixed-width decimal formatting.
        if length < message.len() {
            let suffix = b" code=";
            let take = suffix.len().min(message.len() - length);
            message[length..length + take].copy_from_slice(&suffix[..take]);
            length += take;
        }
        let mut digits = [0u8; 12];
        let mut value = code.unsigned_abs();
        let mut digit_count = 1usize;
        while value >= 10 && digit_count < digits.len() {
            digits[digit_count] = b'0' + (value % 10) as u8;
            value /= 10;
            digit_count += 1;
        }
        digits[0] = b'0' + (value % 10) as u8;
        if code < 0 && length < message.len() {
            message[length] = b'-';
            length += 1;
        }
        for index in (0..digit_count).rev() {
            if length >= message.len() {
                break;
            }
            message[length] = digits[index];
            length += 1;
        }
        // SAFETY: signal-handler context; `record_crash` only uses atomics and a
        // bounded copy.
        unsafe {
            region.record_crash(
                signal as u32,
                code as u32,
                address as u64,
                pc as u64,
                lr as u64,
                &message[..length],
            );
        }
    }
    // Restore the default action and re-raise so the system tombstone is intact.
    // SAFETY: `signal` is valid and SIG_DFL is a valid disposition.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
        libc::_exit(128 + signal);
    }
}

fn signal_name(signal: libc::c_int) -> &'static [u8] {
    match signal {
        libc::SIGSEGV => b"SIGSEGV",
        libc::SIGABRT => b"SIGABRT",
        libc::SIGBUS => b"SIGBUS",
        libc::SIGFPE => b"SIGFPE",
        libc::SIGILL => b"SIGILL",
        _ => b"SIGNAL",
    }
}

#[cfg(target_arch = "arm")]
fn fault_registers(context: *mut c_void) -> (usize, usize) {
    if context.is_null() {
        return (0, 0);
    }
    // SAFETY: on arm the kernel passes a ucontext_t for SA_SIGINFO handlers.
    unsafe {
        let ucontext = &*(context as *const libc::ucontext_t);
        (ucontext.uc_mcontext.arm_pc as usize, ucontext.uc_mcontext.arm_lr as usize)
    }
}

#[cfg(target_arch = "aarch64")]
fn fault_registers(context: *mut c_void) -> (usize, usize) {
    if context.is_null() {
        return (0, 0);
    }
    // SAFETY: on aarch64 the kernel passes a ucontext_t for SA_SIGINFO handlers.
    unsafe {
        let ucontext = &*(context as *const libc::ucontext_t);
        (ucontext.uc_mcontext.pc as usize, ucontext.uc_mcontext.regs[30] as usize)
    }
}

#[cfg(target_arch = "x86_64")]
fn fault_registers(context: *mut c_void) -> (usize, usize) {
    if context.is_null() {
        return (0, 0);
    }
    // SAFETY: on x86_64 the kernel passes a ucontext_t for SA_SIGINFO handlers.
    unsafe {
        let ucontext = &*(context as *const libc::ucontext_t);
        (
            ucontext.uc_mcontext.gregs[libc::REG_RIP as usize] as usize,
            ucontext.uc_mcontext.gregs[libc::REG_RBP as usize] as usize,
        )
    }
}

#[cfg(not(any(target_arch = "arm", target_arch = "aarch64", target_arch = "x86_64")))]
fn fault_registers(_context: *mut c_void) -> (usize, usize) {
    (0, 0)
}

/// Installs a panic hook that records the message before aborting.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic| {
        let message = if let Some(location) = panic.location() {
            format!("panic at {}:{}: {}", location.file(), location.line(), panic)
        } else {
            format!("panic: {panic}")
        };
        let region_ptr = REGION.load(Ordering::SeqCst);
        if !region_ptr.is_null() {
            // SAFETY: `region_ptr` points at the live static region.
            let region = unsafe { &*region_ptr };
            // SAFETY: records the panic before the abort; same bounded writes as
            // the signal handler.
            unsafe { region.record_crash(0, 0, 0, 0, 0, message.as_bytes()) };
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names_are_static() {
        assert_eq!(signal_name(libc::SIGSEGV), b"SIGSEGV");
        assert_eq!(signal_name(libc::SIGABRT), b"SIGABRT");
        assert_eq!(signal_name(99), b"SIGNAL");
    }

    #[test]
    fn null_context_is_safe() {
        assert_eq!(fault_registers(core::ptr::null_mut()), (0, 0));
    }
}
