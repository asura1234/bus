use super::*;
use std::os::unix::{
    fs::{DirBuilderExt, PermissionsExt},
    net::UnixListener,
};

const TEST: &str = "agents::providers::codex::native::launcher::tests::codex_native_shell_helper_inherits_environment_and_fences_runtime_lifetime";
const ROOT_ENV: &str = "BUS_CODEX_HELPER_TEST_ROOT";

fn initialize_peer(
    listener: UnixListener,
    root: std::path::PathBuf,
    command: String,
    trusted: bool,
) -> std::thread::JoinHandle<()> {
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
        // The fixture endpoint is separate from the fake backend. Do not let
        // an immediate refusal retire that child before its startup evidence.
        let cwd_written =
            || fs::read_to_string(root.join("backend-cwd")).is_ok_and(|cwd| cwd.ends_with('\n'));
        while !cwd_written() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(cwd_written());
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
        if let Ok(frame) = peer.read() {
            let request: serde_json::Value =
                serde_json::from_str(frame.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "hooks/list");
            assert_eq!(
                request["params"]["cwds"][0],
                fs::read_to_string(root.join("expected-project")).unwrap(),
                "the hook preflight must inspect Codex's effective project"
            );
            let hooks: Vec<_> = ["sessionStart", "userPromptSubmit", "stop"].into_iter().map(|event| {
                serde_json::json!({"eventName":event,"source":"project","handlerType":"command",
                    "command":command,"enabled":true,"trustStatus":if trusted {"trusted"} else {"untrusted"}})
            }).collect();
            peer.send(tungstenite::Message::Text(
                serde_json::json!({"id":request["id"],"result":{"data":[{
                    "cwd": request["params"]["cwds"][0], "hooks":hooks,"errors":[],"warnings":[]
                }]}})
                .to_string()
                .into(),
            ))
            .unwrap();
        }
    })
}

#[test]
fn codex_native_shell_helper_inherits_environment_and_fences_runtime_lifetime() {
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        let root = std::path::PathBuf::from(root);
        let directory = fs::read_to_string(root.join("runtime-directory")).unwrap();
        let mut args = vec![
            directory,
            root.join("capture").to_string_lossy().into_owned(),
            root.join("codex").to_string_lossy().into_owned(),
            "--no-daemon".into(),
            "-c".into(),
            "note=\"--sandbox=read-only\"".into(),
        ];
        if std::env::var_os("BUS_CODEX_HELPER_TEST_CD").is_some() {
            args.extend(["--cd".into(), "../target-project".into()]);
        }
        run(&args).unwrap();
        unreachable!("the helper must exec its TUI");
    }
    exercise_shell_helper(true, false);
}

#[test]
fn codex_native_helper_uses_standalone_until_hooks_are_trusted() {
    exercise_shell_helper(false, false);
}

#[test]
fn codex_native_helper_cd_uses_effective_project_for_backend_and_hook_preflight() {
    exercise_shell_helper(true, true);
}

