//! Synthetic cold-capture cases; ownership attestation remains in the coordinator.
use super::*;
use crate::agents::providers::{
    grok::launch::{self, launch_test::Fixture},
    launch as common,
};
use serde_json::json;
#[test]
fn grok_resume_requires_existing_capture_without_silent_repair() {
    let f = Fixture::new();
    f.install();
    let spec = f.spec("", true);
    assert!(capture_args(&spec.hooks, &f.project).is_err());
    assert!(!f.project.join(".grok").exists());
    f.prepare("--permission-mode default");
    let path = f.project.join(".grok/hooks/bus.json");
    let original = std::fs::read(&path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(&original).unwrap();
    doc["hooks"]["Stop"][0]["hooks"][0]["timeout"] = json!(999);
    let mutated = serde_json::to_vec(&doc).unwrap();
    std::fs::write(&path, &mutated).unwrap();
    assert!(capture_args(&spec.hooks, &f.project).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), mutated);
    std::fs::write(&path, &original).unwrap();
    std::fs::remove_file(spec.hooks.spool.join("launch-options.json")).unwrap();
    assert!(capture_args(&spec.hooks, &f.project).is_err());
}
#[test]
fn grok_resume_rebinds_exact_owned_hooks_and_restores_saved_options() {
    let f = Fixture::new();
    f.install();
    f.prepare("--session-id 160d1f8b-9023-44b8-9bc7-24333effb185 --permission-mode default --allow 'Bash(ls*)' initial");
    let mut spec = f.spec("", true);
    spec.hooks.binary = f.root.join("new bus synthetic");
    let (path, args, rebound) = capture_args(&spec.hooks, &f.project).unwrap();
    assert!(rebound);
    assert_eq!(args, ["--no-leader", "--fullscreen", "--no-auto-update"]);
    crate::agents::providers::hook_json::validate_resume_hooks(
        &path,
        crate::agents::providers::grok::HOOK_CONTRACT,
        &spec.hooks.binary,
    )
    .unwrap();
    assert!(!capture_args(&spec.hooks, &f.project).unwrap().2);
    let options = common::saved_options(
        crate::agents::providers::ProviderKind::Grok,
        &spec.hooks.spool,
    )
    .unwrap();
    assert_eq!(
        common::resumable_options(crate::agents::providers::ProviderKind::Grok, &options),
        ["--permission-mode", "default", "--allow", "Bash(ls*)"]
    );
    assert_eq!(
        launch::environment(&spec.hooks).unwrap()["BUS_LAUNCH_ID"],
        "synthetic-launch"
    );
}
#[test]
fn grok_resume_revalidates_the_saved_executable_instead_of_switching_path() {
    let f = Fixture::new();
    f.install();
    f.prepare("");
    let spec = f.spec("", true);
    let preferred = f.root.join("home/.grok/bin/grok");
    f.executable(&preferred, "grok 1.0.5 (synthetic)");
    assert_eq!(
        std::env::split_paths(&launch::environment(&spec.hooks).unwrap()["PATH"])
            .next()
            .unwrap(),
        f.bin
    );
    f.executable(&f.bin.join("grok"), "regex grok 1.0");
    assert!(capture_args(&spec.hooks, &f.project).is_err());
    assert!(launch::environment(&spec.hooks).is_err());
}
#[test]
fn grok_resume_disabled_hooks_are_kept_unchanged() {
    let f = Fixture::new();
    f.install();
    f.prepare("");
    let mut spec = f.spec("", true);
    spec.hooks.binary = f.root.join("new-bus");
    let path = f.project.join(".grok/hooks/bus.json");
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    doc["disableAllHooks"] = json!(true);
    let bytes = serde_json::to_vec(&doc).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(capture_args(&spec.hooks, &f.project).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn grok_resume_does_not_duplicate_explicit_runtime_flags() {
    let f = Fixture::new();
    f.install();
    f.prepare("--no-leader --fullscreen --no-auto-update");
    let spec = f.spec("", true);
    assert!(capture_args(&spec.hooks, &f.project).unwrap().1.is_empty());
}

#[test]
fn grok_cold_resume_preserves_explicit_runtime_with_literal_startup_tail() {
    let f = Fixture::new();
    f.install();
    f.prepare("--resume=160d1f8b-9023-44b8-9bc7-24333effb185 --no-leader --fullscreen --no-auto-update --permission-mode default -- '--cwd=/tmp --single literal'");
    let spec = f.spec("", true);
    let (_, capture, _) = capture_args(&spec.hooks, &f.project).unwrap();
    let options = common::saved_options(
        crate::agents::providers::ProviderKind::Grok,
        &spec.hooks.spool,
    )
    .unwrap();
    let mut restored =
        common::resumable_options(crate::agents::providers::ProviderKind::Grok, &options);
    common::extend_generated_args(
        crate::agents::providers::ProviderKind::Grok,
        &mut restored,
        capture,
    );
    assert_eq!(
        restored,
        [
            "--no-leader",
            "--fullscreen",
            "--no-auto-update",
            "--permission-mode",
            "default"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(spec.hooks.spool.join("adopted-session")).unwrap(),
        "160d1f8b-9023-44b8-9bc7-24333effb185"
    );
}
#[test]
fn grok_cold_resume_rejects_relocating_saved_options_without_changing_hooks() {
    let f = Fixture::new();
    f.install();
    f.prepare("");
    let spec = f.spec("", true);
    let path = f.project.join(".grok/hooks/bus.json");
    let bytes = std::fs::read(&path).unwrap();
    for options in [["--cwd=/tmp"], ["-wsynthetic"]] {
        std::fs::write(
            spec.hooks.spool.join("launch-options.json"),
            serde_json::to_vec(&options).unwrap(),
        )
        .unwrap();
        assert!(capture_args(&spec.hooks, &f.project).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[cfg(unix)]
#[test]
fn grok_resume_refuses_home_and_external_hook_ancestors_without_rebinding() {
    for ancestor in [".grok", ".grok/hooks", ".grok/hooks/bus.json"] {
        let f = Fixture::new();
        f.install();
        f.prepare("");
        let mut spec = f.spec("", true);
        spec.hooks.binary = f.root.join("replacement-bus");
        let home = f.root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::rename(f.project.join(".grok"), home.join(".grok")).unwrap();
        let global = home.join(".grok/hooks/bus.json");
        let original = std::fs::read(&global).unwrap();
        assert!(capture_args(&spec.hooks, &home).is_err());
        assert_eq!(std::fs::read(&global).unwrap(), original);
        std::fs::rename(home.join(".grok"), f.project.join(".grok")).unwrap();
        let target = f.project.join(ancestor);
        let external = f.root.join("external");
        std::fs::rename(&target, &external).unwrap();
        std::os::unix::fs::symlink(&external, &target).unwrap();
        let external_file = match ancestor {
            ".grok" => external.join("hooks/bus.json"),
            ".grok/hooks" => external.join("bus.json"),
            _ => external.clone(),
        };
        let original = std::fs::read(&external_file).unwrap();
        assert!(capture_args(&spec.hooks, &f.project).is_err(), "{ancestor}");
        assert_eq!(std::fs::read(external_file).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn grok_resume_preserves_global_hooks_aliased_to_project_configuration() {
    let mut mutated = Vec::new();
    let mut accepted = Vec::new();
    for alias in [".grok", ".grok/hooks", ".grok/hooks/bus.json"] {
        let f = Fixture::new();
        f.install();
        f.prepare("");
        let home = f.root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        if alias == ".grok" {
            std::os::unix::fs::symlink(f.project.join(".grok"), home.join(alias)).unwrap();
        } else if alias == ".grok/hooks/bus.json" {
            std::fs::create_dir_all(home.join(".grok/hooks")).unwrap();
            std::os::unix::fs::symlink(f.project.join(".grok/hooks/bus.json"), home.join(alias))
                .unwrap();
        } else {
            std::fs::create_dir_all(home.join(".grok")).unwrap();
            std::os::unix::fs::symlink(f.project.join(".grok/hooks"), home.join(alias)).unwrap();
        }
        let global_hooks = home.join(".grok/hooks/bus.json");
        let original = std::fs::read(&global_hooks).unwrap();
        let mut spec = f.spec("", true);
        spec.hooks.binary = f.root.join("replacement-bus");

        let result = capture_args(&spec.hooks, &f.project);

        if std::fs::read(&global_hooks).unwrap() != original {
            mutated.push(alias);
        }
        if result.is_ok() {
            accepted.push(alias);
        }
    }
    assert!(
        mutated.is_empty(),
        "resume changed global hook bytes through {mutated:?}"
    );
    assert!(
        accepted.is_empty(),
        "global hook aliases were accepted: {accepted:?}"
    );
}

#[cfg(unix)]
#[test]
fn grok_resume_preserves_global_hooks_with_a_different_filename() {
    let mut mutated = Vec::new();
    let mut accepted = Vec::new();
    for alias in ["direct", "relative", "chained", "unrelated"] {
        let f = Fixture::new();
        f.install();
        f.prepare("");
        let global_hooks = f.root.join("home/.grok/hooks");
        std::fs::create_dir_all(&global_hooks).unwrap();
        let project_file = f.project.join(".grok/hooks/bus.json");
        let original = std::fs::read(&project_file).unwrap();
        let global_file = global_hooks.join("shared.json");
        match alias {
            "direct" => std::os::unix::fs::symlink(&project_file, &global_file).unwrap(),
            "relative" => {
                std::os::unix::fs::symlink("../../../project/.grok/hooks/bus.json", &global_file)
                    .unwrap()
            }
            "chained" => {
                std::os::unix::fs::symlink(&project_file, global_hooks.join("linked-hook.json"))
                    .unwrap();
                std::os::unix::fs::symlink("linked-hook.json", &global_file).unwrap();
            }
            "unrelated" => {
                let independent = f.root.join("independent-hook.json");
                std::fs::write(&independent, &original).unwrap();
                std::os::unix::fs::symlink(independent, &global_file).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(std::fs::read(&global_file).unwrap(), original);
        let mut spec = f.spec("", true);
        spec.hooks.binary = f.root.join("replacement-bus");
        let result = capture_args(&spec.hooks, &f.project);
        if std::fs::read(&global_file).unwrap() != original {
            mutated.push(alias);
        }
        if alias == "unrelated" {
            assert!(
                result.is_ok(),
                "unrelated global alias blocked resume: {result:?}"
            );
        } else if result.is_ok() {
            accepted.push(alias);
        }
    }
    assert!(
        mutated.is_empty(),
        "resume changed global shared.json bytes through {mutated:?}"
    );
    assert!(
        accepted.is_empty(),
        "global shared.json aliases were accepted: {accepted:?}"
    );
}
