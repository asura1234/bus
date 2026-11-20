//! Host terminal resize and interrupt signal handling.

/// Raised by the SIGWINCH handler, consumed by the host resize watcher.
#[cfg(unix)]
static TERMINAL_RESIZE_SIGNALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn record_terminal_resize_signal(_signal: libc::c_int) {
    TERMINAL_RESIZE_SIGNALLED.store(true, std::sync::atomic::Ordering::Release);
}

/// Records SIGWINCH events that size polling can miss.
#[cfg(unix)]
pub(crate) fn watch_terminal_resize_signal() {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction =
        record_terminal_resize_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // Keep blocking stdin and socket reads from failing with EINTR.
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
pub(crate) fn watch_terminal_resize_signal() {}

#[cfg(unix)]
extern "C" fn disregard_signal(_signal: libc::c_int) {}

/// Makes SIGINT harmless to this process. A no-op handler is used instead of
/// SIG_IGN because exec resets handlers but inherits SIG_IGN, and the client
/// may spawn a server whose agents must still be interruptible with Ctrl+C.
#[cfg(unix)]
pub(crate) fn disregard_interrupt_signal() {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = disregard_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    action.sa_flags = libc::SA_RESTART;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
}

/// Raw console mode reads Ctrl+C as input rather than a control event.
#[cfg(not(unix))]
pub(crate) fn disregard_interrupt_signal() {}

/// Returns whether a terminal size change was signalled since the last call.
#[cfg(unix)]
pub(crate) fn take_terminal_resize_signal() -> bool {
    TERMINAL_RESIZE_SIGNALLED.swap(false, std::sync::atomic::Ordering::AcqRel)
}

/// Windows relies on size polling.
#[cfg(not(unix))]
pub(crate) fn take_terminal_resize_signal() -> bool {
    false
}

/// After the UI loop, termination must kill the process again.
///
/// `ctrlc` stays installed for the whole process and only sets a flag. Once
/// the loop has exited, a stuck save or server stop would ignore SIGTERM the
/// way a restart waiter did.
#[cfg(unix)]
pub(crate) fn restore_default_termination_signals() {
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        libc::signal(libc::SIGHUP, libc::SIG_DFL);
    }
}

#[cfg(unix)]
static QUIT_FLAG: std::sync::atomic::AtomicPtr<std::sync::atomic::AtomicBool> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[cfg(unix)]
extern "C" fn mark_quit_flag(_signal: libc::c_int) {
    let flag = QUIT_FLAG.load(std::sync::atomic::Ordering::Relaxed);
    if !flag.is_null() {
        unsafe {
            (*flag).store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

/// SIGTERM and SIGHUP set the quit flag directly.
///
/// `ctrlc`'s Apple handler posts a dispatch semaphore from the signal
/// context, and that post does not wake the waiter, so the flag stays false
/// and the process keeps sleeping in `kevent`. No `SA_RESTART`, so the
/// runtime's poll returns and the loop observes the flag.
#[cfg(unix)]
pub(crate) fn install_quit_on_termination(flag: &std::sync::atomic::AtomicBool) {
    QUIT_FLAG.store(
        std::ptr::from_ref(flag).cast_mut(),
        std::sync::atomic::Ordering::Release,
    );
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = mark_quit_flag as *const () as libc::sighandler_t;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGHUP, &action, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
pub(crate) fn install_quit_on_termination(_flag: &std::sync::atomic::AtomicBool) {}

#[cfg(not(unix))]
pub(crate) fn restore_default_termination_signals() {}
