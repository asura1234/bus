use super::*;

struct Fixture {
    root: PathBuf,
    file: PathBuf,
    exchange: Exchange,
}

impl Fixture {
    fn new(text: &str) -> Self {
        let root = crate::utils::test_temp::unique_temp_path("bus-draft-editor");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("native-editor.txt");
        fs::write(&file, text).unwrap();
        let exchange = Exchange::begin(root.join("private"), Duration::from_secs(60)).unwrap();
        Self {
            root,
            file,
            exchange,
        }
    }
    fn run(&self) -> io::Result<()> {
        run_at(Some(self.exchange.path()), &self.file, "false")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn external_editor_preserves_unicode_and_all_trailing_whitespace_through_readback() {
    let original = "\t  中文  \n\nline two \t \n\n";
    let fixture = Fixture::new(original);
    fixture.exchange.request_capture().unwrap();
    assert!(!fixture.exchange.acknowledged().unwrap());
    fixture.run().unwrap();
    assert!(fixture.exchange.acknowledged().unwrap());
    assert_eq!(fixture.exchange.captured_text().unwrap(), original);
    assert_eq!(fs::read(&fixture.file).unwrap(), b"");
    // Duplicate editor callbacks cannot replace the saved draft with emptiness.
    fixture.run().unwrap();
    assert_eq!(fixture.exchange.captured_text().unwrap(), original);
    for verify in [false, true] {
        if verify {
            fixture.exchange.request_verify().unwrap();
        } else {
            fixture.exchange.request_restore().unwrap();
        }
        assert!(
            !fixture.exchange.acknowledged().unwrap(),
            "prior operation's ack must not match"
        );
        fixture.run().unwrap();
        assert!(fixture.exchange.acknowledged().unwrap());
        let edited = fs::read_to_string(&fixture.file).unwrap();
        assert_eq!(edited, format!("{original}{}", fixture.exchange.marker()));
        // Codex trims all trailing whitespace; Cursor strips a trailing LF.
        // The sentinel protects both, and the parent removes only its bytes.
        let draft = edited
            .trim_end()
            .strip_suffix(fixture.exchange.marker())
            .unwrap();
        assert_eq!(draft, original);
        fs::write(&fixture.file, draft).unwrap();
    }
    fixture.exchange.remove_recovery().unwrap();
    assert!(!fixture.exchange.recovery().exists());
}

#[test]
fn a_live_exchange_never_quarantines_its_recovery_file() {
    let root = crate::utils::test_temp::unique_temp_path("bus-draft-live-recovery");
    let exchange = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    fs::write(exchange.recovery(), "active bytes").unwrap();
    assert_eq!(
        Exchange::begin(root.clone(), Duration::from_secs(60))
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(fs::read(exchange.recovery()).unwrap(), b"active bytes");
    drop(exchange);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn kimi_persistent_cancelled_handoff_does_not_block_later_exchange() {
    let root = crate::utils::test_temp::unique_temp_path("bus-draft-cancelled-handoff");
    let exchange = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    exchange.save_recovery("still recoverable").unwrap();
    exchange.request_capture().unwrap();
    let saved = exchange.recovery().to_owned();
    drop(exchange);
    let next = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    assert!(!saved.exists());
    assert_eq!(
        fs::read_to_string(saved.with_extension("recovered")).unwrap(),
        "still recoverable"
    );
    drop(next);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn exchange_files_and_directory_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("private draft");
    fixture.exchange.request_capture().unwrap();
    fixture.run().unwrap();
    assert_eq!(
        fs::metadata(fixture.exchange.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for path in [
        fixture.exchange.recovery().to_owned(),
        fixture.exchange.path().join(REQUEST),
        fixture.exchange.path().join(ACK),
        fixture.exchange.path().join("exchange.lock"),
        fixture.file.clone(),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn expired_and_cancelled_editor_invocations_never_clear_or_fall_back() {
    let fixture = Fixture::new("keep this draft");
    fixture.exchange.request_capture().unwrap();
    let mut request = read_request(fixture.exchange.path()).unwrap().unwrap();
    request.expires_ms = 0;
    write_json(&fixture.exchange.path().join(REQUEST), &request).unwrap();
    assert_eq!(fixture.run().unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(
        fs::read_to_string(&fixture.file).unwrap(),
        "keep this draft"
    );
    assert!(!fixture.exchange.recovery().exists());

    let root = crate::utils::test_temp::unique_temp_path("bus-draft-cancel");
    let exchange = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    let file = root.join("native.txt");
    fs::write(&file, "cancelled draft").unwrap();
    exchange.request_capture().unwrap();
    drop(exchange);
    assert!(run_at(Some(&root), &file, "false").is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "cancelled draft");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_composer_refuses_restore_and_readback_without_overwriting_user_input() {
    let fixture = Fixture::new("original \n\t");
    fixture.exchange.request_capture().unwrap();
    fixture.run().unwrap();
    for verify in [false, true] {
        fs::write(&fixture.file, "developer typed meanwhile").unwrap();
        if verify {
            fixture.exchange.request_verify().unwrap();
        } else {
            fixture.exchange.request_restore().unwrap();
        }
        assert!(fixture.run().is_err());
        assert!(fixture.exchange.acknowledged().is_err());
        assert_eq!(
            fs::read_to_string(&fixture.file).unwrap(),
            "developer typed meanwhile"
        );
        assert_eq!(fixture.exchange.captured_text().unwrap(), "original \n\t");
    }
    let recovery = fixture.exchange.recovery().to_owned();
    let dir = fixture.exchange.path().to_owned();
    let exchange = Exchange::begin(dir.join("next"), Duration::from_secs(60)).unwrap();
    exchange.save_recovery("native stash [Image #1]").unwrap();
    let native_backup = exchange.recovery().to_owned();
    drop(exchange);
    assert_eq!(
        fs::read_to_string(native_backup).unwrap(),
        "native stash [Image #1]"
    );
    assert!(recovery.exists());
}

#[test]
fn opaque_editor_placeholders_are_held_without_creating_or_clearing_a_draft() {
    for text in [
        "[Image #1] draft",
        "before [Pasted text #3 +20 lines]",
        "[Pasted Content 1024 chars]",
    ] {
        let fixture = Fixture::new(text);
        fixture.exchange.request_capture().unwrap();
        assert!(fixture.run().is_err());
        assert!(fixture.exchange.acknowledged().is_err());
        assert_eq!(fs::read_to_string(&fixture.file).unwrap(), text);
        assert!(!fixture.exchange.recovery().exists());
    }
}

#[test]
fn stale_ack_and_changed_owner_cannot_authorize_an_operation() {
    let fixture = Fixture::new("unchanged");
    fixture.exchange.request_capture().unwrap();
    let mut stale = read_request(fixture.exchange.path()).unwrap().unwrap();
    stale.sequence -= 1;
    write_json(
        &fixture.exchange.path().join(ACK),
        &Acknowledgement {
            request: stale,
            error: None,
        },
    )
    .unwrap();
    assert!(!fixture.exchange.acknowledged().unwrap());
    assert_eq!(
        Exchange::begin(fixture.exchange.path().to_owned(), Duration::from_secs(60))
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let mut replacement = read_request(fixture.exchange.path()).unwrap().unwrap();
    replacement.token = "cafe".into();
    write_json(&fixture.exchange.path().join(REQUEST), &replacement).unwrap();
    assert!(fixture.exchange.request_restore().is_err());
    assert_eq!(fs::read_to_string(&fixture.file).unwrap(), "unchanged");
}

#[test]
fn orphaned_recovery_is_quarantined_and_cancelled_native_launch_blocks_replacement() {
    let root = crate::utils::test_temp::unique_temp_path("bus-draft-unresolved");
    let exchange = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    exchange.save_recovery("still recoverable").unwrap();
    let saved = exchange.recovery().to_owned();
    drop(exchange);
    let next = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    assert!(!saved.exists());
    let recovered = saved.with_extension("recovered");
    assert_eq!(fs::read_to_string(&recovered).unwrap(), "still recoverable");
    drop(next);
    fs::remove_file(recovered).unwrap();
    let exchange = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    exchange.request_capture().unwrap();
    drop(exchange);
    let mut cancelled = read_request(&root).unwrap().unwrap();
    cancelled.expires_ms = 0;
    write_json(&root.join(REQUEST), &cancelled).unwrap();
    assert_eq!(
        Exchange::begin(root.clone(), Duration::from_secs(60))
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let file = root.join("late-native.txt");
    fs::write(&file, "unchanged").unwrap();
    assert!(test_run_at(&root, &file).is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "unchanged");
    let exchange = Exchange::begin(root.clone(), Duration::from_secs(60)).unwrap();
    drop(exchange);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(not(windows))]
#[test]
fn manual_editor_uses_original_command_arguments_and_treats_filename_as_data() {
    let root = crate::utils::test_temp::unique_temp_path("bus-manual-editor");
    fs::create_dir_all(&root).unwrap();
    let script = root.join("editor with spaces.sh");
    fs::write(&script, "printf 'manually edited' > \"$1\"\n").unwrap();
    let file = root.join("literal $(do-not-run); name.txt");
    fs::write(&file, "before").unwrap();
    let editor = format!("/bin/sh '{}'", script.display());
    run_at(Some(&root.join("not-created")), &file, &editor).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "manually edited");
    assert!(manual_editor(&file, "false").is_err());
    assert!(manual_editor(&file, "bus --bus-callback draft-editor").is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn configure_honors_explicit_editor_and_does_not_nest_an_inherited_wrapper() {
    let mut command = portable_pty::CommandBuilder::new("test");
    configure(
        &mut command,
        "w1:p2",
        &[("VISUAL".into(), "'custom editor' --wait".into())],
    )
    .unwrap();
    assert_eq!(command.get_env(FALLBACK).unwrap(), "'custom editor' --wait");
    assert!(command
        .get_env("VISUAL")
        .unwrap()
        .to_string_lossy()
        .ends_with(" --bus-callback draft-editor"));
    assert_eq!(command.get_env("VISUAL"), command.get_env("EDITOR"));
    configure(&mut command, "w1:p2", &[]).unwrap();
    assert_eq!(command.get_env(FALLBACK).unwrap(), "'custom editor' --wait");
    let path = directory("../../pane:1");
    assert!(path.starts_with(crate::utils::paths::data_dir().join("private/drafts")));
    assert!(!path.file_name().unwrap().to_string_lossy().contains('.'));
    assert!(run(&["bus".into(), "--bus-callback".into(), "draft-editor".into()]).is_err());
}
