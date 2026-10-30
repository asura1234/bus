use super::*;

#[test]
fn a_decline_is_saved_for_this_bundle_and_eof_keeps_the_offer_pending() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-consent");
    std::fs::create_dir_all(&home).unwrap();
    let mut output = Vec::new();
    offer(&home, &mut &b""[..], &mut output).unwrap();
    assert!(skills::needs_offer(&home).unwrap());
    offer(&home, &mut &b"invalid\nno\n"[..], &mut output).unwrap();
    assert!(!skills::needs_offer(&home).unwrap());
    let answer = skills::answer(&home).unwrap().unwrap();
    assert!(!answer.accepted);
    assert_eq!(answer.bundle_hash, skills::bundle_hash());
    assert!(!home.join(".claude").exists());
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Install the following set of default skills?"));
    for name in skills::skill_names() {
        assert!(text.contains(name));
    }
    let settings_path = home.join(".local/share/bus/settings.json");
    let mut settings = crate::messaging::prefs::settings::load(&settings_path).unwrap();
    settings.skills_answer.as_mut().unwrap().bundle_hash = "previous-bundle".into();
    crate::messaging::prefs::settings::save(&settings_path, &settings).unwrap();
    assert!(skills::needs_offer(&home).unwrap());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn accepting_installs_skills_and_keeps_unrelated_settings() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-yes");
    let path = home.join(".local/share/bus/settings.json");
    crate::messaging::prefs::settings::update(&path, |settings| settings.color_blind_mode = true)
        .unwrap();
    offer(&home, &mut &b"yes\n"[..], &mut Vec::new()).unwrap();
    assert!(!skills::needs_offer(&home).unwrap());
    assert!(home.join(".agents/skills/review-pr/SKILL.md").is_file());
    assert!(
        crate::messaging::prefs::settings::load(&path)
            .unwrap()
            .color_blind_mode
    );
    std::fs::remove_dir_all(home).unwrap();
}
