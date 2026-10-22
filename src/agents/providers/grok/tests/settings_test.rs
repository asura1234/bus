//! Synthetic project settings; never writes a real Grok configuration.
use super::launch_test::Fixture;
use crate::agents::providers::{hook_json, launch as common};
use serde_json::json;
#[test]
fn grok_consent_absent_writes_nothing_and_never_initializes() {
    let f = Fixture::new();
    f.install();
    let err = common::prepare(&f.spec("", false), |_| panic!("no consent")).unwrap_err();
    assert!(err.contains("confirmation"));
    assert!(!f.project.join(".grok").exists());
    assert!(!f.root.join("capture").exists());
}
#[test]
fn grok_settings_preserve_user_hooks_and_reject_conflict_atomically() {
    let f = Fixture::new();
    f.install();
    let path = f.project.join(".grok/hooks/bus.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let user = json!({"matcher":"Bash","hooks":[{"type":"command","command":"echo synthetic-user","timeout":7}]});
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"custom":"keep","hooks":{"Stop":[user.clone()]}})).unwrap(),
    )
    .unwrap();
    f.prepare("");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["custom"], "keep");
    assert_eq!(value["hooks"]["Stop"][0], user);
    let contract = super::super::HOOK_CONTRACT;
    let command = contract.command(&f.spec("", true).hooks.binary);
    for event in contract.events {
        assert!(value["hooks"][event]
            .as_array()
            .unwrap()
            .contains(&contract.entry(&command)));
    }
    let conflict = json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"bus --bus-callback grok-hook","timeout":999}]}]}});
    let bytes = serde_json::to_vec(&conflict).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(common::prepare(&f.spec("", true), |_| panic!(
        "conflict initialized capture"
    ))
    .is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
#[test]
fn grok_settings_use_git_project_root_and_do_not_modify_other_hook_files() {
    let f = Fixture::new();
    f.install();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&f.project)
        .status()
        .unwrap()
        .success());
    let nested = f.project.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let other = f.project.join(".grok/hooks/user.json");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "synthetic-user-bytes").unwrap();
    let mut spec = f.spec("", true);
    spec.cwd = nested.display().to_string();
    common::prepare(&spec, |h| {
        std::fs::create_dir_all(&h.spool).map_err(|e| e.to_string())
    })
    .unwrap();
    assert!(f.project.join(".grok/hooks/bus.json").exists());
    assert!(!nested.join(".grok").exists());
    assert_eq!(
        std::fs::read_to_string(other).unwrap(),
        "synthetic-user-bytes"
    );
    hook_json::validate_resume_hooks(
        &f.project.join(".grok/hooks/bus.json"),
        super::super::HOOK_CONTRACT,
        &spec.hooks.binary,
    )
    .unwrap();
}

