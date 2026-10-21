//! Counts every malloc/free/realloc in the process while armed, including those made by C++
//! code, which `assert_no_alloc` cannot see. macOS only: it hooks libmalloc's
//! `malloc_logger`, the callback behind `MallocStackLogging`. Elsewhere it reports `None`.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ARMED: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "macos")]
mod hook {
    use super::*;

    type Logger = unsafe extern "C" fn(u32, usize, usize, usize, usize, u32);

    unsafe extern "C" {
        static mut malloc_logger: Option<Logger>;
    }

    unsafe extern "C" fn count(_: u32, _: usize, _: usize, _: usize, _: usize, _: u32) {
        if ARMED.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn install() -> bool {
        // SAFETY: written once, from the main thread, before any measurement; libmalloc reads
        // it on every allocation. The callback only touches atomics.
        unsafe { malloc_logger = Some(count) };
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod hook {
    pub fn install() -> bool {
        false
    }
}

/// Install the hook. Returns false where counting is not supported.
pub fn install() -> bool {
    hook::install()
}

/// Run `f` and return its result with the number of allocator events it caused (`None` if
/// counting is unsupported).
pub fn count<R>(supported: bool, f: impl FnOnce() -> R) -> (R, Option<u64>) {
    COUNT.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::SeqCst);
    let r = f();
    ARMED.store(false, Ordering::SeqCst);
    (r, supported.then(|| COUNT.load(Ordering::Relaxed)))
}
