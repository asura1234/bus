use super::*;
use std::os::unix::{
    fs::{DirBuilderExt, PermissionsExt},
    net::UnixListener,
};

const TEST: &str = "agents::providers::codex::native::launcher::tests::codex_native_shell_helper_inherits_environment_and_fences_runtime_lifetime";
const ROOT_ENV: &str = "BUS_CODEX_HELPER_TEST_ROOT";

fn initialize_peer(listener: UnixListener) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(7);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                other => panic!("helper failed to attach: {other:?}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut peer = tungstenite::accept(stream).unwrap();
        let request: serde_json::Value =
            serde_json::from_str(peer.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(request["method"], "initialize");
        peer.send(tungstenite::Message::Text(
            serde_json::json!({"id":request["id"],"result":{"userAgent":"fixture"}})
                .to_string()
                .into(),
        ))
        .unwrap();
        let initialized: serde_json::Value =
            serde_json::from_str(peer.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(initialized["method"], "initialized");
    })
}

#[test]
fn codex_native_shell_helper_inherits_environment_and_fences_runtime_lifetime() {
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        let root = std::path::PathBuf::from(root);
        let directory = fs::read_to_string(root.join("runtime-directory")).unwrap();
        run(&[
            directory,
            root.join("capture").to_string_lossy().into_owned(),
            root.join("codex").to_string_lossy().into_owned(),
            "--no-daemon".into(),
            "-c".into(),
            "note=\"--sandbox=read-only\"".into(),
        ])
        .unwrap();
        unreachable!("the helper must exec its TUI");
    }
    let root = crate::utils::test_temp::unique_temp_path("codex-shell-helper");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    for name in ["capture", "home", "shell-cwd"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let program = root.join("codex");
    fs::write(
        &program,
        r#"#!/bin/sh
: "${BUS_CODEX_HELPER_TEST_ROOT:?private fixture root required}"
if [ "$1" = app-server ]; then
    printf '%s' "${BUS_CODEX_TEST_EXPORT:-unset}" > "$BUS_CODEX_HELPER_TEST_ROOT/backend-env"
    /bin/pwd > "$BUS_CODEX_HELPER_TEST_ROOT/backend-cwd"
    exec /bin/sleep 60
fi
printf '%s' "${BUS_CODEX_TEST_EXPORT:-unset}" > "$BUS_CODEX_HELPER_TEST_ROOT/tui-env"
/bin/pwd > "$BUS_CODEX_HELPER_TEST_ROOT/tui-cwd"
printf '%s\n' "$@" > "$BUS_CODEX_HELPER_TEST_ROOT/tui-argv"
"#,
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let environment = vec![
        ("BUS_CODEX_NATIVE".into(), "1".into()),
        (
            "BUS_CALLBACK_DIR".into(),
            root.join("capture").to_string_lossy().into_owned(),
        ),
    ];
    let runtime = std::sync::Arc::new(std::sync::Mutex::new(
        runtime::Runtime::from_environment(&environment).unwrap(),
    ));
    let argv = [
        program.to_string_lossy().into_owned(),
        "--no-daemon".into(),
        "-c".into(),
        "note=\"--sandbox=read-only\"".into(),
    ];
    let launch = runtime.lock().unwrap().launch(&argv).unwrap();
    assert_eq!(launch[1], "--bus-codex-runtime");
    let directory = std::path::PathBuf::from(&launch[2]);
    assert!(
        !root.join("backend-env").exists(),
        "Bus must not launch from the pre-shell environment"
    );
    fs::write(root.join("runtime-directory"), directory.to_str().unwrap()).unwrap();
    let peer = initialize_peer(UnixListener::bind(directory.join("s")).unwrap());
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env_clear()
        .env(ROOT_ENV, &root)
        .env("BUS_CODEX_TEST_EXPORT", "initialized-shell")
        .env("HOME", root.join("home"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root.join("shell-cwd"))
        .output()
        .unwrap();
    peer.join().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.join("backend-env").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(root.join("backend-env")).unwrap(),
        "initialized-shell"
    );
    assert_eq!(
        fs::read_to_string(root.join("tui-env")).unwrap(),
        "initialized-shell"
    );
    for path in ["backend-cwd", "tui-cwd"] {
        assert_eq!(
            std::path::Path::new(fs::read_to_string(root.join(path)).unwrap().trim())
                .canonicalize()
                .unwrap(),
            root.join("shell-cwd").canonicalize().unwrap()
        );
    }
    let tui = fs::read_to_string(root.join("tui-argv")).unwrap();
    assert!(
        tui.starts_with(&format!("--remote\nunix://{}/s\n", directory.display())),
        "{tui}"
    );
    assert!(!tui.lines().any(|v| v == "--no-daemon"));
    let lock_path = root.join("capture/codex-native-writer.lock");
    assert!(
        matches!(acquire_writer_lock(&lock_path),Err(e) if e.kind()==io::ErrorKind::WouldBlock),
        "backend must retain the launch lock after TUI exec/exit"
    );
    drop(runtime);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if acquire_writer_lock(&lock_path).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Bus lifetime closure must retire its writer"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!directory.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn codex_native_helper_backend_start_failure_retains_standalone_input() {
    const ROOT: &str = "BUS_CODEX_FALLBACK_TEST_ROOT";
    const TEST: &str = "agents::providers::codex::native::launcher::tests::codex_native_helper_backend_start_failure_retains_standalone_input";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = std::path::PathBuf::from(root);
        run(&[
            fs::read_to_string(root.join("runtime-directory")).unwrap(),
            root.join("capture").to_string_lossy().into_owned(),
            root.join("codex").to_string_lossy().into_owned(),
            "--no-daemon".into(),
        ])
        .unwrap();
        unreachable!("helper replaces itself with the configured TUI");
    }
    let root = crate::utils::test_temp::unique_temp_path("codex-helper-fallback");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    fs::create_dir(root.join("capture")).unwrap();
    fs::write(
        root.join("codex"),
        r#"#!/bin/sh
if [ "$1" = app-server ]; then exit 1; fi
printf '%s\n' "$@" > "$BUS_CODEX_FALLBACK_TEST_ROOT/tui-argv"
"#,
    )
    .unwrap();
    fs::set_permissions(root.join("codex"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut provider = runtime::Runtime::from_environment(&[
        ("BUS_CODEX_NATIVE".into(), "1".into()),
        (
            "BUS_CALLBACK_DIR".into(),
            root.join("capture").to_string_lossy().into_owned(),
        ),
    ])
    .unwrap();
    let launch = provider
        .launch(&[
            root.join("codex").to_string_lossy().into_owned(),
            "--no-daemon".into(),
        ])
        .unwrap();
    fs::write(root.join("runtime-directory"), &launch[2]).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env_clear()
        .env(ROOT, &root)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("tui-argv")).unwrap(),
        "--no-daemon\n"
    );
    assert!(
        provider
            .prompt(
                "thread",
                "standalone text",
                false,
                "fallback",
                Instant::now() + Duration::from_secs(1),
                None
            )
            .is_none(),
        "only the explicit standalone launch permits terminal fallback"
    );
    drop(provider);
    fs::remove_dir_all(root).unwrap();
}
