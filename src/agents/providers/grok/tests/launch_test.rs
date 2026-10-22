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
        let old = ["HOME", "PATH", "BUS_YOLO"]
            .into_iter()
            .map(|k| (k, std::env::var_os(k)))
            .collect();
        std::env::set_var("HOME", root.join("home"));
        std::env::set_var(
            "PATH",
            std::env::join_paths([
                bin.clone(),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ])
            .unwrap(),
        );
        std::env::remove_var("BUS_YOLO");
        Self {
            root,
            project,
            bin,
            old,
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
fn grok_ad_identity_prefers_validated_alias_over_regex_namesake() {
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
fn grok_ad_identity_falls_back_only_to_verified_build_candidates() {
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
fn grok_ad_prepare_persists_original_argv_and_private_environment() {
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
    assert_eq!(prepared.env.len(), 3);
    assert!(!f.root.join("home/.grok/config.toml").exists());
}
#[test]
fn grok_ad_prepare_normalizes_adoption_and_prefixes_runtime_before_literal_tail() {
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
fn grok_ad_prepare_rejects_invalid_input_before_initializer_or_hook_writes() {
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
fn grok_ad_run_scoped_permissions_do_not_replace_saved_user_choices() {
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
fn grok_ad_identity_probe_rejects_slow_output() {
    let f = Fixture::new();
    f.install();
    let executable = f.bin.join("grok");
    fs::write(&executable,"#!/bin/sh\n# SYNTHETIC stalled identity command; no model.\n/bin/sleep 4\nprintf '%s\\n' 'grok 1.0.5 (synthetic)'\n").unwrap();
    assert!(
        identity_output(&executable, "--version").is_err(),
        "unbounded stalled probe was accepted"
    );
}

#[test]
fn grok_ad_identity_probe_rejects_oversized_output() {
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
