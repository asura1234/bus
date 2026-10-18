//! Age and byte limits for completed stack dumps. Never removes active files.
use crate::utils::logging::ArchivePolicy;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub(super) struct Eviction {
    pub(super) file: PathBuf,
    pub(super) bytes: u64,
    pub(super) reason: &'static str,
}
pub(super) fn prune(
    dir: &Path,
    role: &str,
    active: &BTreeSet<String>,
    policy: ArchivePolicy,
    now: SystemTime,
) -> Vec<Eviction> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let suffix = format!("-{role}.txt");
    let partial_suffix = format!("{suffix}.part");
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("stall-")
                || !(name.ends_with(&suffix) || name.ends_with(&partial_suffix))
                || active.contains(&name)
            {
                return None;
            }
            if name.ends_with(&partial_suffix) {
                let lease = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(entry.path().with_extension("lock"));
                match lease {
                    Ok(lease) if lease.try_lock().is_err() => return None,
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => return None,
                    _ => {}
                }
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    entry.path(),
                    metadata.len(),
                    metadata.modified().unwrap_or(now),
                )
            })
        })
        .collect();
    files.sort_by_key(|(path, _, modified)| (*modified, path.clone()));
    let mut total: u64 = files
        .iter()
        .fold(0_u64, |total, (_, bytes, _)| total.saturating_add(*bytes));
    let mut evicted = Vec::new();
    for (file, bytes, modified) in files {
        let reason = if now.duration_since(modified).unwrap_or(Duration::ZERO) > policy.max_age {
            "age"
        } else if total > policy.max_total_bytes {
            "bytes"
        } else {
            continue;
        };
        if std::fs::remove_file(&file).is_ok() {
            if file.extension().is_some_and(|ext| ext == "part") {
                let _ = std::fs::remove_file(file.with_extension("lock"));
            }
            total = total.saturating_sub(bytes);
            evicted.push(Eviction {
                file,
                bytes,
                reason,
            });
        }
    }
    evicted
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn age_and_byte_caps_keep_more_than_five_small_dumps_and_protect_active_files() {
        let dir = crate::utils::test_temp::unique_temp_path("freeze-retention");
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for n in 0..8 {
            std::fs::write(dir.join(format!("stall-{n}-client.txt")), [0; 10]).unwrap();
        }
        let policy = ArchivePolicy {
            max_age: Duration::from_secs(60),
            max_total_bytes: 100,
        };
        assert!(prune(&dir, "client", &BTreeSet::new(), policy, now).is_empty());
        let active = BTreeSet::from(["stall-0-client.txt".to_string()]);
        let removed = prune(
            &dir,
            "client",
            &active,
            ArchivePolicy {
                max_total_bytes: 20,
                ..policy
            },
            now,
        );
        assert_eq!(removed.len(), 5);
        assert!(removed.iter().all(|e| e.reason == "bytes"));
        assert!(dir.join("stall-0-client.txt").exists());
        let removed = prune(
            &dir,
            "client",
            &active,
            policy,
            now + Duration::from_secs(61),
        );
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().all(|e| e.reason == "age"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn live_partial_dumps_are_protected_and_orphans_obey_the_byte_cap() {
        let dir = crate::utils::test_temp::unique_temp_path("freeze-partial-lease");
        std::fs::create_dir_all(&dir).unwrap();
        let partial = dir.join("stall-1-client.txt.part");
        std::fs::write(&partial, [0; 64]).unwrap();
        let lock = partial.with_extension("lock");
        let lease = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)
            .unwrap();
        lease.try_lock().unwrap();
        let policy = ArchivePolicy {
            max_age: Duration::from_secs(60),
            max_total_bytes: 0,
        };
        assert!(prune(&dir, "client", &BTreeSet::new(), policy, SystemTime::now()).is_empty());
        drop(lease);
        assert_eq!(
            prune(&dir, "client", &BTreeSet::new(), policy, SystemTime::now()).len(),
            1
        );
        assert!(!partial.exists());
        assert!(!lock.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
