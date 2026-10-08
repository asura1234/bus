#[test]
fn terminal_resize_signal_is_recorded_once_per_delivery() {
    watch_terminal_resize_signal();
    assert!(!take_terminal_resize_signal());

    unsafe {
        libc::raise(libc::SIGWINCH);
    }

    assert!(take_terminal_resize_signal());
    assert!(!take_terminal_resize_signal());
}
