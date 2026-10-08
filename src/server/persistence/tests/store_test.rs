use super::*;
use crate::server::persistence::schema::{
    PaneHistorySnapshot, TabHistorySnapshot, WorkspaceHistorySnapshot,
};

fn temp_session_path(name: &str) -> PathBuf {
    let unique = format!(
        "herdr-session-tests-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::env::temp_dir().join(unique).join("session.json")
}

fn temp_session_paths(name: &str) -> (PathBuf, PathBuf) {
    let session = temp_session_path(name);
    let history = session.with_file_name("session-history.json");
    (session, history)
}

fn empty_snapshot() -> SessionSnapshot {
    SessionSnapshot {
        version: SNAPSHOT_VERSION,
        workspaces: vec![],
        active: None,
        selected: 0,
    }
}

fn history_snapshot(secret: &str) -> SessionHistorySnapshot {
    SessionHistorySnapshot {
        version: SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceHistorySnapshot {
            tabs: vec![TabHistorySnapshot {
                panes: std::collections::HashMap::from([(
                    0,
                    PaneHistorySnapshot {
                        ansi: secret.to_string(),
                        lines: 1,
                    },
                )]),
            }],
        }],
    }
}

#[test]
fn save_to_paths_writes_pane_history_only_to_history_file() {
    let (session_path, history_path) = temp_session_paths("split-history");

    save_to_paths(
        &session_path,
        &history_path,
        &empty_snapshot(),
        Some(&history_snapshot("split-secret")),
    )
    .unwrap();

    let session = std::fs::read_to_string(&session_path).unwrap();
    let history = std::fs::read_to_string(&history_path).unwrap();
    assert!(!session.contains("split-secret"));
    assert!(!session.contains("history"));
    assert!(history.contains("split-secret"));
}

#[test]
fn save_to_paths_removes_stale_history_when_history_is_disabled() {
    let (session_path, history_path) = temp_session_paths("clear-history");
    save_to_paths(
        &session_path,
        &history_path,
        &empty_snapshot(),
        Some(&history_snapshot("stale-secret")),
    )
    .unwrap();

    save_to_paths(&session_path, &history_path, &empty_snapshot(), None).unwrap();

    assert!(session_path.exists());
    assert!(!history_path.exists());
}

#[test]
fn clear_path_removes_existing_session_file() {
    let path = temp_session_path("clear-existing");
    save_to_path(&path, &empty_snapshot()).unwrap();

    clear_path(&path).unwrap();

    assert!(!path.exists());
}

#[test]
fn clear_path_ignores_missing_session_file() {
    let path = temp_session_path("clear-missing");

    clear_path(&path).unwrap();

    assert!(!path.exists());
}

#[test]
fn save_to_path_cleans_temp_after_failed_replace() {
    let path = temp_session_path("failed-replace");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("keep"), b"original").unwrap();
    assert!(save_to_path(&path, &empty_snapshot()).is_err());
    assert_eq!(std::fs::read(path.join("keep")).unwrap(), b"original");
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn saved_session_and_history_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let (session, history) = temp_session_paths("permissions");
    save_to_paths(
        &session,
        &history,
        &empty_snapshot(),
        Some(&history_snapshot("secret")),
    )
    .unwrap();
    for path in [&session, &history] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(session.parent().unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn save_to_path_preserves_existing_symlink() {
    let target = temp_session_path("symlink-target");
    let link = target.with_file_name("link.json");
    save_to_path(&target, &empty_snapshot()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let mut snap = empty_snapshot();
    snap.selected = 7;
    save_to_path(&link, &snap).unwrap();

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    let parsed = parse_snapshot(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(parsed.selected, 7);
}

#[cfg(unix)]
#[test]
fn save_to_path_writes_through_dangling_symlink() {
    let target = temp_session_path("dangling-target");
    let link = target.with_file_name("link.json");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    save_to_path(&link, &empty_snapshot()).unwrap();

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(target.exists());
}

#[cfg(unix)]
#[test]
fn save_to_path_resolves_relative_symlink() {
    let session = temp_session_path("relative-symlink");
    let dir = session.parent().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let target = dir.join("real.json");
    let link = dir.join("link.json");
    std::os::unix::fs::symlink("real.json", &link).unwrap();

    save_to_path(&link, &empty_snapshot()).unwrap();

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(target.exists());
}
