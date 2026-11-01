use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn home() -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "bus-skills-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn bus(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("BUS_DATA_DIR")
        .env_remove("BUS_SESSION_ID")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .current_dir(home)
        .output()
        .unwrap()
}

fn json(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn skills_status_is_read_only_without_a_session_and_install_copies_every_provider() {
    let home = home();
    let status = json(bus(&home, &["skills", "status"]));
    assert!(
        status.get("answer").is_none(),
        "status has no install consent"
    );
    assert!(status["build_sha"]
        .as_str()
        .is_some_and(|sha| !sha.is_empty()));
    assert!(status["source_root"]
        .as_str()
        .is_some_and(|root| !root.is_empty()));
    assert!(status["skills"].as_array().unwrap().len() >= 30);
    assert!(!home.join(".local").exists(), "status must not write state");
    let installed = json(bus(&home, &["skills", "install", "--yes"]));
    assert!(installed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .all(|skill| skill["action"] == "installed"));
    for provider in [".claude", ".agents", ".cursor", ".grok", ".kimi-code"] {
        let skill = home.join(provider).join("skills/review-pr/SKILL.md");
        assert!(skill.is_file(), "{}", skill.display());
        assert!(!std::fs::symlink_metadata(skill)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(home
            .join(provider)
            .join("skills/_bus/cli_extensions/review_artifact.py")
            .is_file());
    }
    assert!(
        !home.join(".codex").exists(),
        "Codex user skills live in ~/.agents/skills"
    );
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join(".local/share/bus/skills-manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["schema"], 1);
    assert_eq!(manifest["bundle_hash"], installed["bundle_hash"]);
    let status = json(bus(&home, &["skills", "status"]));
    assert!(
        status.get("answer").is_none(),
        "install does not record consent"
    );
    assert!(!home.join(".local/share/bus/settings.json").exists());
    assert!(status["skills"]
        .as_array()
        .unwrap()
        .iter()
        .all(|skill| { skill["state"] == "installed" && skill.get("action").is_none() }));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn install_never_adopts_or_overwrites_user_skills_or_modified_bus_copies() {
    let home = home();
    let user = home.join(".claude/skills/pr");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(user.join("SKILL.md"), "user skill").unwrap();
    let report = json(bus(&home, &["skills", "install", "--yes"]));
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(user.join("SKILL.md")).unwrap(),
        "user skill"
    );
    let preserved = report["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["provider"] == "claude" && skill["name"] == "pr")
        .unwrap();
    assert_eq!(preserved["state"], "conflict");
    assert_eq!(preserved["action"], "preserved-conflict");
    let owned = home.join(".agents/skills/review-pr/SKILL.md");
    std::fs::write(&owned, "user edit of Bus skill").unwrap();
    let report = json(bus(&home, &["skills", "install", "--yes"]));
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(&owned).unwrap(),
        "user edit of Bus skill"
    );
    let edited = report["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["provider"] == "codex" && skill["name"] == "review-pr")
        .unwrap();
    assert_eq!(edited["state"], "conflict");
    assert_eq!(edited["action"], "preserved-conflict");
    std::fs::remove_dir_all(home).unwrap();
}
