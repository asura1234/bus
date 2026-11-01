//! All executable identities and projects here are synthetic scratch fixtures.
use super::*;
use crate::agents::providers::{launch as common, ProviderKind};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{ffi::OsString, fs, sync::MutexGuard};
pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) project: PathBuf,
    pub(crate) bin: PathBuf,
    old: Vec<(&'static str, Option<OsString>)>,
    _path: crate::utils::env::PathOverride,
    _lock: MutexGuard<'static, ()>,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        let lock = crate::utils::config::test_config_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = crate::utils::test_temp::unique_temp_path("grok-ad-synthetic");
        let project = root.join("project");
        let bin = root.join("bin");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let old = ["HOME", "BUS_YOLO", "GROK_HOME"]
            .into_iter()
            .map(|k| (k, std::env::var_os(k)))
            .collect();
        std::env::set_var("HOME", root.join("home"));
        let path = crate::utils::env::PathOverride::set(
            std::env::join_paths([
                bin.clone(),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ])
            .unwrap(),
        );
        std::env::remove_var("BUS_YOLO");
        std::env::remove_var("GROK_HOME");
        Self {
            root,
            project,
            bin,
            old,
            _path: path,
            _lock: lock,
        }
    }
    pub(crate) fn executable(&self, path: &Path, identity: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path,format!("#!/bin/sh\n# SYNTHETIC Grok identity fixture; never runs a model.\ncase \"$1\" in\n--version) printf '%s\\n' '{identity}' ;;\n--help) printf '%s\\n' 'Grok Build TUI' 'Usage: grok [OPTIONS] [PROMPT] [COMMAND]' '--resume --session-id --fullscreen --permission-mode --prompt-json' ;;\n*) exit 91 ;;\nesac\n")).unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    pub(crate) fn install(&self) {
        self.executable(&self.bin.join("grok"), "grok 1.0.5 (synthetic)");
    }
    pub(crate) fn spec(&self, extra: &str, consent: bool) -> LaunchSpec {
        LaunchSpec {
            provider: ProviderKind::Grok,
            cwd: self.project.display().to_string(),
            extra_args: extra.into(),
            consent_project_hooks: consent,
            hooks: HookContext {
                binary: self.root.join("bus synthetic"),
                spool: self.root.join("capture"),
                launch_id: "synthetic-launch".into(),
            },
        }
    }
    pub(crate) fn prepare(&self, extra: &str) -> PreparedLaunch {
        common::prepare(&self.spec(extra, true), |hooks| {
            fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
        })
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for (key, value) in &self.old {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn grok_identity_prefers_validated_alias_over_regex_namesake() {
    let f = Fixture::new();
    f.executable(&f.bin.join("grok"), "grok 1.20110708 regex");
    let alias = f.root.join("home/.grok/bin/grok");
    let target = f.root.join("home/.grok/downloads/grok-1.0.5-synthetic");
    f.executable(&target, "grok 1.0.5 (synthetic)");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &alias).unwrap_or_else(|_| {
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
    });
    assert_eq!(resolve_executable().unwrap(), alias);
    let prepared = f.prepare("");
    let output = std::process::Command::new("grok")
        .arg("--version")
        .envs(prepared.env)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "grok 1.0.5 (synthetic)"
    );
}
#[test]
fn grok_identity_falls_back_only_to_verified_build_candidates() {
    let f = Fixture::new();
    f.executable(&f.root.join("home/.grok/bin/grok"), "regex grok 1.0");
    f.install();
    assert_eq!(resolve_executable().unwrap(), f.bin.join("grok"));
    f.executable(&f.bin.join("grok"), "grok 1.0.5");
    assert!(resolve_executable().is_err());
    f.executable(&f.bin.join("grok"), "grok 1.0.5 (synthetic)");
    fs::write(
        f.bin.join("grok"),
        "#!/bin/sh\necho 'grok 1.0.5 (synthetic)'\n",
    )
    .unwrap();
    assert!(
        resolve_executable().is_err(),
        "version alone does not attest Grok Build"
    );
}

