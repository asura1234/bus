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
fn kimi_yolo_refuses_auto_combined_with_legacy_yolo_before_saving() {
    let fixture = Fixture::new();
    std::env::set_var("BUS_YOLO", "1");
    let prepared = prepare(
        &LaunchSpec {
            provider: ProviderKind::Kimi,
            cwd: fixture.root.to_string_lossy().into_owned(),
            extra_args: "--auto --yolo".into(),
            consent_project_hooks: false,
            hooks: fixture.hooks.clone(),
        },
        |_| Ok(()),
    );
    assert!(
        prepared.is_err(),
        "Bus yolo saved --auto --yolo, which a normal resume rejects"
    );
    assert!(
        !fixture.hooks.spool.join("launch-options.json").exists(),
        "rejected combination was written to launch options"
    );
}

#[test]
fn kimi_yolo_hidden_flags_fail_before_save() {
    let mut failures = Vec::new();
    for input in [
        "-Cy --auto",
        "--auto -Cy",
        "-C --auto --yolo",
        "-C /tmp",
        "--resume --auto --yolo",
        "--resume sess --auto --yolo",
        "-r --auto -y",
        "-r sess --auto -y",
        "-Qy --auto",
        "-yrm --auto",
        "-yrm synthetic",
    ] {
        for enabled in [false, true] {
            let fixture = Fixture::new();
            if enabled {
                std::env::set_var("BUS_YOLO", "1");
            }
            let prepared = prepare(
                &LaunchSpec {
                    provider: ProviderKind::Kimi,
                    cwd: fixture.root.to_string_lossy().into_owned(),
                    extra_args: input.into(),
                    consent_project_hooks: false,
                    hooks: fixture.hooks.clone(),
                },
                |_| Ok(()),
            );
            if let Ok(prepared) = prepared {
                let saved =
                    common::saved_options(ProviderKind::Kimi, &fixture.hooks.spool).unwrap();
                std::env::remove_var("BUS_YOLO");
                let mut replay = resumable_options(&saved);
                common::apply_run_permissions(ProviderKind::Kimi, &mut replay);
                let resumed = validate_run_permissions(&replay);
                failures.push(format!(
                    "prepare kept input {input:?} (bus yolo {enabled}); effective {:?}; saved {saved:?}; normal replay {replay:?}; resume validate {}",
                    prepared.args,
                    if resumed.is_err() { "err" } else { "ok" }
                ));
            } else if fixture.hooks.spool.join("launch-options.json").exists() {
                failures.push(format!(
                    "rejected {input:?} but wrote launch options (bus yolo {enabled})"
                ));
            }
        }
    }
    for (enabled, input, effective) in [
        (true, "-Cy", vec!["--auto", "-C"]),
        (true, "-yC", vec!["--auto", "-C"]),
        (true, "-Qy", vec!["--auto", "-Q"]),
        (true, "-yr", vec!["--auto", "-r"]),
        (true, "-yrm", vec!["--auto", "-rm"]),
        (true, "-yr sess", vec!["--auto", "-r", "sess"]),
        (true, "-ry", vec!["--auto", "-ry"]),
        (false, "-ry --auto", vec!["-ry", "--auto"]),
        (false, "-C --auto", vec!["-C", "--auto"]),
        (false, "--auto -r sess", vec!["--auto", "-r", "sess"]),
        (true, "-ry --auto", vec!["-ry", "--auto"]),
        (
            false,
            "--resume sess --model synthetic",
            vec!["--resume", "sess", "--model", "synthetic"],
        ),
    ] {
        let fixture = Fixture::new();
        if enabled {
            std::env::set_var("BUS_YOLO", "1");
        }
        let prepared = prepare(
            &LaunchSpec {
                provider: ProviderKind::Kimi,
                cwd: fixture.root.to_string_lossy().into_owned(),
                extra_args: input.into(),
                consent_project_hooks: false,
                hooks: fixture.hooks.clone(),
            },
            |_| Ok(()),
        );
        let saved = common::saved_options(ProviderKind::Kimi, &fixture.hooks.spool).ok();
        let Ok(prepared) = prepared else {
            failures.push(format!(
                "replayable input {input:?} was rejected (bus yolo {enabled})"
            ));
            continue;
        };
        if prepared.args != effective {
            failures.push(format!(
                "input {input:?} (bus yolo {enabled}) effective {:?}, want {effective:?}",
                prepared.args
            ));
        }
        let raw = common::parse_path_tokens(input).unwrap();
        if saved.as_deref() != Some(raw.as_slice()) {
            failures.push(format!(
                "input {input:?} saved {saved:?}, want the original tokens"
            ));
        }
        std::env::remove_var("BUS_YOLO");
        let mut replay = resumable_options(&raw);
        common::apply_run_permissions(ProviderKind::Kimi, &mut replay);
        if validate_run_permissions(&replay).is_err() || replay != raw {
            failures.push(format!(
                "normal resume changed replayable input {input:?} into {replay:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn kimi_yolo_cluster_with_session_leaves_no_legacy_yolo_alias() {
    let fixture = Fixture::new();
    std::env::set_var("BUS_YOLO", "1");
    let prepared = prepare(
        &LaunchSpec {
            provider: ProviderKind::Kimi,
            cwd: fixture.root.to_string_lossy().into_owned(),
            extra_args: "-yS -y".into(),
            consent_project_hooks: false,
            hooks: fixture.hooks.clone(),
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(prepared.args, ["--auto", "-S"]);
    let saved = common::saved_options(ProviderKind::Kimi, &fixture.hooks.spool).unwrap();
    assert_eq!(saved, ["-yS", "-y"]);
    std::env::remove_var("BUS_YOLO");
    let mut replay = common::resumable_options(ProviderKind::Kimi, &saved);
    common::apply_run_permissions(ProviderKind::Kimi, &mut replay);
    validate_run_permissions(&replay).unwrap();
    assert_eq!(replay, ["-yS", "-y"]);
}

#[test]
fn kimi_yolo_normalizes_fresh_launch_without_persisting_generated_auto() {
    for (input, yolo, normal) in [
        ("", vec!["--auto"], vec![]),
        ("--auto", vec!["--auto"], vec!["--auto"]),
        ("--yolo", vec!["--auto"], vec!["--yolo"]),
        ("-y", vec!["--auto"], vec!["-y"]),
        ("-yy", vec!["--auto"], vec!["-yy"]),
        ("-ymVALUE", vec!["--auto", "-mVALUE"], vec!["-ymVALUE"]),
        (
            "-ymcustom --yolo",
            vec!["--auto", "-mcustom"],
            vec!["-ymcustom", "--yolo"],
        ),
        (
            "-ym VALUE",
            vec!["--auto", "-m", "VALUE"],
            vec!["-ym", "VALUE"],
        ),
        (
            "--model synthetic --yolo --output-format text",
            vec!["--auto", "--model", "synthetic", "--output-format", "text"],
            vec!["--model", "synthetic", "--yolo", "--output-format", "text"],
        ),
    ] {
        let original = common::parse_path_tokens(input).unwrap();
        for (enabled, expected) in [(true, yolo), (false, normal)] {
            let fixture = Fixture::new();
            if enabled {
                std::env::set_var("BUS_YOLO", "1");
            }
            let prepared = fixture.prepare_args(input);
            assert_eq!(prepared.args, expected, "input={input:?}, yolo={enabled}");
            assert_eq!(
                common::saved_options(ProviderKind::Kimi, &fixture.hooks.spool).unwrap(),
                original,
                "generated permissions replaced saved options"
            );
        }
    }
}
