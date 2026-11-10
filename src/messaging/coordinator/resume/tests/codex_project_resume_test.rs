use super::*;
use std::os::unix::fs::PermissionsExt;

struct CodexHome(Option<std::ffi::OsString>);
impl Drop for CodexHome {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("CODEX_HOME", value),
            None => std::env::remove_var("CODEX_HOME"),
        }
    }
}

fn effective_project_fixture() -> (Fixture, PathBuf, Vec<String>) {
    let fixture = Fixture::new(Provider::Codex);
    let target = fixture.root.join("target project");
    std::fs::create_dir_all(target.join("nested")).unwrap();
    for project in [&fixture.root, &target] {
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(project)
            .status()
            .unwrap()
            .success());
    }
    std::fs::create_dir_all(target.join(".codex")).unwrap();
    std::fs::rename(fixture.hook_path(), target.join(".codex/hooks.json")).unwrap();
    let options = vec!["--cd".to_owned(), "../target project/nested".to_owned()];
    std::fs::write(
        fixture
            .root
            .join("callbacks/owned-launch/launch-options.json"),
        serde_json::to_vec(&options).unwrap(),
    )
    .unwrap();
    (fixture, target.canonicalize().unwrap(), options)
}

fn set_trust(fixture: &Fixture, target: &Path, target_trusted: bool) -> CodexHome {
    let home = fixture.root.join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let key = |path: &Path| {
        serde_json::to_string(&path.canonicalize().unwrap().display().to_string()).unwrap()
    };
    let trust = |trusted| if trusted { "trusted" } else { "untrusted" };
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[projects.{}]\ntrust_level = {:?}\n[projects.{}]\ntrust_level = {:?}\n",
            key(&fixture.root),
            trust(!target_trusted),
            key(target),
            trust(target_trusted),
        ),
    )
    .unwrap();
    let previous = CodexHome(std::env::var_os("CODEX_HOME"));
    std::env::set_var("CODEX_HOME", home);
    previous
}

#[test]
fn codex_cold_resume_loads_effective_project_hooks_without_shell_hooks() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (fixture, target, options) = effective_project_fixture();
    let _home = set_trust(&fixture, &target, true);
    let bin = fixture.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("codex");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _path = crate::utils::env::PathOverride::set(&bin);
    let mut context = fixture.context();
    // A restored pane reports its shell's Git root, while --cd was relative
    // to the original agent cwd one directory beneath that root.
    context.project = fixture.root.clone();
    let extras = load(&context, &fixture.facts, &fixture.plan)
        .unwrap_or_else(|error| panic!("effective project resume failed: {error}"));
    assert!(!fixture.hook_path().exists());
    assert!(!fixture.root.join(".codex/hooks.json").exists());
    assert_eq!(
        crate::agents::providers::launch::saved_options(
            crate::agents::providers::ProviderKind::Codex,
            &fixture.root.join("callbacks/owned-launch"),
        )
        .unwrap(),
        options
    );
    assert!(extras
        .args
        .windows(options.len())
        .any(|args| args == options.as_slice()));
    let bridge = codex_callback_binary(&fixture.root, &fixture.binary).unwrap();
    assert!(validate_resume_hooks(
        &target.join(".codex/hooks.json"),
        HookContract::for_provider(crate::agents::providers::ProviderKind::Codex),
        &bridge
    )
    .is_ok());
    assert_eq!(
        extras
            .env
            .iter()
            .find(|(key, _)| key == "BUS_CODEX_NATIVE")
            .map(|(_, value)| value.as_str()),
        Some("1")
    );
    let mut runtime =
        crate::agents::providers::codex::native::Runtime::from_environment(&extras.env).unwrap();
    let argv = runtime
        .launch(&[
            executable.display().to_string(),
            "resume".into(),
            "160d1f8b-9023-44b8-9bc7-24333effb185".into(),
        ])
        .unwrap();
    assert!(argv.iter().any(|arg| arg == "--bus-codex-runtime"));
}

#[test]
fn codex_cold_resume_keeps_distrust_of_the_effective_project() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (fixture, target, _) = effective_project_fixture();
    let _home = set_trust(&fixture, &target, false);
    let mut context = fixture.context();
    context.project = fixture.root.clone();
    let before = std::fs::read(target.join(".codex/hooks.json")).unwrap();
    let error = load(&context, &fixture.facts, &fixture.plan).unwrap_err();
    assert!(
        error.contains("untrusted") && error.contains(target.to_str().unwrap()),
        "{error}"
    );
    assert_eq!(
        std::fs::read(target.join(".codex/hooks.json")).unwrap(),
        before
    );
    assert!(!fixture.root.join(".codex/hooks.json").exists());
}
