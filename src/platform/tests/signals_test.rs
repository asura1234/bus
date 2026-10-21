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

#[cfg(target_os = "macos")]
#[test]
fn raising_thread_priority_never_lowers_a_thread_already_at_user_interactive() {
    fn current() -> libc::qos_class_t {
        let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
        let mut priority = 0;
        // SAFETY: reads this thread's own QoS into valid locals.
        unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut priority) };
        class
    }
    // A process's main thread starts at USER_INTERACTIVE; `bus server` runs
    // its loop there.
    let class = std::thread::spawn(|| {
        // SAFETY: changes only this test thread's own scheduling class.
        unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
        };
        set_current_thread_priority(ThreadPriority::Initiated);
        current()
    })
    .join()
    .unwrap();
    assert!(
        matches!(class, libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE),
        "{class:?}"
    );
}
