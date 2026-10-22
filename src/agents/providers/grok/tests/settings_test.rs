//! Synthetic project settings; never writes a real Grok configuration.
use super::launch_test::Fixture;
use crate::agents::providers::{hook_json, launch as common};
use serde_json::json;
#[test]
fn grok_ad_consent_absent_writes_nothing_and_never_initializes() {
    let f = Fixture::new();
    f.install();
    let err = common::prepare(&f.spec("", false), |_| panic!("no consent")).unwrap_err();
    assert!(err.contains("confirmation"));
    assert!(!f.project.join(".grok").exists());
    assert!(!f.root.join("capture").exists());
}
#[test]
fn grok_ad_settings_preserve_user_hooks_and_reject_conflict_atomically() {
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
fn grok_ad_settings_use_git_project_root_and_do_not_modify_other_hook_files() {
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
fn grok_ad_disabled_project_hooks_fail_before_files_or_initializer() {
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
