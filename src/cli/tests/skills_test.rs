use super::*;

#[test]
fn a_decline_installs_nothing_and_writes_no_settings() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-consent");
    std::fs::create_dir_all(&home).unwrap();
    let mut output = Vec::new();
    offer(&home, &mut &b""[..], &mut output).unwrap();
    offer(&home, &mut &b"invalid\nno\n"[..], &mut output).unwrap();
    assert!(!home.join(".claude").exists());
    assert!(!home.join(".agents").exists());
    assert!(!home.join(".local/share/bus/settings.json").exists());
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Install the following set of default skills?"));
    assert!(text.contains("Install? [y/N]"));
    for name in skills::skill_names() {
        assert!(text.contains(name));
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn accepting_installs_skills_and_keeps_unrelated_settings() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-yes");
    let path = home.join(".local/share/bus/settings.json");
    crate::messaging::prefs::settings::update(&path, |settings| settings.color_blind_mode = true)
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let report = offer(&home, &mut &b"yes\n"[..], &mut Vec::new())
        .unwrap()
        .expect("yes installs");
    let json = serde_json::to_value(&report).unwrap();
    assert!(json["skills"]
        .as_array()
        .unwrap()
        .iter()
        .all(|skill| skill["action"] == "installed"));
    assert!(home.join(".agents/skills/review-pr/SKILL.md").is_file());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        crate::messaging::prefs::settings::load(&path)
            .unwrap()
            .color_blind_mode
    );
    std::fs::remove_dir_all(home).unwrap();
}
