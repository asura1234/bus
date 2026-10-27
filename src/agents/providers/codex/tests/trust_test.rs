use crate::agents::providers::{
    codex,
    hook_json::{self, HookContext, HookContract},
    launch::{self, LaunchSpec},
    ProviderKind,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

struct Fixture {
    root: PathBuf,
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl Fixture {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("codex-project-trust");
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        for binary in ["codex", "claude", "cursor-agent"] {
            std::fs::write(root.join("bin").join(binary), "fixture").unwrap();
        }
        let previous = ["CODEX_HOME", "PATH"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        let mut paths = vec![root.join("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        std::env::set_var("CODEX_HOME", root.join("home"));
        Self { root, previous }
    }

    fn project(&self, name: &str, git: bool) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        if git {
            assert!(std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&path)
                .status()
                .unwrap()
                .success());
        }
        path.canonicalize().unwrap()
    }

    fn trust(&self, project: &Path, trust: &str) {
        std::fs::write(
            self.root.join("home/config.toml"),
            format!(
                "[projects.{}]\ntrust_level = {trust:?}\n",
                serde_json::to_string(&project.display().to_string()).unwrap()
            ),
        )
        .unwrap();
    }

    fn spec(&self, cwd: &Path, provider: ProviderKind) -> LaunchSpec {
        LaunchSpec {
            provider,
            cwd: cwd.display().to_string(),
            extra_args: String::new(),
            consent_project_hooks: true,
            hooks: HookContext {
                binary: std::env::current_exe().unwrap(),
                spool: self.root.join("capture"),
                launch_id: "trust-launch".into(),
            },
        }
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
fn untrusted_codex_non_git_refuses_even_with_current_hooks_before_capture() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let project = fixture.project("plain", false);
    fixture.trust(&project, "untrusted");
    let spec = fixture.spec(&project, ProviderKind::Codex);
    let path = project.join(".codex/hooks.json");
    let contract = HookContract::for_provider(ProviderKind::Codex);
    hook_json::install_hooks(&path, contract, &spec.hooks.binary).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut initialized = false;
    let result = launch::prepare(&spec, |hooks| {
        initialized = true;
        std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    assert!(
        result.is_err(),
        "untrusted non-git launch accepted with current hooks"
    );
    let error = result.unwrap_err();
    assert!(
        error.contains("untrusted") && error.contains("hooks"),
        "{error}"
    );
    assert!(
        error.contains("trusted") && error.contains("/hooks"),
        "{error}"
    );
    assert!(!initialized);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!spec.hooks.spool.exists());
}

#[test]
fn untrusted_codex_git_root_and_subdirectory_refuse() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let project = fixture.project("repo", true);
    fixture.trust(&project, "untrusted");
    let nested = project.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut accepted = Vec::new();
    for cwd in [&project, &nested] {
        let result = launch::prepare(&fixture.spec(cwd, ProviderKind::Codex), |hooks| {
            std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
        });
        if result.is_ok() {
            accepted.push(cwd.display().to_string());
        }
    }
    assert!(
        accepted.is_empty(),
        "untrusted git launches accepted: {accepted:?}"
    );
}

#[test]
fn trusted_codex_non_git_git_root_and_subdirectory_keep_capture() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    for git in [false, true] {
        let project = fixture.project(if git { "repo" } else { "plain" }, git);
        let nested = project.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        for cwd in [&project, &nested] {
            fixture.trust(&launch::project_root(cwd), "trusted");
            let spec = fixture.spec(cwd, ProviderKind::Codex);
            let prepared = launch::prepare(&spec, |hooks| {
                std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
            })
            .unwrap();
            assert_eq!(prepared.cwd, *cwd);
            assert_eq!(prepared.env["BUS_LAUNCH_ID"], "trust-launch");
            assert!(hook_json::hooks_are_current(
                &launch::project_root(cwd).join(".codex/hooks.json"),
                HookContract::for_provider(ProviderKind::Codex),
                &spec.hooks.binary
            ));
        }
    }
}

#[test]
fn codex_project_distrust_does_not_block_claude_or_cursor_in_same_non_git_pwd() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let project = fixture.project("plain", false);
    fixture.trust(&project, "untrusted");
    for provider in [ProviderKind::ClaudeCode, ProviderKind::Cursor] {
        let spec = fixture.spec(&project, provider);
        let prepared = launch::prepare(&spec, |hooks| {
            std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(prepared.cwd, project);
        assert_eq!(prepared.env["BUS_LAUNCH_ID"], "trust-launch");
    }
}

#[test]
fn untrusted_codex_adopted_relaunch_refuses_without_reserving_session() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let project = fixture.project("plain", false);
    fixture.trust(&project, "untrusted");
    let mut spec = fixture.spec(&project, ProviderKind::Codex);
    spec.extra_args = "resume 160d1f8b-9023-44b8-9bc7-24333effb185".into();
    let result = launch::prepare(&spec, |hooks| {
        std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    assert!(
        result.is_err(),
        "adopted relaunch silently accepted disabled hooks"
    );
    assert!(!spec.hooks.spool.exists());
}

#[test]
fn untrusted_codex_native_resume_refuses_without_rebinding_hooks() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let project = fixture.project("plain", false);
    fixture.trust(&project, "untrusted");
    let spec = fixture.spec(&project, ProviderKind::Codex);
    let path = project.join(".codex/hooks.json");
    hook_json::install_hooks(
        &path,
        HookContract::for_provider(ProviderKind::Codex),
        &fixture.root.join("old-bus"),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(codex::resume::capture_args(&spec.hooks, &project).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