#[test]
fn grok_disabled_project_hooks_fail_before_files_or_initializer() {
    let f = Fixture::new();
    f.install();
    let path = f.project.join(".grok/hooks/bus.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = b"{\"disableAllHooks\":true}";
    std::fs::write(&path, bytes).unwrap();
    assert!(common::prepare(&f.spec("", true), |_| panic!(
        "disabled hooks initialized capture"
    ))
    .is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn grok_setup_rejects_a_symlinked_project_config_ancestor() {
    let f = Fixture::new();
    f.install();
    // Model a project's .grok pointing at a shared/global configuration. Both
    // sides remain scratch fixtures; this must never access the real HOME.
    let external = f.root.join("external-grok-config");
    let hooks_dir = external.join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let external_bus = hooks_dir.join("bus.json");
    let original = b"{\"custom\":\"external-config-must-stay-unchanged\"}";
    std::fs::write(&external_bus, original).unwrap();
    std::os::unix::fs::symlink(&external, f.project.join(".grok")).unwrap();
    let initialized = std::cell::Cell::new(false);

    let result = common::prepare(&f.spec("", true), |hooks| {
        initialized.set(true);
        std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });

    assert!(
        result.is_err(),
        "project setup followed a .grok symlink: {result:?}"
    );
    assert!(!initialized.get(), "unsafe setup initialized a capture");
    assert_eq!(std::fs::read(external_bus).unwrap(), original);
}

#[test]
fn grok_project_setup_does_not_install_hooks_in_the_home_config() {
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut spec = f.spec("", true);
    spec.cwd = "~".into();
    let initialized = std::cell::Cell::new(false);

    let result = common::prepare(&spec, |hooks| {
        initialized.set(true);
        std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });

    assert!(
        result.is_err(),
        "project setup accepted HOME and wrote global hooks: {result:?}"
    );
    assert!(!initialized.get());
    assert!(!home.join(".grok/hooks/bus.json").exists());
}

#[test]
fn grok_home_dotfiles_repository_never_receives_project_hooks() {
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    let nested = home.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&home)
        .status()
        .unwrap()
        .success());
    let mut spec = f.spec("", true);
    spec.cwd = nested.display().to_string();
    let initialized = std::cell::Cell::new(false);
    assert!(common::prepare(&spec, |_| {
        initialized.set(true);
        Ok(())
    })
    .is_err());
    assert!(!initialized.get());
    assert!(!home.join(".grok").exists());
}

#[cfg(unix)]
#[test]
fn grok_setup_preserves_global_hooks_aliased_to_project_configuration() {
    let mut mutated = Vec::new();
    let mut accepted = Vec::new();
    let mut initialized_aliases = Vec::new();
    for alias in [".grok", ".grok/hooks", ".grok/hooks/bus.json"] {
        let f = Fixture::new();
        f.install();
        let home = f.root.join("home");
        let project_hooks = f.project.join(".grok/hooks");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project_hooks).unwrap();
        let original = b"{\"custom\":\"global-config-must-stay-unchanged\"}";
        std::fs::write(project_hooks.join("bus.json"), original).unwrap();
        if alias == ".grok" {
            std::os::unix::fs::symlink(f.project.join(".grok"), home.join(alias)).unwrap();
        } else if alias == ".grok/hooks/bus.json" {
            std::fs::create_dir_all(home.join(".grok/hooks")).unwrap();
            std::os::unix::fs::symlink(f.project.join(".grok/hooks/bus.json"), home.join(alias))
                .unwrap();
        } else {
            std::fs::create_dir_all(home.join(".grok")).unwrap();
            std::os::unix::fs::symlink(&project_hooks, home.join(alias)).unwrap();
        }
        let global_hooks = home.join(".grok/hooks/bus.json");
        assert_eq!(std::fs::read(&global_hooks).unwrap(), original);
        let initialized = std::cell::Cell::new(false);

        let result = common::prepare(&f.spec("", true), |hooks| {
            initialized.set(true);
            std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
        });

        if std::fs::read(&global_hooks).unwrap() != original {
            mutated.push(alias);
        }
        if result.is_ok() {
            accepted.push(alias);
        }
        if initialized.get() {
            initialized_aliases.push(alias);
        }
    }
    assert!(
        mutated.is_empty(),
        "project setup changed global hook bytes through {mutated:?}"
    );
    assert!(
        accepted.is_empty(),
        "global hook aliases were accepted: {accepted:?}"
    );
    assert!(
        initialized_aliases.is_empty(),
        "unsafe aliases initialized captures: {initialized_aliases:?}"
    );
}

#[cfg(unix)]
#[test]
fn grok_setup_refuses_a_dangling_global_alias_before_creating_its_target() {
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(f.project.join(".grok"), home.join(".grok")).unwrap();
    let initialized = std::cell::Cell::new(false);
    let result = common::prepare(&f.spec("", true), |_| {
        initialized.set(true);
        Ok(())
    });
    assert!(result.is_err());
    assert!(!initialized.get());
    assert!(!f.project.join(".grok").exists());
}

#[cfg(unix)]
#[test]
fn grok_setup_keeps_an_unrelated_global_alias_unchanged() {
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    let global = f.project.join("separate-global-config");
    std::fs::create_dir_all(global.join("hooks")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let original = b"{\"custom\":\"keep-unrelated-global-hook\"}";
    std::fs::write(global.join("hooks/bus.json"), original).unwrap();
    std::os::unix::fs::symlink(&global, home.join(".grok")).unwrap();
    f.prepare("");
    assert!(f.project.join(".grok/hooks/bus.json").exists());
    assert_eq!(
        std::fs::read(home.join(".grok/hooks/bus.json")).unwrap(),
        original
    );
}

#[cfg(unix)]
#[test]
fn grok_setup_refuses_a_global_directory_alias_to_the_project_hooks_directory() {
    let f = Fixture::new();
    f.install();
    let home = f.root.join("home");
    let hooks = f.project.join(".grok/hooks");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&hooks).unwrap();
    let original = b"{\"custom\":\"global-root-entry\"}";
    std::fs::write(hooks.join("bus.json"), original).unwrap();
    std::os::unix::fs::symlink(&hooks, home.join(".grok")).unwrap();
    let initialized = std::cell::Cell::new(false);
    let result = common::prepare(&f.spec("", true), |_| {
        initialized.set(true);
        Ok(())
    });
    assert!(result.is_err());
    assert!(!initialized.get());
    assert_eq!(
        std::fs::read(home.join(".grok/bus.json")).unwrap(),
        original
    );
}

#[cfg(unix)]
#[test]
fn grok_setup_preserves_global_hooks_with_a_different_filename() {
    let mut mutated = Vec::new();
    let mut accepted = Vec::new();
    let mut initialized_aliases = Vec::new();
    for alias in ["direct", "relative", "chained", "unrelated"] {
        let f = Fixture::new();
        f.install();
        let project_hooks = f.project.join(".grok/hooks");
        let global_hooks = f.root.join("home/.grok/hooks");
        std::fs::create_dir_all(&project_hooks).unwrap();
        std::fs::create_dir_all(&global_hooks).unwrap();
        let project_file = project_hooks.join("bus.json");
        let original = b"{\"custom\":\"global-shared-hook-must-stay-unchanged\"}";
        std::fs::write(&project_file, original).unwrap();
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
                std::fs::write(&independent, original).unwrap();
                std::os::unix::fs::symlink(independent, &global_file).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(std::fs::read(&global_file).unwrap(), original);
        let initialized = std::cell::Cell::new(false);
        let result = common::prepare(&f.spec("", true), |hooks| {
            initialized.set(true);
            std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
        });
        if std::fs::read(&global_file).unwrap() != original {
            mutated.push(alias);
        }
        if alias == "unrelated" {
            assert!(
                result.is_ok(),
                "unrelated global alias blocked project setup: {result:?}"
            );
            assert!(initialized.get());
        } else {
            if result.is_ok() {
                accepted.push(alias);
            }
            if initialized.get() {
                initialized_aliases.push(alias);
            }
        }
    }
    assert!(
        mutated.is_empty(),
        "project setup changed global shared.json bytes through {mutated:?}"
    );
    assert!(
        accepted.is_empty(),
        "global shared.json aliases were accepted: {accepted:?}"
    );
    assert!(
        initialized_aliases.is_empty(),
        "unsafe aliases initialized captures: {initialized_aliases:?}"
    );
}

#[cfg(unix)]
#[test]
fn grok_setup_preserves_global_aliases_with_case_variant_targets() {
    use std::os::unix::fs::MetadataExt;
    for alias in ["config", "hooks", "file"] {
        let f = Fixture::new();
        f.install();
        let home = f.root.join("home");
        let config = f.project.join(".grok");
        let hooks = config.join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let original = b"{\"custom\":\"case-aliased-global-hook\"}";
        std::fs::write(hooks.join("bus.json"), original).unwrap();
        let (actual, cased) = match alias {
            "config" => (config.clone(), f.project.join(".GROK")),
            "hooks" => (hooks.clone(), config.join("HOOKS")),
            _ => (hooks.join("bus.json"), hooks.join("BUS.JSON")),
        };
        let Ok(cased_metadata) = std::fs::metadata(&cased) else {
            // Case-sensitive filesystems do not have this physical alias.
            return;
        };
        let actual_metadata = std::fs::metadata(&actual).unwrap();
        assert_eq!(
            (actual_metadata.dev(), actual_metadata.ino()),
            (cased_metadata.dev(), cased_metadata.ino())
        );
        let global_file = match alias {
            "config" => {
                std::os::unix::fs::symlink(&cased, home.join(".grok")).unwrap();
                home.join(".grok/hooks/bus.json")
            }
            "hooks" => {
                std::fs::create_dir_all(home.join(".grok")).unwrap();
                std::os::unix::fs::symlink(&cased, home.join(".grok/hooks")).unwrap();
                home.join(".grok/hooks/bus.json")
            }
            _ => {
                std::fs::create_dir_all(home.join(".grok/hooks")).unwrap();
                let global_file = home.join(".grok/hooks/shared.json");
                std::os::unix::fs::symlink(&cased, &global_file).unwrap();
                global_file
            }
        };
        let result = common::prepare(&f.spec("", true), |hooks| {
            std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
        });
        assert_eq!(std::fs::read(global_file).unwrap(), original, "{alias}");
        assert!(
            result.is_err(),
            "case-variant global {alias} alias was accepted"
        );
    }
}

#[cfg(unix)]
#[test]
fn grok_setup_does_not_create_a_case_variant_global_hook_source() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    f.install();
    let hooks = f.project.join(".grok/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let Ok(cased_parent) = std::fs::metadata(f.project.join(".GROK/HOOKS")) else {
        return;
    };
    let actual_parent = std::fs::metadata(&hooks).unwrap();
    assert_eq!(
        (actual_parent.dev(), actual_parent.ino()),
        (cased_parent.dev(), cased_parent.ino())
    );
    let global_source = hooks.join("BUS.JSON");
    assert!(!global_source.exists());
    let config = f.root.join("home/.grok/hooks-paths");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, format!("{}\n", global_source.display())).unwrap();
    let initialized = std::cell::Cell::new(false);
    let result = common::prepare(&f.spec("", true), |hooks| {
        initialized.set(true);
        std::fs::create_dir_all(&hooks.spool).map_err(|e| e.to_string())
    });
    assert!(
        !global_source.exists(),
        "setup created the globally configured BUS.JSON source before refusing capture: {result:?}"
    );
    assert!(result.is_err());
    assert!(!initialized.get());
}

