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
        std::fs::create_dir_all(&root).unwrap();
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
    for y in 0..40 {
        for x in 28..120 {
            // The version must stay out of the main view and composer.
            assert_ne!(buffer[(x, y)].symbol(), "·");
        }
    }
    row.trim_end().to_owned()
}

#[test]
fn footer_version_tracks_replacement_server_and_matching_build() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    let client = env!("BUS_BUILD_SHA");
    let short = client.get(..8).unwrap();
    scratch.attach(&mut driver, Some(client));
    assert_eq!(footer(&mut driver), env!("BUS_BUILD_SHORT_SHA"));

    let other = if client.starts_with("ab") { "cd" } else { "ab" }.repeat(20);
    scratch.attach(&mut driver, Some(&other));
    assert_eq!(
        footer(&mut driver),
        format!("{} · TUI {short}", &other[..8])
    );

    // A rollback clears the mismatch instead of retaining the replacement SHA.
    scratch.attach(&mut driver, Some(client));
    assert_eq!(footer(&mut driver), env!("BUS_BUILD_SHORT_SHA"));
}

#[test]
fn footer_version_compares_full_shas_not_just_the_visible_prefix() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    let client = env!("BUS_BUILD_SHA");
    let mut other = client.to_owned();
    other.replace_range(39.., if client.ends_with('a') { "b" } else { "a" });
    scratch.attach(&mut driver, Some(&other));
    assert_eq!(
        footer(&mut driver),
        format!("{} · TUI {}", &other[..8], &client[..8])
    );
}

#[test]
fn footer_version_does_not_present_the_client_as_an_unknown_server() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    for sha in [Some(env!("BUS_BUILD_SHA")), None, Some("invalid")] {
        scratch.attach(&mut driver, sha);
        let expected = if sha == Some(env!("BUS_BUILD_SHA")) {
            env!("BUS_BUILD_SHORT_SHA").to_owned()
        } else {
            format!("unknown · TUI {}", &env!("BUS_BUILD_SHA")[..8])
        };
        assert_eq!(footer(&mut driver), expected);
    }
}

#[test]
fn footer_version_keeps_both_builds_visible_at_minimum_sidebar_width() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let scratch = Scratch::new();
    let (mut driver, _) = driver(120);
    receive_width(&mut driver, 20);
    let client = env!("BUS_BUILD_SHA");
    let other = if client.starts_with("ab") { "cd" } else { "ab" }.repeat(20);
    scratch.attach(&mut driver, Some(&other));
    assert_eq!(
        footer(&mut driver),
        format!("{} · TUI {}", &other[..5], &client[..5])
    );
}
