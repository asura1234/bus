//! Synthetic executable and private homes; never runs a model or uses real credentials.
use super::*;
use std::{ffi::OsString, os::unix::fs::PermissionsExt, sync::MutexGuard};

#[path = "resume_home_test.rs"]
pub(crate) mod resume_home_test;

pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    source: PathBuf,
    executable: PathBuf,
    pub(crate) hooks: HookContext,
    previous: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
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
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 2.4.0 ;;\n--help) echo '--session --agent-file --yolo --auto' ;;\n*) exit 91 ;;\nesac\n",
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

    pub(crate) fn prepare(&self) -> PreparedLaunch {
        self.prepare_args("--model synthetic --yolo")
    }

    fn prepare_args(&self, extra_args: &str) -> PreparedLaunch {
        prepare(
            &LaunchSpec {
                provider: ProviderKind::Kimi,
                cwd: self.root.to_string_lossy().into_owned(),
                extra_args: extra_args.into(),
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
    let home = private_home(&fixture.source, &fixture.hooks).unwrap();
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

#[test]
fn kimi_yolo_normalizes_fresh_launch_without_persisting_generated_auto() {
    for (input, yolo, normal) in [
        ("", vec!["--auto"], vec![]),
        ("--auto", vec!["--auto"], vec!["--auto"]),
        ("--yolo", vec!["--auto"], vec!["--yolo"]),
        ("-y", vec!["--auto"], vec!["-y"]),
        (
            "--model synthetic --yolo --output-format text",
            vec!["--auto", "--model", "synthetic", "--output-format", "text"],
            vec!["--model", "synthetic", "--yolo", "--output-format", "text"],
        ),
        ("--auto --yolo", vec!["--auto"], vec!["--auto", "--yolo"]),
    ] {
        let original = common::parse_path_tokens(input).unwrap();
        for (enabled, expected) in [(true, yolo), (false, normal)] {
            let fixture = Fixture::new();
            if enabled {
                std::env::set_var("BUS_YOLO", "1");
            }
            let prepared = if enabled && input == "--auto --yolo" {
                fixture.prepare_args(input)
            } else if input == "--auto --yolo" {
                assert!(prepare(
                    &LaunchSpec {
                        provider: ProviderKind::Kimi,
                        cwd: fixture.root.to_string_lossy().into_owned(),
                        extra_args: input.into(),
                        consent_project_hooks: false,
                        hooks: fixture.hooks.clone(),
                    },
                    |_| Ok(()),
                )
                .is_err());
                continue;
            } else {
                fixture.prepare_args(input)
            };
            assert_eq!(prepared.args, expected, "input={input:?}, yolo={enabled}");
            assert_eq!(
                common::saved_options(ProviderKind::Kimi, &fixture.hooks.spool).unwrap(),
                original,
                "generated permissions replaced saved options"
            );
        }
    }
}
