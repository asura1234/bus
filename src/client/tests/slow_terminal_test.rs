use super::*;
use crate::protocol::wire::{CellData, FrameData, PaneSurfacePatchRow, ServerMessage};
use std::{
    fs,
    io::Read,
    os::fd::{AsRawFd, FromRawFd},
    process::{Command, Stdio},
    time::Instant,
};

const CHILD: &str = "BUS_SLOW_TERMINAL_PROBE";
const TEST: &str =
    "client::event_loop::tests::slow_terminal::paused_terminal_keeps_processing_events";

fn frame(symbol: &str) -> FrameData {
    FrameData {
        width: 120,
        height: 40,
        cells: (0..4800)
            .map(|i| CellData {
                symbol: symbol.into(),
                fg: 0xff000000 | (i * 1337),
                bg: 0,
                modifier: 0,
                skip: false,
                hyperlink: None,
            })
            .collect(),
        cursor: None,
        hyperlinks: vec![],
        graphics: vec![],
    }
}

#[test]
fn paused_terminal_keeps_processing_events() {
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let (mut driver, _) = driver(120);
        fs::write(root.join("started"), "present").unwrap();
        let mut timer = timer::ClientLoopTimer::new();
        for _ in 0..8 {
            driver.state.present_frame(frame("X"));
            driver
                .handle_event(
                    ClientLoopEvent::StdinInput(b"key".to_vec()),
                    Instant::now(),
                    &mut timer,
                )
                .unwrap();
            driver
                .handle_event(ClientLoopEvent::Timer, Instant::now(), &mut timer)
                .unwrap();
            driver
                .handle_event(
                    ClientLoopEvent::ServerMessage {
                        payload_bytes: 0, // Synthetic event: no wire frame was read.
                        message: Box::new(ServerMessage::ClientShellError {
                            message: "probe".into(),
                        }),
                    },
                    Instant::now(),
                    &mut timer,
                )
                .unwrap();
            fs::write(root.join("events"), "key timer server").unwrap();
            let patch = shell::ClientComposedSurfacePatch {
                rows: (0..40)
                    .map(|y| PaneSurfacePatchRow {
                        x: 0,
                        y,
                        cells: frame("Y").cells[..120].to_vec(),
                    })
                    .collect(),
                cursor: None,
            };
            assert!(driver.state.present_surface_patch(patch).unwrap());
        }
        driver.state.present_frame(frame("Z"));
        fs::write(root.join("done"), "done").unwrap();
        // Keep the process alive long enough for its terminal writer to drain.
        while !root.join("release").exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
        return;
    }
    let root = std::env::temp_dir().join(format!("bus-slow-terminal-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let (mut master, slave) = unsafe {
        let mut m = -1;
        let mut s = -1;
        assert_eq!(
            libc::openpty(
                &mut m,
                &mut s,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            0
        );
        (std::fs::File::from_raw_fd(m), std::fs::File::from_raw_fd(s))
    };
    unsafe {
        libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, &root)
        .env("BUS_DATA_DIR", root.join("data"))
        .stdin(Stdio::null())
        .stdout(slave)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("started").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(root.join("started").exists(), "child failed to start");
    // No reads at all: a real host-terminal write must exhaust PTY capacity.
    std::thread::sleep(Duration::from_secs(1));
    let responsive = root.join("events").exists();
    eprintln!("paused PTY for 1000 ms: input/timer/server handled = {responsive}");
    let mut bytes = Vec::new();
    let mut buf = [0; 65536];
    let drain_deadline = Instant::now() + Duration::from_secs(20);
    let mut last_read = Instant::now();
    while Instant::now() < drain_deadline {
        match master.read(&mut buf) {
            Ok(n) if n > 0 => {
                bytes.extend_from_slice(&buf[..n]);
                last_read = Instant::now();
            }
            _ => std::thread::sleep(Duration::from_millis(2)),
        }
        if root.join("done").exists()
            && bytes.contains(&b'Z')
            && last_read.elapsed() > Duration::from_millis(200)
        {
            break;
        }
    }
    let done = root.join("done").exists();
    fs::write(root.join("release"), "release").unwrap();
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() && Instant::now() < exit_deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let status = child.wait().unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert!(
        done && status.success(),
        "probe must finish after resuming terminal drain"
    );
    assert!(
        bytes.contains(&b'Z'),
        "latest frame must eventually reach terminal"
    );
    assert!(
        responsive,
        "UI event processing stalled behind host-terminal output"
    );
}
