use super::*;

#[test]
fn developer_sound_setting_survives_saved_settings_round_trip() {
    let root = crate::utils::test_temp::unique_temp_path("bus-developer-sound-setting");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    std::fs::write(&path, r#"{"developer_sound":"Glass"}"#).unwrap();
    let saved = load(&path).unwrap();
    assert!(
        saved.developer_sound_enabled,
        "legacy settings keep alerts audible"
    );
    let round_trip = serde_json::to_value(saved).unwrap();
    assert_eq!(round_trip["developer_sound"], "Glass");
    assert_eq!(round_trip["feishu_waiting_notifications"], true);
    assert_eq!(BusSettings::default().developer_sound, None);
    assert!(BusSettings::default().developer_sound_enabled);
    let muted = update(&path, |settings| settings.developer_sound_enabled = false).unwrap();
    assert!(!muted.developer_sound_enabled);
    assert_eq!(muted.developer_sound.as_deref(), Some("Glass"));
    assert!(muted.feishu_notifications);
    assert_eq!(load(&path).unwrap(), muted);
    std::fs::remove_dir_all(root).unwrap();
}
