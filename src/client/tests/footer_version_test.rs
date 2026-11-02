use super::*;
use crate::protocol::wire::handshake::{EndpointServerWelcome, ENDPOINT_WELCOME_KIND};
use crate::protocol::wire::{self, ClientMessage, ServerMessage, MAX_FRAME_SIZE};
use crate::utils::restart_record;
use interprocess::local_socket::traits::Listener as _;

struct Scratch {
    root: std::path::PathBuf,
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Scratch {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("fv");
        crate::platform::fs::private_dir(&root).unwrap();
        let saved = [
            "BUS_DATA_DIR",
            crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
        ]
        .map(|key| (key, std::env::var_os(key)))
        .into();
        std::env::set_var("BUS_DATA_DIR", &root);
        std::env::set_var(
            crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
            root.join("c.sock"),
        );
        Self { root, saved }
    }

    fn attach(&self, driver: &mut ClientLoopDriver, sha: Option<&str>) {
        let path = restart_record::build_path(&self.root);
        if let Some(sha) = sha {
            restart_record::write_json(
                &path,
                &serde_json::json!({ "sha": sha, "pid": 22, "binary": "/new/bus" }),
            )
            .unwrap();
        } else {
            let _ = std::fs::remove_file(path);
        }
        let socket = self.root.join("c.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = crate::platform::ipc::bind_local_listener(&socket).unwrap();
        let (done, wait) = std::sync::mpsc::channel();
        let peer = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let _: ClientMessage = wire::read_message(&mut stream, MAX_FRAME_SIZE).unwrap();
            wire::write_message(
                &mut stream,
                &ServerMessage::EndpointControl {
                    kind: ENDPOINT_WELCOME_KIND.into(),
                    data: serde_json::to_string(&EndpointServerWelcome::compatible(Vec::new()))
                        .unwrap(),
                },
            )
            .unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let (tx, _rx) = crate::utils::perf::queue::channel(32);
        let result = driver.attach_restarted_server(&tx);
        done.send(()).unwrap();
        peer.join().unwrap();
        assert!(result.is_ok(), "attachment failed");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn footer(driver: &mut ClientLoopDriver) -> String {
    let ui = driver.state.shell.as_mut().unwrap().bus.as_mut().unwrap();
    ui.compute_view(120, 40);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 120, 40));
    ui.render(&mut buffer);
    let row = (1..120)
        .map(|x| buffer[(x, 38)].symbol())
        .take_while(|symbol| *symbol != "│")
        .collect::<String>();
    let label = row.trim_end().to_owned();
    for y in 0..40 {
        let main = (28..120)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>();
        assert!(!main.contains(&label), "version must stay in the sidebar");
        assert!(!main.contains("TUI"));
    }
    assert!(!label.contains("TUI"), "show only the server version");
    label
}

#[test]
fn footer_version_tracks_the_running_server_after_restart() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    let client = env!("BUS_BUILD_SHA");
    scratch.attach(&mut driver, Some(client));

    let server = if client.starts_with("ab") { "cd" } else { "ab" }.repeat(20);
    scratch.attach(&mut driver, Some(&server));
    assert_eq!(footer(&mut driver), &server[..8]);

    // A rollback must replace the previous server's label too.
    scratch.attach(&mut driver, Some(client));
    assert_eq!(footer(&mut driver), &client[..8]);
}

#[test]
fn footer_version_unknown_server_is_not_replaced_by_the_client_sha() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    for sha in [None, Some("invalid")] {
        scratch.attach(&mut driver, sha);
        assert_eq!(footer(&mut driver), "unknown");
    }
}

#[test]
fn footer_version_server_sha_fits_the_minimum_sidebar_width() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    receive_width(&mut driver, 20);
    let client = env!("BUS_BUILD_SHA");
    let server = if client.starts_with("ab") { "cd" } else { "ab" }.repeat(20);
    scratch.attach(&mut driver, Some(&server));
    assert_eq!(footer(&mut driver), &server[..8]);
}