#[cfg(unix)]
#[test]
fn grok_hook_global_roots_are_enforced_for_install_and_resume() {
    for resume in [false, true] {
        for route in [
            "grok-home",
            "grok-home-parent",
            "grok-home-alias",
            "config-parent",
            "config-alias",
            "config-second",
            "config-folder",
            "config-file",
            "config-relative",
            "config-tilde",
            "configured-shared",
            "configured-nested-shared",
            "independent-home",
            "independent",
        ] {
            let f = Fixture::new();
            f.install();
            f.prepare("");
            let file = f.project.join(".grok/hooks/bus.json");
            let original = std::fs::read(&file).unwrap();
            let global = f.root.join("home/.grok");
            std::fs::create_dir_all(&global).unwrap();
            if route.starts_with("grok-home") || route == "independent-home" {
                let root = match route {
                    "grok-home-parent" => f.project.clone(),
                    "grok-home-alias" => {
                        let alias = f.root.join("relocated-home");
                        std::os::unix::fs::symlink(f.project.join(".grok"), &alias).unwrap();
                        alias
                    }
                    "independent-home" => f.root.join("separate-home"),
                    _ => f.project.join(".grok"),
                };
                std::env::set_var("GROK_HOME", &root);
                if resume {
                    std::fs::write(
                        f.spec("", true).hooks.spool.join("grok-home.json"),
                        serde_json::to_vec(&root).unwrap(),
                    )
                    .unwrap();
                }
            } else {
                let entry = match route {
                    "config-parent" => f.project.clone(),
                    "config-alias" => {
                        let alias = f.root.join("configured-alias");
                        std::os::unix::fs::symlink(f.project.join(".grok/hooks"), &alias).unwrap();
                        alias
                    }
                    "config-second" | "config-folder" => f.project.join(".grok/hooks"),
                    "config-file" => file.clone(),
                    "config-relative" => std::path::PathBuf::from("../../project/.grok/hooks"),
                    "config-tilde" => std::path::PathBuf::from("~/../project/.grok/hooks"),
                    _ => f.root.join("additional-hooks"),
                };
                if route.starts_with("configured-") || route == "independent" {
                    let folder = if route == "configured-nested-shared" {
                        entry.join("hooks")
                    } else {
                        entry.clone()
                    };
                    std::fs::create_dir_all(&folder).unwrap();
                    let target = if route == "independent" {
                        let other = f.root.join("independent.json");
                        std::fs::write(&other, &original).unwrap();
                        other
                    } else {
                        file.clone()
                    };
                    std::os::unix::fs::symlink(target, folder.join("shared.json")).unwrap();
                }
                let mut entries = vec![entry.to_string_lossy().into_owned()];
                if route == "config-second" {
                    let first = f.root.join("independent-first");
                    std::fs::create_dir(&first).unwrap();
                    entries.insert(0, first.to_string_lossy().into_owned());
                }
                std::fs::write(
                    global.join("hooks-paths"),
                    format!("{}\n", entries.join("\n")),
                )
                .unwrap();
            }
            let mut spec = f.spec("", true);
            spec.hooks.binary = f.root.join("replacement-bus");
            let initialized = std::cell::Cell::new(false);
            let result = if resume {
                crate::agents::providers::grok::resume::capture_args(&spec.hooks, &f.project)
                    .map(|_| ())
            } else {
                common::prepare(&spec, |_| {
                    initialized.set(true);
                    Ok(())
                })
                .map(|_| ())
            };
            assert_eq!(
                result.is_ok(),
                route.starts_with("independent")
                    || matches!(route, "config-relative" | "config-tilde"),
                "{resume}/{route}: {result:?}"
            );
            if !route.starts_with("independent")
                && !matches!(route, "config-relative" | "config-tilde")
            {
                assert!(!initialized.get(), "{resume}/{route}");
                assert_eq!(std::fs::read(&file).unwrap(), original, "{resume}/{route}");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn grok_relative_home_uses_provider_cwd_and_registry_ignores_relative_lines() {
    struct RestoreCwd(std::path::PathBuf);
    impl Drop for RestoreCwd {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }
    for resume in [false, true] {
        for route in ["home", "config-folder", "config-file"] {
            let f = Fixture::new();
            f.install();
            f.prepare("");
            let leaf = f.project.join(".grok/hooks/bus.json");
            let original = std::fs::read(&leaf).unwrap();
            let server_cwd = f.root.join("bus-cwd");
            std::fs::create_dir_all(server_cwd.join(".grok/hooks")).unwrap();
            let _restore = RestoreCwd(std::env::current_dir().unwrap());
            std::env::set_current_dir(&server_cwd).unwrap();
            if route == "home" {
                std::env::set_var("GROK_HOME", ".grok");
            } else {
                let global = f.root.join("home/.grok");
                std::fs::create_dir_all(&global).unwrap();
                let entry = if route == "config-file" {
                    ".grok/hooks/bus.json"
                } else {
                    ".grok/hooks"
                };
                std::fs::write(global.join("hooks-paths"), format!("{entry}\n")).unwrap();
            }
            let mut spec = f.spec("", true);
            spec.hooks.binary = f.root.join("replacement-bus");
            let initialized = std::cell::Cell::new(false);
            let result = if resume {
                crate::agents::providers::grok::resume::capture_args(&spec.hooks, &f.project)
                    .map(|_| ())
            } else {
                common::prepare(&spec, |_| {
                    initialized.set(true);
                    Ok(())
                })
                .map(|_| ())
            };
            let refused = !resume && route == "home";
            assert_eq!(result.is_err(), refused, "{resume}/{route}: {result:?}");
            if refused {
                assert!(!initialized.get());
                assert_eq!(std::fs::read(&leaf).unwrap(), original, "{resume}/{route}");
            }
        }
    }
}

#[test]
fn grok_refused_case_variant_home_removes_only_new_empty_directories() {
    for existing_config in [false, true] {
        let f = Fixture::new();
        f.install();
        let probe = f.project.join("case-probe");
        std::fs::write(&probe, b"case sensitivity").unwrap();
        let insensitive = f.project.join("CASE-PROBE").exists();
        std::fs::remove_file(probe).unwrap();
        if !insensitive {
            return;
        }
        let config = f.project.join(".grok");
        if existing_config {
            std::fs::create_dir(&config).unwrap();
            std::fs::write(config.join("keep-user-file"), b"unchanged").unwrap();
        }
        let global = if existing_config {
            config.join("HOOKS")
        } else {
            f.project.join(".GROK")
        };
        std::env::set_var("GROK_HOME", global);
        let initialized = std::cell::Cell::new(false);
        let result = common::prepare(&f.spec("", true), |_| {
            initialized.set(true);
            Ok(())
        });
        assert!(result.is_err());
        assert!(!initialized.get());
        assert!(
            !config.join("hooks").exists(),
            "refusal left a new hooks directory"
        );
        assert_eq!(
            config.exists(),
            existing_config,
            "refusal changed a pre-existing directory"
        );
        if existing_config {
            assert_eq!(
                std::fs::read(config.join("keep-user-file")).unwrap(),
                b"unchanged"
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn grok_global_identity_survives_firmlink_spellings_on_install_and_resume() {
    for resume in [false, true] {
        for route in ["home", "registry-folder", "registry-file", "global-alias"] {
            let f = Fixture::new();
            f.install();
            f.prepare("");
            let leaf = f.project.join(".grok/hooks/bus.json");
            let original = std::fs::read(&leaf).unwrap();
            let actual = if route == "home" {
                f.project.join(".grok")
            } else if route == "registry-folder" {
                leaf.parent().unwrap().to_owned()
            } else {
                leaf.clone()
            };
            let alias = std::path::PathBuf::from(format!(
                "/System/Volumes/Data{}",
                actual.canonicalize().unwrap().display()
            ));
            assert!(alias.exists(), "firmlink fixture is unavailable");
            let mut spec = f.spec("", true);
            spec.hooks.binary = f.root.join("replacement-bus");
            if route == "home" {
                std::env::set_var("GROK_HOME", &alias);
                if resume {
                    std::fs::write(
                        spec.hooks.spool.join("grok-home.json"),
                        serde_json::to_vec(&alias).unwrap(),
                    )
                    .unwrap();
                }
            } else {
                let global = f.root.join("home/.grok");
                std::fs::create_dir_all(global.join("hooks")).unwrap();
                if route == "global-alias" {
                    std::os::unix::fs::symlink(&alias, global.join("hooks/shared.json")).unwrap();
                } else {
                    std::fs::write(global.join("hooks-paths"), format!("{}\n", alias.display()))
                        .unwrap();
                }
            }
            let result = if resume {
                crate::agents::providers::grok::resume::capture_args(&spec.hooks, &f.project)
                    .map(|_| ())
            } else {
                common::prepare(&spec, |_| panic!("global capture was initialized")).map(|_| ())
            };
            assert!(result.is_err(), "{resume}/{route}: {result:?}");
            assert_eq!(std::fs::read(&leaf).unwrap(), original, "{resume}/{route}");
        }
    }
}