#[test]
fn grok_1070_identity_keeps_the_project_hook_contract() {
    let f = Fixture::new();
    f.executable(&f.bin.join("grok"), "grok 1.0.70 (f50904eaee6c)");
    f.prepare("");
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(f.project.join(".grok/hooks/bus.json")).unwrap()).unwrap();
    for event in crate::agents::providers::grok::HOOK_CONTRACT.events {
        assert!(value["hooks"].get(*event).is_some(), "missing {event}");
    }
}
#[test]
fn grok_prepare_persists_original_argv_and_private_environment() {
    let f = Fixture::new();
    f.install();
    let input="--permission-mode default --allow 'Bash(ls*)' --deny 'Bash(rm*)' --no-leader --fullscreen --no-auto-update -- '--single literal prompt'";
    let spec = f.spec(input, true);
    let calls = std::cell::Cell::new(0);
    let prepared = common::prepare(&spec, |h| {
        calls.set(calls.get() + 1);
        assert!(f.project.join(".grok/hooks/bus.json").exists());
        fs::create_dir_all(&h.spool).map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(prepared.args, common::parse_path_tokens(input).unwrap());
    let saved: Vec<String> =
        serde_json::from_slice(&fs::read(spec.hooks.spool.join("launch-options.json")).unwrap())
            .unwrap();
    assert_eq!(saved, common::parse_path_tokens(input).unwrap());
    assert_eq!(prepared.env["BUS_LAUNCH_ID"], "synthetic-launch");
    assert_eq!(
        prepared.env["BUS_CALLBACK_DIR"],
        spec.hooks.spool.display().to_string()
    );
    assert_eq!(
        std::env::split_paths(&prepared.env["PATH"]).next().unwrap(),
        f.bin
    );
    assert_eq!(prepared.env.len(), 4);
    assert!(!f.root.join("home/.grok/config.toml").exists());
}
#[test]
fn grok_prepare_normalizes_adoption_and_prefixes_runtime_before_literal_tail() {
    let f = Fixture::new();
    f.install();
    let prepared =
        f.prepare("--resume=160d1f8b-9023-44b8-9bc7-24333effb185 --model grok-build -- '--single'");
    assert_eq!(
        prepared.adopted_session.as_deref(),
        Some("160d1f8b-9023-44b8-9bc7-24333effb185")
    );
    assert_eq!(
        prepared.args,
        [
            "--resume",
            "160d1f8b-9023-44b8-9bc7-24333effb185",
            "--no-leader",
            "--fullscreen",
            "--no-auto-update",
            "--model",
            "grok-build",
            "--",
            "--single"
        ]
    );
    assert_eq!(
        fs::read_to_string(f.root.join("capture/adopted-session")).unwrap(),
        "160d1f8b-9023-44b8-9bc7-24333effb185"
    );
}
#[test]
fn grok_prepare_rejects_invalid_input_before_initializer_or_hook_writes() {
    let f = Fixture::new();
    f.install();
    for input in ["--single hi", "'unterminated"] {
        assert!(common::prepare(&f.spec(input, true), |_| panic!(
            "invalid argv initialized capture"
        ))
        .is_err());
        assert!(!f.project.join(".grok").exists());
        assert!(!f.root.join("capture").exists());
    }
}

#[test]
fn grok_run_scoped_permissions_do_not_replace_saved_user_choices() {
    let f = Fixture::new();
    f.install();
    std::env::set_var("BUS_YOLO", "1");
    let prepared = f.prepare("--permission-mode default --deny 'Bash(rm*)'");
    assert_eq!(prepared.args.first().unwrap(), "--always-approve");
    let options: Vec<String> =
        serde_json::from_slice(&fs::read(f.root.join("capture/launch-options.json")).unwrap())
            .unwrap();
    assert_eq!(
        options,
        ["--permission-mode", "default", "--deny", "Bash(rm*)"]
    );
    let prepared = f.prepare("--yolo");
    assert!(!prepared.args.iter().any(|a| a == "--always-approve"));
}

#[test]
fn grok_identity_probe_allows_slow_bounded_startup() {
    let f = Fixture::new();
    f.install();
    let executable = f.bin.join("grok");
    fs::write(&executable, "#!/bin/sh\n# Synthetic cold identity startup, never a model.\n/bin/sleep 3\nprintf '%s\\n' 'grok 1.0.5 (synthetic)'\n").unwrap();
    assert_eq!(
        identity_output(&executable, "--version").unwrap().trim(),
        "grok 1.0.5 (synthetic)"
    );
}

#[test]
fn grok_identity_probe_rejects_stalled_output() {
    let f = Fixture::new();
    f.install();
    let executable = f.bin.join("grok");
    fs::write(&executable,"#!/bin/sh\n# SYNTHETIC stalled identity command; no model.\n/bin/sleep 30\nprintf '%s\\n' 'grok 1.0.5 (synthetic)'\n").unwrap();
    let error = identity_output(&executable, "--version").unwrap_err();
    assert!(error.contains("timed out"), "{error}");
}

#[test]
fn grok_identity_probe_rejects_oversized_output() {
    let f = Fixture::new();
    f.install();
    let executable = f.bin.join("grok");
    fs::write(&executable,"#!/bin/sh\n# SYNTHETIC noisy identity command; no model.\n/usr/bin/head -c 70000 /dev/zero\n").unwrap();
    assert!(identity_output(&executable, "--help").is_err());
}
#[test]
fn grok_launch_from_home_never_installs_a_global_grok_hook() {
    // `~/.grok/hooks/*.json` is Grok's always-trusted global hook source; a
    // project install must never land there, even when the launch cwd is HOME.
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut spec = f.spec("", true);
    spec.cwd = home.display().to_string();
    let prepared = common::prepare(&spec, |hooks| {
        fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    let global = home.join(".grok/hooks/bus.json");
    assert!(
        !global.exists(),
        "a launch from HOME wrote the global Grok hook {} (prepare ok: {})",
        global.display(),
        prepared.is_ok()
    );
}

#[test]
fn grok_installed_identity_rejection_reports_why_without_captured_output() {
    let f = Fixture::new();
    f.executable(&f.bin.join("grok"), "PRIVATE_INVALID_IDENTITY");
    let error = resolve_executable().unwrap_err();
    assert!(
        error.contains("rejected") && error.contains("not Grok Build"),
        "{error}"
    );
    assert!(!error.contains("PRIVATE_INVALID_IDENTITY"));
}
#[cfg(unix)]
#[test]
fn grok_launch_never_writes_through_a_symlinked_global_grok_directory() {
    // A dotfiles-managed `~/.grok` may be a symlink to the project's `.grok`;
    // the project file is then Grok's always-trusted global hook source.
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    fs::create_dir_all(f.project.join(".grok/hooks")).unwrap();
    fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(f.project.join(".grok"), home.join(".grok")).unwrap();
    let prepared = common::prepare(&f.spec("", true), |hooks| {
        fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    let global = home.join(".grok/hooks/bus.json");
    assert!(
        !global.exists(),
        "a project launch wrote the global Grok hook through the ~/.grok alias (prepare ok: {})",
        prepared.is_ok()
    );
}
#[test]
fn grok_launch_never_writes_into_the_grok_home_hook_directory() {
    // GROK_HOME relocates Grok's global configuration, including its
    // always-trusted hooks; a project inside it must not receive Bus hooks.
    let f = Fixture::new();
    f.install();
    let previous = std::env::var_os("GROK_HOME");
    std::env::set_var("GROK_HOME", f.project.join(".grok"));
    let prepared = common::prepare(&f.spec("", true), |hooks| {
        fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    match previous {
        Some(value) => std::env::set_var("GROK_HOME", value),
        None => std::env::remove_var("GROK_HOME"),
    }
    let global = f.project.join(".grok/hooks/bus.json");
    assert!(
        !global.exists(),
        "a project launch wrote $GROK_HOME/hooks/bus.json (prepare ok: {})",
        prepared.is_ok()
    );
}

#[cfg(unix)]
#[test]
fn grok_setup_rechecks_project_ancestors_after_identity_queries() {
    let f = Fixture::new();
    f.install();
    let project_config = f.project.join(".grok");
    let moved_config = f.project.join(".grok-before-identity");
    let global_config = f.root.join("home/.grok");
    fs::create_dir_all(project_config.join("hooks")).unwrap();
    fs::create_dir_all(global_config.join("hooks")).unwrap();
    let global_hook = global_config.join("hooks/bus.json");
    let original = br#"{"custom":"global-hook-must-stay-unchanged"}"#;
    fs::write(&global_hook, original).unwrap();
    let quote = |path: &Path| crate::platform::remote_reattach_program(&path.to_string_lossy());
    // Deterministically model a concurrent project-directory replacement after
    // containment validation, while the metadata-only fake is being queried.
    // The child only swaps project paths; Bus performs the hook write itself.
    let executable = f.bin.join("grok");
    let script = fs::read_to_string(&executable).unwrap();
    let replacement = format!(
        "--version) /bin/mv {} {}; /bin/ln -s {} {}; ",
        quote(&project_config),
        quote(&moved_config),
        quote(&global_config),
        quote(&project_config),
    );
    fs::write(&executable, script.replacen("--version) ", &replacement, 1)).unwrap();
    let initialized = std::cell::Cell::new(false);
    let result = common::prepare(&f.spec("", true), |hooks| {
        initialized.set(true);
        fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    assert!(fs::symlink_metadata(&project_config)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&global_hook).unwrap(), original,
        "Bus changed global hook bytes after the project ancestor was replaced during identity validation");
    assert!(
        result.is_err(),
        "swapped project ancestor was accepted: {result:?}"
    );
    assert!(
        !initialized.get(),
        "unsafe project ancestor initialized capture"
    );
}
#[cfg(target_os = "macos")]
#[test]
fn grok_launch_from_a_firmlink_spelling_of_home_never_installs_a_global_grok_hook() {
    // APFS firmlinks expose the same directory under /System/Volumes/Data;
    // realpath keeps that spelling, so path prefixes cannot prove separation.
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    fs::create_dir_all(&home).unwrap();
    let firmlink = PathBuf::from(format!(
        "/System/Volumes/Data{}",
        home.canonicalize().unwrap().display()
    ));
    assert!(
        firmlink.is_dir(),
        "{} is not a firmlink alias",
        firmlink.display()
    );
    let mut spec = f.spec("", true);
    spec.cwd = firmlink.display().to_string();
    let prepared = common::prepare(&spec, |hooks| {
        fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    let global = home.join(".grok/hooks/bus.json");
    assert!(
        !global.exists(),
        "a launch from the firmlink spelling of HOME wrote {} (prepare ok: {})",
        global.display(),
        prepared.is_ok()
    );
}
#[test]
fn grok_launch_never_writes_into_a_directory_registered_in_the_global_hooks_paths_file() {
    // Grok's `$GROK_HOME/hooks-paths` registry lists absolute global hook
    // sources one per line; a registered project hooks directory is global.
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    let hooks = f.project.join(".grok/hooks");
    fs::create_dir_all(home.join(".grok")).unwrap();
    fs::create_dir_all(&hooks).unwrap();
    fs::write(
        home.join(".grok/hooks-paths"),
        format!("{}\n", hooks.canonicalize().unwrap().display()),
    )
    .unwrap();
    let prepared = common::prepare(&f.spec("", true), |hooks| {
        fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    assert!(
        !hooks.join("bus.json").exists(),
        "a project launch wrote into a hooks-paths registered directory (prepare ok: {})",
        prepared.is_ok()
    );
}

#[test]
fn grok_launch_and_resume_environment_bind_the_validated_home() {
    let f = Fixture::new();
    f.install();
    let prepared = f.prepare("");
    let expected = f
        .root
        .canonicalize()
        .unwrap()
        .join("home/.grok")
        .to_string_lossy()
        .into_owned();
    assert_eq!(prepared.env.get("GROK_HOME"), Some(&expected));
    std::env::set_var("GROK_HOME", f.project.join(".grok"));
    let spec = f.spec("", true);
    crate::agents::providers::grok::resume::capture_args(&spec.hooks, &f.project).unwrap();
    assert_eq!(
        environment(&spec.hooks).unwrap().get("GROK_HOME"),
        Some(&expected)
    );
}
#[test]
fn grok_launch_with_ordinary_global_hooks_and_registry_installs_only_the_project_hook() {
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    let global = home.join(".grok");
    let shared = f.root.join("shared-hooks");
    fs::create_dir_all(global.join("hooks")).unwrap();
    fs::create_dir_all(&shared).unwrap();
    let user_hook = br#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#;
    fs::write(global.join("hooks/user.json"), user_hook).unwrap();
    fs::write(shared.join("team.json"), user_hook).unwrap();
    let registry = format!(
        "{}\nrelative/ignored\n",
        shared.canonicalize().unwrap().display()
    );
    fs::write(global.join("hooks-paths"), &registry).unwrap();
    let prepared = f.prepare("");
    assert!(f.project.join(".grok/hooks/bus.json").is_file());
    assert_eq!(fs::read(global.join("hooks/user.json")).unwrap(), user_hook);
    assert_eq!(fs::read(shared.join("team.json")).unwrap(), user_hook);
    assert_eq!(
        fs::read_to_string(global.join("hooks-paths")).unwrap(),
        registry
    );
    assert_eq!(
        fs::read_dir(global.join("hooks")).unwrap().count(),
        1,
        "Bus added a file to the global hooks directory"
    );
    assert_eq!(
        prepared.env.get("GROK_HOME").map(PathBuf::from),
        Some(global.canonicalize().unwrap())
    );
}

#[cfg(unix)]
#[test]
fn grok_ordinary_relocated_home_launch_and_resume_keep_global_hooks_unchanged() {
    for route in ["absolute", "relative", "symlinked"] {
        let f = Fixture::new();
        f.install();
        let global = f.root.join("relocated-global");
        fs::create_dir_all(global.join("hooks")).unwrap();
        let sentinel = br#"{"user":"preserved"}"#;
        fs::write(global.join("hooks/user.json"), sentinel).unwrap();
        let configured = match route {
            "relative" => PathBuf::from("../relocated-global"),
            "symlinked" => {
                let alias = f.root.join("global-alias");
                std::os::unix::fs::symlink(&global, &alias).unwrap();
                alias
            }
            _ => global.clone(),
        };
        std::env::set_var("GROK_HOME", &configured);
        let prepared = f.prepare("");
        let expected = global.canonicalize().unwrap().to_str().unwrap().to_owned();
        assert_eq!(prepared.env.get("GROK_HOME"), Some(&expected), "{route}");
        assert!(f.project.join(".grok/hooks/bus.json").is_file(), "{route}");
        // Restart under a changed server environment must retain the selected
        // global home while rebinding only the existing project hook.
        std::env::set_var("GROK_HOME", f.project.join(".grok"));
        let mut spec = f.spec("", true);
        spec.hooks.binary = f.root.join("replacement-bus");
        crate::agents::providers::grok::resume::capture_args(&spec.hooks, &f.project).unwrap();
        assert_eq!(
            environment(&spec.hooks).unwrap().get("GROK_HOME"),
            Some(&expected)
        );
        assert_eq!(
            fs::read(global.join("hooks/user.json")).unwrap(),
            sentinel,
            "{route}"
        );
        assert_eq!(
            fs::read_dir(global.join("hooks")).unwrap().count(),
            1,
            "{route}"
        );
        assert!(!global.join("hooks/bus.json").exists(), "{route}");
    }
}

#[test]
fn grok_fixture_path_is_private_to_the_calling_thread() {
    let inherited = std::env::var_os("PATH");
    let fixture = Fixture::new();
    fixture.install();
    let sibling_path = std::thread::spawn(|| std::env::var_os("PATH"))
        .join()
        .unwrap();
    assert_eq!(
        sibling_path, inherited,
        "a Grok fixture changed another thread's PATH"
    );
    assert_eq!(resolve_executable().unwrap(), fixture.bin.join("grok"));
}
