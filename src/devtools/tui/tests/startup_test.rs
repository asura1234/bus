use super::super::session::{self, Claim, Graphics, SessionInfo};
use super::*;
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

static SERIAL: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    paths: SessionPaths,
    child: Child,
    claim: Claim,
}

impl Fixture {
    fn new() -> Self {
        let root = std::path::PathBuf::from(format!(
            "/tmp/bts-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        session::create_private_dir(&root).unwrap();
        let paths = SessionPaths::new(&root, "start");
        let claim = Claim::new("start");
        paths.claim(&claim).unwrap();
        let child = Command::new("/bin/cat")
            .env_clear()
            .current_dir(&paths.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        paths
            .write_info(&SessionInfo {
                name: paths.name.clone(),
                host_pid: child.id(),
                bus_pid: None,
                data_dir: paths.data_dir(),
                run_dir: paths.dir.join("runs"),
                binary: "/usr/bin/false".into(),
                size: (80, 24),
                home: paths.home_dir(),
                graphics: Graphics::default(),
                yolo: false,
            })
            .unwrap();
        Self {
            paths,
            child,
            claim,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(self.paths.dir.parent().unwrap());
    }
}

fn bound_socket(path: &Path) -> OwnedFd {
    // SAFETY: socket returns a new fd; the initialized address contains a
    // terminated pathname. The fd stays owned until transferred to a listener.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    assert!(fd >= 0);
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    let bytes = path.as_os_str().as_bytes();
    assert!(bytes.len() < address.sun_path.len());
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = *from as _;
    }
    assert_eq!(
        unsafe {
            libc::bind(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                std::mem::size_of_val(&address) as _,
            )
        },
        0
    );
    fd
}

fn accept(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                return stream;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "no startup probe arrived");
                std::thread::yield_now();
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
}

fn read_status(stream: &mut UnixStream) {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"verb": "status"})
    );
}

#[test]
fn bound_path_is_retried_until_listen_and_a_complete_status_answer() {
    let mut fixture = Fixture::new();
    let fd = bound_socket(&fixture.paths.socket());
    assert_eq!(
        status(&fixture.paths, Instant::now() + PROBE_TIMEOUT)
            .unwrap_err()
            .kind(),
        io::ErrorKind::ConnectionRefused
    );
    let pid = fixture.child.id();
    std::thread::scope(|scope| {
        let (send, recv) = mpsc::channel();
        let paths = &fixture.paths;
        let child = &mut fixture.child;
        scope.spawn(move || {
            send.send(wait_status(
                paths,
                child,
                Instant::now() + Duration::from_secs(10),
            ))
            .unwrap()
        });
        assert!(matches!(
            recv.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        // SAFETY: this socket is bound and owned by this fixture.
        assert_eq!(unsafe { libc::listen(fd.as_raw_fd(), 4) }, 0);
        let listener = UnixListener::from(fd);
        let mut peer = accept(&listener);
        read_status(&mut peer);
        // An accepted connection and a partial reply still do not mean ready.
        write!(peer, "{{\"ok\":true,").unwrap();
        assert!(matches!(
            recv.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        writeln!(peer, "\"alive\":true,\"host_pid\":{pid}}}").unwrap();
        assert_eq!(
            recv.recv_timeout(Duration::from_secs(10)).unwrap().unwrap()["host_pid"],
            pid
        );
    });
}

#[test]
fn silent_accepted_peer_hits_deadline_and_failed_start_reaps_host() {
    let mut fixture = Fixture::new();
    let listener = UnixListener::bind(fixture.paths.socket()).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut peer = accept(&listener);
            read_status(&mut peer);
            // Keep the connection open until cleanup removes the claimed dir.
            let deadline = Instant::now() + Duration::from_secs(10);
            while fixture.paths.dir.exists() && Instant::now() < deadline {
                std::thread::yield_now();
            }
        });
        let result = wait(
            &fixture.paths,
            &mut fixture.child,
            Path::new("/usr/bin/false"),
            &fixture.claim.token,
            Instant::now(),
            Duration::from_secs(1),
        );
        assert_eq!(result["error"]["code"], "start_failed");
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("deadline"));
        assert!(fixture.child.try_wait().unwrap().is_some());
        assert!(!fixture.paths.dir.exists());
    });
}

#[test]
fn explicit_host_error_cleans_up_even_before_metadata_or_listen() {
    let mut fixture = Fixture::new();
    std::fs::remove_file(fixture.paths.info_file()).unwrap();
    std::fs::write(
        fixture.paths.dir.join("host.error"),
        "cannot bind driver socket",
    )
    .unwrap();
    let result = wait(
        &fixture.paths,
        &mut fixture.child,
        Path::new("/usr/bin/false"),
        &fixture.claim.token,
        Instant::now(),
        Duration::from_millis(50),
    );
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("cannot bind driver socket"));
    assert!(fixture.child.try_wait().unwrap().is_some());
    assert!(!fixture.paths.dir.exists());
}

#[test]
fn early_host_exit_without_error_still_removes_claimed_state() {
    let mut fixture = Fixture::new();
    fixture.child.kill().unwrap();
    fixture.child.wait().unwrap();
    let result = wait(
        &fixture.paths,
        &mut fixture.child,
        Path::new("/usr/bin/false"),
        &fixture.claim.token,
        Instant::now(),
        Duration::from_secs(1),
    );
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("host exited"));
    assert!(!fixture.paths.dir.exists());
}
