use super::*;
use crate::terminal::pty::{
    actor::{PtyIoActor, PtyIoActorConfig, PtyReadResult},
    fd,
};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::net::UnixStream,
};

fn read_exact(peer: &mut File, buffer: &mut [u8]) {
    let mut remaining = buffer;
    while !remaining.is_empty() {
        let mut poll = libc::pollfd {
            fd: peer.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert!(
            unsafe { libc::poll(&mut poll, 1, 2000) } > 0,
            "actor did not service subsequent input"
        );
        match peer.read(remaining) {
            Ok(0) => panic!("peer closed"),
            Ok(n) => remaining = &mut remaining[n..],
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => panic!("read failed: {e}"),
        }
    }
}

fn backpressured_cleanup(master: OwnedFd, peer: OwnedFd) {
    fd::set_nonblocking(master.as_raw_fd()).unwrap();
    fd::set_nonblocking(peer.as_raw_fd()).unwrap();
    let mut master = File::from(master);
    let mut peer = File::from(peer);
    let mut prefilled = 0;
    for fill in [&[0xAA; 8192][..], &[0xAA][..]] {
        loop {
            match master.write(fill) {
                Ok(written) => prefilled += written,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("prefill failed: {error}"),
            }
            assert!(
                prefilled < 16 * 1024 * 1024,
                "fixture must apply local backpressure"
            );
        }
    }
    assert!(prefilled > 0);
    let handle = PtyIoActor::spawn(PtyIoActorConfig {
        pane_id: 1,
        master_fd: master.into(),
        initially_quiesced: false,
        on_read: Box::new(|_| PtyReadResult::empty()),
    })
    .unwrap();
    let (runtime, _writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let hard_deadline = Instant::now() + Duration::from_millis(150);
    let submission = ObservedSubmission {
        scope: Default::default(),
        terminal: runtime.terminal.clone(),
        io: PaneRuntimeIo::Actor(handle.clone()),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|_: &str| InputObservation::Pending),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(hard_deadline),
        hard_deadline,
        withdraw_grace: Duration::from_secs(1),
        clock: SubmissionClock::default(),
        diagnostics: Default::default(),
        startup_suggestion_open: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    let result =
        submission.withdraw_pending(io::Error::new(io::ErrorKind::TimedOut, "unconfirmed Enter"));
    let finished = Instant::now();
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("could not be safely cleared"));
    assert!(finished >= hard_deadline);
    // Release pressure only after completion. A following ordinary-input marker
    // proves the actor resumed and cannot hide a late cleanup key behind a stall.
    handle
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut received = vec![0; prefilled + 5];
    read_exact(&mut peer, &mut received);
    handle.shutdown();
    assert!(received[..prefilled].iter().all(|byte| *byte == 0xAA));
    assert_eq!(
        &received[prefilled..],
        b"after",
        "cleanup key arrived after completion"
    );
}

#[tokio::test]
async fn backpressured_cleanup_is_cancelled_before_reporting_timeout() {
    let (master, peer) = UnixStream::pair().unwrap();
    backpressured_cleanup(master.into(), peer.into());
}

#[tokio::test]
async fn real_pty_backpressured_cleanup_is_cancelled_before_reporting_timeout() {
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // Raw mode disables canonical line buffering, echo and input transformations.
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), termios.as_mut_ptr()) },
        0
    );
    let mut termios = unsafe { termios.assume_init() };
    unsafe { libc::cfmakeraw(&mut termios) };
    assert_eq!(
        unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios) },
        0
    );
    backpressured_cleanup(master, slave);
}
