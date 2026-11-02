//! Restart identity regressions. All homes, binaries and records are synthetic.
use super::*;

fn rewrite_record(fixture: &Fixture, name: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = fixture.hooks.spool.join(name);
    let mut value = hook_json::read_resume_json(&path).unwrap();
    edit(&mut value);
    hook_json::atomic_write(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
}

// Model the incident's saved st_dev from a previous macOS mount. Neither the
// directory nor the executable changed. A Bus-only restart cannot remount a disk.
#[cfg(target_os = "macos")]
pub(crate) fn previous_mount(fixture: &Fixture, legacy: bool) {
    rewrite_record(fixture, HOME_FILE, |value| {
        let device = value["device"].as_u64().unwrap();
        value["device"] = (device + 1).into();
        if legacy {
            value.as_object_mut().unwrap().remove("volume_uuid");
        }
    });
    rewrite_record(fixture, EXECUTABLE_FILE, |value| {
        let stamp = &mut value[1];
        let device = stamp["device"].as_u64().unwrap();
        stamp["device"] = (device + 1).into();
        if legacy {
            stamp.as_object_mut().unwrap().remove("volume_uuid");
        }
    });
}

pub(crate) fn use_home(fixture: &Fixture) -> PathBuf {
    let home = saved_home(&fixture.hooks).unwrap();
    std::fs::create_dir_all(home.join("sessions/owned-session")).unwrap();
    std::fs::write(
        home.join("sessions/owned-session/context.jsonl"),
        b"synthetic",
    )
    .unwrap();
    // Native atomic token/trust writes do not replace the private home.
    for folder in ["credentials", "workspace-trust"] {
        std::fs::write(home.join(folder).join("pending"), b"synthetic").unwrap();
        std::fs::rename(
            home.join(folder).join("pending"),
            home.join(folder).join("record"),
        )
        .unwrap();
    }
    home
}

fn resume(fixture: &Fixture) -> Result<(), String> {
    super::super::super::resume::capture_args(&fixture.hooks, &fixture.root)?;
    environment(&fixture.hooks)?;
    Ok(())
}

#[test]
fn restart_after_normal_use_preserves_home_and_rebound_binary() {
    let mut fixture = Fixture::new();
    fixture.prepare();
    let home = use_home(&fixture);
    fixture.hooks.binary = fixture.root.join("new-bus");
    resume(&fixture).unwrap();
    resume(&fixture).unwrap();
    assert_eq!(saved_home(&fixture.hooks).unwrap(), home);
    assert!(std::fs::read_to_string(home.join("config.toml"))
        .unwrap()
        .contains("new-bus"));
}

#[test]
#[cfg(target_os = "macos")]
fn restart_after_device_renumbering_preserves_used_home() {
    let fixture = Fixture::new();
    fixture.prepare();
    let home = use_home(&fixture);
    previous_mount(&fixture, false);
    resume(&fixture).unwrap();
    assert_eq!(saved_home(&fixture.hooks).unwrap(), home);
}

#[test]
#[cfg(target_os = "macos")]
fn restart_upgrades_legacy_home_after_device_renumbering_and_repairs() {
    let mut fixture = Fixture::new();
    fixture.prepare();
    let home = use_home(&fixture);
    // A home captured before #266/#267: no update setting and private trust.
    std::fs::write(home.join("tui.toml"), "theme = \"dark\"\n").unwrap();
    std::fs::remove_file(home.join("workspace-trust")).unwrap();
    crate::platform::fs::private_dir(&home.join("workspace-trust")).unwrap();
    std::fs::write(
        home.join("workspace-trust/decision"),
        br#"{"root":"/synthetic/project","trustedAt":123}"#,
    )
    .unwrap();
    previous_mount(&fixture, true);
    fixture.hooks.binary = fixture.root.join("new-bus");
    resume(&fixture).unwrap();
    resume(&fixture).unwrap();
    assert_eq!(saved_home(&fixture.hooks).unwrap(), home);
    assert!(home.join("workspace-trust").is_symlink());
    assert!(home.join("workspace-trust/decision").is_file());
    let tui = std::fs::read_to_string(home.join("tui.toml")).unwrap();
    assert!(tui.contains("auto_install = false"));
    assert!(tui.contains("theme = \"dark\""));
    assert_eq!(
        environment(&fixture.hooks).unwrap()[NO_AUTO_UPDATE_ENV],
        "1"
    );
    for (name, index) in [(HOME_FILE, None), (EXECUTABLE_FILE, Some(1))] {
        let value = hook_json::read_resume_json(&fixture.hooks.spool.join(name)).unwrap();
        let record = index.map_or(&value, |index| &value[index]);
        assert!(record["volume_uuid"].is_array(), "{name}: {record}");
    }
}

#[test]
#[cfg(target_os = "macos")]
fn restart_executable_accepts_only_device_drift() {
    let fixture = Fixture::new();
    fixture.prepare();
    previous_mount(&fixture, true);
    assert_eq!(
        saved_executable(&fixture.hooks).unwrap(),
        fixture.executable.canonicalize().unwrap()
    );
    std::fs::write(&fixture.executable, b"replaced executable").unwrap();
    assert!(saved_executable(&fixture.hooks).is_err());
}

#[test]
fn restart_refuses_replaced_home_even_with_matching_auth_links() {
    let fixture = Fixture::new();
    fixture.prepare();
    let home = use_home(&fixture);
    #[cfg(target_os = "macos")]
    previous_mount(&fixture, true);
    // Retain the old directory so the filesystem cannot recycle its inode.
    std::fs::rename(&home, fixture.hooks.spool.join("old-home")).unwrap();
    private_home(&fixture.source, &fixture.hooks).unwrap();
    assert!(resume(&fixture).is_err());
}

#[test]
fn restart_refuses_foreign_callback_home_and_auth_source() {
    let fixture = Fixture::new();
    fixture.prepare();
    let home = use_home(&fixture);
    rewrite_record(&fixture, HOME_FILE, |value| {
        value["path"] = fixture.root.join("foreign-home").to_string_lossy().into();
    });
    assert!(resume(&fixture).is_err());
    rewrite_record(&fixture, HOME_FILE, |value| {
        value["path"] = home.to_string_lossy().into();
    });
    std::fs::remove_file(home.join("oauth")).unwrap();
    std::fs::create_dir(home.join("oauth")).unwrap();
    assert!(resume(&fixture).is_err());
}

#[test]
#[cfg(target_os = "macos")]
fn restart_refuses_foreign_volume_with_the_same_inode() {
    let fixture = Fixture::new();
    fixture.prepare();
    // An inode is only unique within its volume. A UUID mismatch must refuse
    // even if a foreign filesystem happens to reuse the saved inode/device.
    rewrite_record(&fixture, HOME_FILE, |value| {
        value["volume_uuid"] = serde_json::to_value([0_u8; 16]).unwrap();
    });
    assert!(resume(&fixture).is_err());
}

#[test]
#[cfg(target_os = "macos")]
fn restart_legacy_device_drift_requires_the_same_executable_mount_transition() {
    let fixture = Fixture::new();
    fixture.prepare();
    previous_mount(&fixture, true);
    rewrite_record(&fixture, EXECUTABLE_FILE, |value| {
        let device = value[1]["device"].as_u64().unwrap();
        value[1]["device"] = (device + 1).into();
    });
    assert!(resume(&fixture).is_err());
}
