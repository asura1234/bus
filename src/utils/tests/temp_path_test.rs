//! Shared unit and integration fixture paths; include once per test crate.
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Return an uncreated path. PID separates processes; the sequence separates same-clock calls.
pub(crate) fn unique_temp_path(label: &str) -> PathBuf {
    unique_temp_path_at(label, SystemTime::now())
}

pub(crate) fn unique_temp_path_at(label: &str, timestamp: SystemTime) -> PathBuf {
    fixture_root().join(format!(
        "b{:x}-{:x}-{:x}-{label}",
        std::process::id(),
        timestamp.duration_since(UNIX_EPOCH).unwrap().as_micros(),
        NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn fixture_root() -> PathBuf {
    #[cfg(unix)]
    let root = std::fs::canonicalize("/tmp").unwrap();
    #[cfg(windows)]
    let root = std::env::temp_dir();
    root
}

/// Recognize this process's fixture namespace without admitting the whole short temp root.
pub(crate) fn is_fixture_path(path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(fixture_root()) else {
        return false;
    };
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return false;
    }
    let Some(name) = relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
    else {
        return false;
    };
    let mut parts = name.splitn(4, '-');
    parts.next() == Some(format!("b{:x}", std::process::id()).as_str())
        && parts
            .next()
            .is_some_and(|part| u128::from_str_radix(part, 16).is_ok())
        && parts
            .next()
            .is_some_and(|part| u64::from_str_radix(part, 16).is_ok())
        && parts.next().is_some_and(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn fixture_namespace_excludes_unrelated_roots_and_parent_traversal() {
        let path = unique_temp_path("namespace");
        assert!(is_fixture_path(&path));
        assert!(is_fixture_path(&path.join("project/.codex/hooks.json")));
        assert!(!is_fixture_path(path.parent().unwrap()));
        assert!(!is_fixture_path(&path.join("../unrelated/hooks.json")));
        for name in [
            "unrelated-project".to_owned(),
            format!("b{:x}-bad-clock-1-project", std::process::id()),
            format!("b{:x}-1-1-project", std::process::id() + 1),
        ] {
            assert!(!is_fixture_path(&fixture_root().join(name)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn uncreated_fixture_has_a_physical_directory_parent() {
        let path = unique_temp_path("bus-physical-root");
        assert!(!path.exists(), "allocation must not create the fixture");
        let parent = path.parent().unwrap();
        let metadata = std::fs::symlink_metadata(parent).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "fixture parent must be a physical directory: {parent:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fixture_keeps_the_real_room_client_socket_suffix_bindable() {
        use std::os::unix::net::UnixListener;

        // A modern clock and a sequence beyond the fixed-clock stress test.
        let timestamp = UNIX_EPOCH + std::time::Duration::from_secs(4_102_444_800);
        let roots: Vec<_> = (0..4096)
            .map(|_| unique_temp_path_at("bus-msg-test", timestamp))
            .collect();
        let root = roots.last().unwrap();
        let socket = root.join("runtime/herdr-config/sessions/bus/herdr-client.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket);
        std::fs::remove_dir_all(root).unwrap();
        assert!(listener.is_ok(), "cannot bind {socket:?}: {listener:?}");
    }

    #[cfg(unix)]
    #[test]
    fn same_tick_socket_paths_are_distinct_and_bind_together() {
        use std::os::unix::net::UnixListener;

        let timestamp = UNIX_EPOCH + std::time::Duration::from_secs(4_102_444_800);
        let first = unique_temp_path_at("bus-writer.sock", timestamp);
        let second = unique_temp_path_at("bus-writer.sock", timestamp);
        assert_ne!(first, second, "same-tick socket allocations collided");

        let first_listener = UnixListener::bind(&first);
        let second_listener = UnixListener::bind(&second);
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
        assert!(
            first_listener.is_ok(),
            "cannot bind {first:?}: {first_listener:?}"
        );
        assert!(
            second_listener.is_ok(),
            "cannot bind {second:?}: {second_listener:?}"
        );
    }

    #[test]
    fn concurrent_fixtures_with_one_clock_value_get_distinct_roots() {
        let timestamp = UNIX_EPOCH + std::time::Duration::from_secs(1);
        let paths: Vec<_> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(move || {
                        (0..16)
                            .map(|_| unique_temp_path_at("bus-shared-fixture", timestamp))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect()
        });
        let distinct: HashSet<_> = paths.iter().collect();
        assert_eq!(
            distinct.len(),
            paths.len(),
            "fixture roots collided: {paths:?}"
        );
    }
}
