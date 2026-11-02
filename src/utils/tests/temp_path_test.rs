//! Shared unit and integration fixture paths; include once per test crate.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Return an uncreated path. PID separates processes; the sequence separates same-clock calls.
pub(crate) fn unique_temp_path(label: &str) -> PathBuf {
    unique_temp_path_at(label, SystemTime::now())
}

pub(crate) fn unique_temp_path_at(label: &str, timestamp: SystemTime) -> PathBuf {
    #[cfg(unix)]
    let root = PathBuf::from("/tmp"); // Keep private sockets below sockaddr_un limits.
    #[cfg(windows)]
    let root = std::env::temp_dir();
    root.join(format!(
        "{label}-{}-{}-{}",
        std::process::id(),
        timestamp.duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
