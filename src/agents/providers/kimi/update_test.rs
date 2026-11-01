//! Synthetic executable and private homes; never runs a model or uses real credentials.
use super::*;
use std::{ffi::OsString, os::unix::fs::PermissionsExt, sync::MutexGuard};

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    executable: PathBuf,
    hooks: HookContext,
    previous: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let lock = crate::utils::config::test_config_env_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = crate::utils::test_temp::unique_temp_path("kimi-update-synthetic");
        let source = root.join("source");
        let executable = source.join("bin/kimi");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(source.join("config.toml"), "").unwrap();
        std::fs::write(
            &executable,
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 2.4.0 ;;\n--help) echo '--session --agent-file --yolo' ;;\n*) exit 91 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let previous = ["KIMI_CODE_HOME", "KIMI_CODE_NO_AUTO_UPDATE", "BUS_YOLO"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("KIMI_CODE_HOME", &source);
        std::env::set_var("KIMI_CODE_NO_AUTO_UPDATE", "0");
        std::env::remove_var("BUS_YOLO");
        Self {
            hooks: HookContext {
                binary: std::env::current_exe().unwrap(),
                spool: root.join("callbacks/launch"),
                launch_id: "launch".into(),
            },
            root,
            source,
            executable,
            previous,
            _lock: lock,
        }
    }

    fn prepare(&self) -> PreparedLaunch {
        prepare(
            &LaunchSpec {
                provider: ProviderKind::Kimi,
                cwd: self.root.to_string_lossy().into_owned(),
                extra_args: "--model synthetic --yolo".into(),
                consent_project_hooks: false,
                hooks: self.hooks.clone(),
            },
            |_| Ok(()),
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn kimi_update_check_is_disabled_on_fresh_launch() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare();
    assert_eq!(
        prepared
            .env
            .get("KIMI_CODE_NO_AUTO_UPDATE")
            .map(String::as_str),
        Some("1")
    );
}

#[test]
fn kimi_update_check_is_disabled_in_launch_and_resume_argv() {
    let fixture = Fixture::new();
    for args in [vec![], resume_args("synthetic-session")] {
        let mut argv = vec![fixture.executable.to_string_lossy().into_owned()];
        argv.extend(args);
        let bound = bind_home(argv.clone(), &fixture.root.join("private-home")).unwrap();
        assert_eq!(bound[0], "/usr/bin/env");
        assert!(
            bound.iter().any(|arg| arg == "KIMI_CODE_NO_AUTO_UPDATE=1"),
            "{bound:?}"
        );
        assert!(bound.ends_with(&argv));
    }
}

#[test]
fn kimi_update_check_is_disabled_when_rebinding_a_preexisting_home() {
    let fixture = Fixture::new();
    // Write the pre-change capture format, with no persisted update switch.
    let home = private_home(&fixture.source, &fixture.root, &fixture.hooks).unwrap();
    for (name, value) in [
        (
            HOME_FILE,
            serde_json::to_value(home_record(&home, &fixture.source).unwrap()).unwrap(),
        ),
        (
            EXECUTABLE_FILE,
            serde_json::to_value((&fixture.executable, stamp(&fixture.executable).unwrap()))
                .unwrap(),
        ),
    ] {
        hook_json::atomic_write(
            &fixture.hooks.spool.join(name),
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    super::super::resume::capture_args(&fixture.hooks, &fixture.root).unwrap();
    let restored = environment(&fixture.hooks).unwrap();
    assert_eq!(
        restored.get("KIMI_CODE_NO_AUTO_UPDATE").map(String::as_str),
        Some("1")
    );
    assert_eq!(saved_home(&fixture.hooks).unwrap(), home);
    assert_eq!(
        std::fs::read_to_string(fixture.source.join("config.toml")).unwrap(),
        ""
    );
}

#[test]
fn kimi_update_switch_is_not_saved_as_a_user_launch_option() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare();
    let expected = ["--model", "synthetic", "--yolo"];
    assert_eq!(prepared.args, expected);
    assert_eq!(
        common::saved_options(ProviderKind::Kimi, &fixture.hooks.spool).unwrap(),
        expected
    );
}
