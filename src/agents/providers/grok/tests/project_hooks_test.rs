use super::{transact, Mode, Stage};
use crate::agents::providers::grok::launch::launch_test::Fixture;
use std::{fs, os::unix::fs::symlink};

#[test]
fn grok_generated_project_hooks_stay_out_of_git_status_without_hiding_user_settings() {
    let f = Fixture::new();
    f.install();
    let init = std::process::Command::new("git")
        .arg("-C")
        .arg(&f.project)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(init.success());
    f.prepare("");
    fs::write(f.project.join(".grok/settings.json"), "{}\n").unwrap();
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&f.project)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains(".grok/settings.json"), "{status}");
    assert!(!status.contains(".grok/hooks/bus.json"), "{status}");
    assert!(!status.contains(".grok/hooks/.bus-hooks.lock"), "{status}");
    let exclude = f.project.join(".git/info/exclude");
    let before = fs::read(&exclude).unwrap();
    f.prepare("");
    assert_eq!(
        fs::read(exclude).unwrap(),
        before,
        "exclude entries duplicated"
    );
}

#[test]
fn grok_git_exclude_symlink_is_never_followed() {
    let f = Fixture::new();
    f.install();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&f.project)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    let exclude = f.project.join(".git/info/exclude");
    let outside = f.root.join("outside-exclude");
    fs::write(&outside, b"untouched\n").unwrap();
    fs::remove_file(&exclude).unwrap();
    symlink(&outside, &exclude).unwrap();
    f.prepare("");
    assert_eq!(fs::read(outside).unwrap(), b"untouched\n");
}

#[test]
fn grok_hook_transaction_pins_directories_and_rechecks_after_commit() {
    for resume in [false, true] {
        for stage in [Stage::BeforeRename, Stage::AfterRename] {
            for component in ["root", ".grok", ".grok/hooks"] {
                for symlinked in [false, true] {
                    let f = Fixture::new();
                    f.install();
                    f.prepare("");
                    let global = f.root.join("home/.grok/hooks/bus.json");
                    fs::create_dir_all(global.parent().unwrap()).unwrap();
                    let original = br#"{"custom":"untouched-global-hook"}"#;
                    fs::write(&global, original).unwrap();
                    let (source, replacement) = match component {
                        "root" => (f.project.clone(), f.root.join("home")),
                        ".grok" => (f.project.join(component), f.root.join("home/.grok")),
                        _ => (f.project.join(component), f.root.join("home/.grok/hooks")),
                    };
                    let mut swapped = false;
                    let result = transact(
                        &f.project,
                        &f.root.join("replacement-bus"),
                        if resume { Mode::Resume } else { Mode::Install },
                        || Ok(()),
                        |point| {
                            if point == stage && !swapped {
                                fs::rename(&source, f.root.join("pinned-before-swap")).unwrap();
                                if symlinked {
                                    symlink(&replacement, &source).unwrap();
                                } else {
                                    fs::create_dir(&source).unwrap();
                                    if component == "root" {
                                        fs::create_dir_all(source.join(".grok/hooks")).unwrap();
                                    }
                                    if component == ".grok" {
                                        fs::create_dir(source.join("hooks")).unwrap();
                                    }
                                }
                                swapped = true;
                            }
                        },
                    );
                    assert!(swapped, "test did not reach the commit boundary");
                    assert!(result.is_err(), "{resume}/{component}: {result:?}");
                    assert_eq!(fs::read(&global).unwrap(), original, "{resume}/{component}");
                }
            }
        }
    }
}

#[test]
fn grok_hook_atomic_replace_never_follows_a_swapped_leaf() {
    for stage in [Stage::BeforeRename, Stage::AfterRename] {
        let f = Fixture::new();
        f.install();
        f.prepare("");
        let global = f.root.join("home/.grok/hooks/bus.json");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        let original = b"keep-global-leaf";
        fs::write(&global, original).unwrap();
        let leaf = f.project.join(".grok/hooks/bus.json");
        let mut swapped = false;
        let result = transact(
            &f.project,
            &f.root.join("replacement-bus"),
            Mode::Resume,
            || Ok(()),
            |point| {
                if point == stage && !swapped {
                    fs::remove_file(&leaf).unwrap();
                    symlink(&global, &leaf).unwrap();
                    swapped = true;
                }
            },
        );
        assert!(swapped);
        assert_eq!(result.is_ok(), stage == Stage::BeforeRename, "{result:?}");
        assert_eq!(fs::read(global).unwrap(), original);
    }
}

