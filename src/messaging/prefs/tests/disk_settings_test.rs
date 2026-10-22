use super::*;
use serde_json::json;

#[test]
fn disk_settings_default_to_twenty_and_five_and_round_trip() {
    let root = crate::utils::test_temp::unique_temp_path("bus-disk-settings");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    let defaults = serde_json::to_value(load(&path).unwrap()).unwrap();
    assert_eq!(defaults["disk_warning_gb"], 20);
    assert_eq!(defaults["disk_critical_gb"], 5);
    for (warning, critical) in [(2, 1), (20, 5), (10_000, 9_999)] {
        std::fs::write(
            &path,
            json!({"disk_warning_gb": warning, "disk_critical_gb": critical}).to_string(),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        let value = serde_json::to_value(&loaded).unwrap();
        assert_eq!(value["disk_warning_gb"], warning);
        assert_eq!(value["disk_critical_gb"], critical);
        save(&path, &loaded).unwrap();
        assert_eq!(load(&path).unwrap(), loaded);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn disk_settings_reject_out_of_range_nonintegers_and_critical_at_or_above_warning() {
    let root = crate::utils::test_temp::unique_temp_path("bus-disk-settings-invalid");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    for value in [
        json!({"disk_warning_gb": 0}),
        json!({"disk_warning_gb": 10_001}),
        json!({"disk_critical_gb": 0}),
        json!({"disk_critical_gb": 10_000}),
        json!({"disk_warning_gb": 5, "disk_critical_gb": 5}),
        json!({"disk_warning_gb": 4, "disk_critical_gb": 5}),
        json!({"disk_warning_gb": -1}),
        json!({"disk_critical_gb": 1.5}),
        json!({"disk_warning_gb": "twenty"}),
    ] {
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(load(&path).is_err(), "accepted invalid settings: {value}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
