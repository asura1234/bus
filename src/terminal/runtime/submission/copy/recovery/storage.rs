//! Durable files and reversible deletion for copied draft recovery.
#[cfg(test)]
use super::test_hooks;
use super::Key;
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_RECORDS: usize = 16;
const MAX_BYTES: u64 = 8 * 1024 * 1024;

fn unique_path(path: &Path, digest: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = crate::utils::time::now_ns();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        "copied-{digest}-{}-{stamp}-{sequence}.txt",
        std::process::id()
    ))
}

pub(super) fn save(key: &Key, snapshot: &[u8]) -> io::Result<PathBuf> {
    let Key { directory, digest } = key;
    crate::platform::fs::private_dir(directory)?;
    let records = records(directory)?;
    if let Some(record) = records
        .iter()
        .find(|record| record.digest == *digest && matches_snapshot(&record.path, snapshot))
    {
        return Ok(record.path.clone());
    }

    #[cfg(test)]
    test_hooks::before_write(directory)?;
    let primary = directory.join(format!("copied-{digest}.txt"));
    let (path, mut file) = match crate::platform::create_private_state_file(&primary) {
        Ok(file) => (primary, file),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                || fs::symlink_metadata(&primary).is_ok() =>
        {
            let alternate = unique_path(&primary, digest);
            let file = crate::platform::create_private_state_file(&alternate)?;
            (alternate, file)
        }
        Err(error) => return Err(error),
    };
    let written = file.write_all(snapshot).and_then(|()| file.sync_all());
    drop(file);
    let published = written
        .and_then(|()| crate::platform::sync_parent_directory(directory))
        .and_then(|()| {
            matches_snapshot(&path, snapshot)
                .then_some(())
                .ok_or_else(|| io::Error::other("Copied draft could not be read back"))
        });
    if let Err(error) = published {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn matches_snapshot(path: &Path, snapshot: &[u8]) -> bool {
    read_private(path).is_some_and(|bytes| bytes == snapshot)
}

pub(super) fn verified_records(key: &Key) -> io::Result<Vec<PathBuf>> {
    let records = match records(&key.directory) {
        Ok(records) => records,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(records
        .into_iter()
        .filter(|record| {
            record.digest == key.digest
                && read_private(&record.path)
                    .is_some_and(|bytes| crate::utils::time::digest(&bytes) == key.digest)
        })
        .map(|record| record.path)
        .collect())
}

fn read_private(path: &Path) -> Option<Vec<u8>> {
    fn open(path: &Path) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        options.open(path)
    }
    let file = fs::symlink_metadata(path)
        .ok()
        .filter(private_file)
        .and_then(|_| open(path).ok())?;
    let metadata = file.metadata().ok().filter(private_file)?;
    let size = metadata.len();
    if size > crate::platform::draft_editor::MAX_BYTES {
        return None;
    }
    let mut contents = Vec::with_capacity(size as usize);
    file.take(size + 1).read_to_end(&mut contents).ok()?;
    (contents.len() as u64 == size).then_some(contents)
}

fn private_file(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return false;
        }
    }
    metadata.file_type().is_file()
}

pub(super) struct Record {
    path: PathBuf,
    digest: String,
    metadata: Metadata,
}

pub(super) fn records(directory: &Path) -> io::Result<Vec<Record>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((digest, suffix)) = name
            .to_str()
            .and_then(|name| name.strip_prefix("copied-")?.strip_suffix(".txt"))
            .map(|stem| stem.split_once('-').unwrap_or((stem, "")))
        else {
            continue;
        };
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-')
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if private_file(&metadata) {
            records.push(Record {
                path,
                digest: digest.to_owned(),
                metadata,
            });
        }
    }
    Ok(records)
}

pub(super) fn prune(
    directory: &Path,
    current: &Path,
    can_discard: impl Fn(&str) -> bool,
) -> io::Result<bool> {
    let mut records = records(directory)?;
    let mut count = records.len();
    let mut bytes = records.iter().fold(0_u64, |sum, record| {
        sum.saturating_add(record.metadata.len())
    });
    records.sort_unstable_by(|a, b| {
        a.metadata
            .modified()
            .ok()
            .cmp(&b.metadata.modified().ok())
            .then_with(|| a.path.cmp(&b.path))
    });
    for record in records {
        if count <= MAX_RECORDS && bytes <= MAX_BYTES {
            break;
        }
        if record.path != current
            && discard_if(&record.path, &record.digest, || can_discard(&record.digest))?
        {
            count -= 1;
            bytes = bytes.saturating_sub(record.metadata.len());
        }
    }
    Ok(count <= MAX_RECORDS && bytes <= MAX_BYTES)
}

// Stage the pathname change reversibly. The final authorization after
// rename is the deletion's linearization point. A registration arriving
// during rename wins and the worker puts the original name back. If Bus
// exits during staging, the alternate name is still a managed record.
pub(super) fn discard_if(
    path: &Path,
    digest: &str,
    allowed: impl Fn() -> bool,
) -> io::Result<bool> {
    if !allowed() {
        return Ok(false);
    }
    let staged = unique_path(path, digest);
    fs::rename(path, &staged)?;
    #[cfg(test)]
    test_hooks::before_effect(path);
    if !allowed() {
        fs::rename(staged, path)?;
        return Ok(false);
    }
    fs::remove_file(staged)?;
    Ok(true)
}
