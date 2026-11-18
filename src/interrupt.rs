//! SIGINT, SIGTERM and SIGHUP during a deploy or provision are noted, not obeyed at once, so the
//! run stops at its next step boundary and its own cleanup runs: the build lease closes and a
//! half-built release is removed. The default action would kill bunker before either happened.
//! A handler (not SIG_IGN) resets to the default in children, so composer, npm and limactl still
//! die on Ctrl-C and their step fails normally.
use std::sync::atomic::{AtomicBool, Ordering};

pub static FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn note(_: libc::c_int) {
    FLAG.store(true, Ordering::SeqCst);
}

/// Installs the handler for SIGINT, SIGTERM and SIGHUP; `FLAG` turns true on any of them.
pub fn install() {
    let handler = note as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: `note` only stores to an atomic, which is async-signal-safe.
        unsafe {
            libc::signal(sig, handler);
        }
    }
}