#[test]
fn codex_native_attachment_requires_current_enabled_trusted_callback_hooks() {
    let cwd = Path::new("/private/project");
    let capture = Path::new("/private/bus/callbacks/launch");
    let command = crate::agents::providers::hook_json::HookContract::for_provider(
        crate::agents::providers::ProviderKind::Codex,
    )
    .command(&capture.parent().unwrap().join("codex-hook-bridge"));
    let hooks: Vec<_> = ["sessionStart", "userPromptSubmit", "stop"]
        .into_iter()
        .map(|event| {
            serde_json::json!({"eventName":event,"source":"project",
            "handlerType":"command","command":command,"enabled":true,"trustStatus":"trusted"})
        })
        .collect();
    let trusted = serde_json::json!({"data":[{"cwd":cwd,"hooks":hooks,"errors":[],"warnings":[]}]});
    assert!(trusted_bus_hooks(&trusted, capture, cwd));
    for case in [
        "untrusted-project",
        "missing-event",
        "disabled",
        "modified",
        "wrong-command",
        "wrong-source",
        "wrong-handler",
        "errors",
        "wrong-cwd",
        "malformed",
        "foreign-hook",
    ] {
        let mut result = trusted.clone();
        match case {
            "untrusted-project" => result["data"][0]["hooks"] = serde_json::json!([]),
            "missing-event" => {
                result["data"][0]["hooks"].as_array_mut().unwrap().pop();
            }
            "disabled" => result["data"][0]["hooks"][0]["enabled"] = false.into(),
            "modified" => result["data"][0]["hooks"][0]["trustStatus"] = "modified".into(),
            "wrong-command" => {
                result["data"][0]["hooks"][0]["command"] =
                    "/other/bus --bus-callback codex-hook".into()
            }
            "wrong-source" => result["data"][0]["hooks"][0]["source"] = "user".into(),
            "wrong-handler" => result["data"][0]["hooks"][0]["handlerType"] = "prompt".into(),
            "errors" => {
                result["data"][0]["errors"] = serde_json::json!([{"message":"invalid hook"}])
            }
            "wrong-cwd" => result["data"][0]["cwd"] = "/other/project".into(),
            "malformed" => result["data"] = serde_json::Value::Null,
            "foreign-hook" => result["data"][0]["hooks"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"enabled":true,"trustStatus":"untrusted"})),
            _ => unreachable!(),
        }
        assert!(!trusted_bus_hooks(&result, capture, cwd), "{case}");
    }
}

fn exercise_shell_helper(trusted: bool, override_cd: bool) {
    let root = crate::utils::test_temp::unique_temp_path("codex-shell-helper");
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    for name in ["capture", "home", "shell-cwd", "target-project"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let project = root
        .join(if override_cd {
            "target-project"
        } else {
            "shell-cwd"
        })
        .canonicalize()
        .unwrap();
    fs::write(root.join("expected-project"), project.to_str().unwrap()).unwrap();
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
    let command = crate::agents::providers::hook_json::HookContract::for_provider(
        crate::agents::providers::ProviderKind::Codex,
    )
    .command(&root.join("codex-hook-bridge"));
    let peer = initialize_peer(
        UnixListener::bind(directory.join("s")).unwrap(),
        root.clone(),
        command,
        trusted,
    );
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", TEST, "--nocapture"]);
    child.env_clear();
    if override_cd {
        child.env("BUS_CODEX_HELPER_TEST_CD", "1");
    }
    let output = child
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
            if path == "backend-cwd" {
                project.clone()
            } else {
                root.join("shell-cwd").canonicalize().unwrap()
            }
        );
    }
    let tui = fs::read_to_string(root.join("tui-argv")).unwrap();
    assert_eq!(
        tui.starts_with(&format!("--remote\nunix://{}/s\n", directory.display())),
        trusted,
        "{tui}"
    );
    assert_eq!(tui.lines().any(|v| v == "--no-daemon"), !trusted);
    assert_eq!(
        fs::read_to_string(directory.join("mode")).unwrap(),
        if trusted { "native" } else { "standalone" }
    );
    let lock_path = root.join("capture/codex-native-writer.lock");
    if trusted {
        assert!(
            matches!(acquire_writer_lock(&lock_path),Err(e) if e.kind()==io::ErrorKind::WouldBlock),
            "backend must retain the launch lock after TUI exec/exit"
        );
    } else {
        assert!(
            acquire_writer_lock(&lock_path).is_ok(),
            "untrusted backend must retire before standalone exits"
        );
    }
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

#[test]
fn codex_native_writer_lock_failure_keeps_the_open_error() {
    use std::error::Error;
    let missing = crate::utils::test_temp::unique_temp_path("codex-writer-lock-missing");
    let error = super::writer_lock(&missing).unwrap_err();
    let message = error.to_string();
    assert_eq!(
        error.kind(),
        io::ErrorKind::NotFound,
        "a missing capture directory must keep the open error, not a relabeled kind: {message}"
    );
    assert!(
        message.contains("codex-native-writer.lock") && message.contains("No such file"),
        "the lock path and the open error must survive: {message}"
    );
    assert!(
        error.source().is_some(),
        "the original open error must stay chained: {message}"
    );
}