#[test]
fn grok_hook_transaction_refuses_a_symlinked_lock() {
    let f = Fixture::new();
    f.install();
    f.prepare("");
    let global = f.root.join("global-sentinel");
    fs::write(&global, b"untouched").unwrap();
    let lock = f.project.join(".grok/hooks/.bus-hooks.lock");
    fs::remove_file(&lock).unwrap();
    symlink(&global, lock).unwrap();
    assert!(super::update(
        &f.project,
        &f.root.join("replacement-bus"),
        Mode::Resume,
        None,
        || Ok(())
    )
    .is_err());
    assert_eq!(fs::read(global).unwrap(), b"untouched");
}

#[test]
fn grok_global_aliases_to_a_missing_hook_fail_closed_before_creation() {
    for configured in [false, true] {
        let f = Fixture::new();
        f.install();
        let folder = f.root.join("home/.grok/hooks");
        fs::create_dir_all(&folder).unwrap();
        let hook = f.project.join(".grok/hooks/bus.json");
        symlink(&hook, folder.join("shared.json")).unwrap();
        if configured {
            let additional = f.root.join("additional");
            fs::create_dir_all(&additional).unwrap();
            fs::rename(folder.join("shared.json"), additional.join("shared.json")).unwrap();
            fs::write(
                f.root.join("home/.grok/hooks-paths"),
                format!("{}\n", additional.display()),
            )
            .unwrap();
        }
        assert!(
            super::update(&f.project, &f.root.join("bus"), Mode::Install, None, || Ok(
                ()
            ))
            .is_err()
        );
        assert!(!f.project.join(".grok").exists());
    }
}

#[test]
fn grok_hook_post_write_identity_rejects_regular_leaf_replacement() {
    let f = Fixture::new();
    f.install();
    f.prepare("");
    let leaf = f.project.join(".grok/hooks/bus.json");
    let result = transact(
        &f.project,
        &f.root.join("replacement-bus"),
        Mode::Resume,
        || Ok(()),
        |point| {
            if point == Stage::AfterRename {
                fs::remove_file(&leaf).unwrap();
                fs::write(&leaf, b"replacement from another writer").unwrap();
            }
        },
    );
    assert!(result.unwrap_err().contains("identity changed after write"));
    assert_eq!(fs::read(&leaf).unwrap(), b"replacement from another writer");
}

#[test]
fn grok_hook_transaction_refuses_a_global_hard_link() {
    let f = Fixture::new();
    f.install();
    f.prepare("");
    let leaf = f.project.join(".grok/hooks/bus.json");
    let original = fs::read(&leaf).unwrap();
    let global = f.root.join("home/.grok/hooks/shared.json");
    fs::create_dir_all(global.parent().unwrap()).unwrap();
    fs::hard_link(&leaf, &global).unwrap();
    super::update(
        &f.project,
        &f.root.join("replacement-bus"),
        Mode::Resume,
        None,
        || Ok(()),
    )
    .unwrap_err();
    assert_eq!(fs::read(&global).unwrap(), original);
    assert_eq!(fs::read(&leaf).unwrap(), original);
}

#[test]
fn grok_invalid_global_registry_does_not_authorize_setup() {
    for route in ["directory", "non-utf8", "oversize", "missing-target"] {
        let f = Fixture::new();
        f.install();
        let global = f.root.join("home/.grok/hooks-paths");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        match route {
            "directory" => fs::create_dir(&global).unwrap(),
            "non-utf8" => fs::write(&global, [255]).unwrap(),
            "oversize" => fs::write(&global, vec![b'a'; 2 * 1024 * 1024 + 1]).unwrap(),
            _ => fs::write(&global, f.root.join("missing-target").to_str().unwrap()).unwrap(),
        }
        assert!(
            super::update(&f.project, &f.root.join("bus"), Mode::Install, None, || Ok(
                ()
            ))
            .is_err(),
            "{route}"
        );
        assert!(!f.project.join(".grok").exists(), "{route}");
    }
}

#[test]
fn grok_git_exclude_hardlink_does_not_modify_global_settings() {
    let f = Fixture::new();
    f.install();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&f.project)
        .args(["init", "-q"])
        .status()
        .unwrap()
        .success());
    let global = f.root.join("home/.grok/settings.json");
    fs::create_dir_all(global.parent().unwrap()).unwrap();
    fs::write(&global, b"{}\n").unwrap();
    let exclude = f.project.join(".git/info/exclude");
    fs::remove_file(&exclude).unwrap();
    fs::hard_link(&global, &exclude).unwrap();
    f.prepare("");
    assert_eq!(
        fs::read(&global).unwrap(),
        b"{}\n",
        "global Grok settings changed through Git exclude hard link"
    );
}
