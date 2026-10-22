use super::{transact, Mode, Stage};
use crate::agents::providers::grok::launch::launch_test::Fixture;
use std::{fs, os::unix::fs::symlink};

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
                f.root.join("home/.grok/config.toml"),
                format!(
                    "hooks-paths = [{}]\n",
                    serde_json::to_string(&additional.to_string_lossy()).unwrap()
                ),
            )
            .unwrap();
        }
        assert!(super::update(&f.project, &f.root.join("bus"), Mode::Install, || Ok(())).is_err());
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
fn grok_hook_atomic_replacement_keeps_a_global_hard_link_unchanged() {
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
        || Ok(()),
    )
    .unwrap();
    assert_eq!(fs::read(&global).unwrap(), original);
    assert_ne!(fs::read(&leaf).unwrap(), original);
}

#[test]
fn grok_unreadable_global_root_configuration_does_not_authorize_setup() {
    for contents in [
        "[bad toml",
        "hooks-paths = 'not-an-array'",
        "hooks-paths = [42]",
    ] {
        let f = Fixture::new();
        f.install();
        let global = f.root.join("home/.grok/config.toml");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        fs::write(&global, contents).unwrap();
        assert!(super::update(&f.project, &f.root.join("bus"), Mode::Install, || Ok(())).is_err());
        assert!(!f.project.join(".grok").exists());
        assert_eq!(fs::read_to_string(&global).unwrap(), contents);
    }
}
