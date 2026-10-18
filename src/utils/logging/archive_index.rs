//! Timestamp bounds written by the archive worker while it compresses perf JSONL.
//! Sidecars describe immutable archive bytes, never the live file or its mtime.
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Timestamp {
    ts_ms: u64,
}

/// Validates JSON while ignoring other fields without constructing their values.
pub(crate) fn timestamp(line: &[u8]) -> serde_json::Result<u64> {
    if line.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(serde_json::Error::io(io::Error::new(
            io::ErrorKind::InvalidData,
            "perf record must be a JSON object",
        )));
    }
    serde_json::from_slice::<Timestamp>(line).map(|record| record.ts_ms)
}

#[derive(Deserialize, Serialize)]
pub(crate) struct ArchiveIndex {
    version: u8,
    pub(crate) first_ts_ms: u64,
    pub(crate) last_ts_ms: u64,
    pub(crate) records: u64,
    pub(crate) invalid: u64,
    plain_bytes: u64,
    gzip_bytes: u64,
}

impl Default for ArchiveIndex {
    fn default() -> Self {
        Self {
            version: 1,
            first_ts_ms: u64::MAX,
            last_ts_ms: 0,
            records: 0,
            invalid: 0,
            plain_bytes: 0,
            gzip_bytes: 0,
        }
    }
}

fn index_path(archive: &Path) -> PathBuf {
    let plain = if archive.extension().is_some_and(|ext| ext == "gz") {
        archive.with_extension("")
    } else {
        archive.to_owned()
    };
    let mut name = plain.into_os_string();
    name.push(".index.json");
    PathBuf::from(name)
}

/// A missing, interrupted, unknown-version or mismatched index means scan.
pub(crate) fn read(archive: &Path) -> Option<ArchiveIndex> {
    let file = File::open(index_path(archive)).ok()?;
    let index: ArchiveIndex = serde_json::from_reader(file.take(4096)).ok()?;
    let bytes = if archive.extension().is_some_and(|ext| ext == "gz") {
        index.gzip_bytes
    } else {
        index.plain_bytes
    };
    (index.version == 1
        && index.records > 0
        && index.first_ts_ms <= index.last_ts_ms
        && fs::metadata(archive).ok()?.len() == bytes)
        .then_some(index)
}

pub(super) fn remove(archive: &Path) {
    let _ = fs::remove_file(index_path(archive));
}

fn is_perf_archive(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("client.jsonl.") || name.starts_with("server.jsonl."))
}

fn copy_indexed(source: &File, target: &mut impl Write) -> io::Result<ArchiveIndex> {
    let mut index = ArchiveIndex::default();
    let mut reader = BufReader::new(source);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        target.write_all(&line)?;
        index.plain_bytes += line.len() as u64;
        match timestamp(&line) {
            Ok(ts) => {
                // Bounds, rather than append order: different processes and
                // wall-clock adjustments can put recent records in old files.
                index.first_ts_ms = index.first_ts_ms.min(ts);
                index.last_ts_ms = index.last_ts_ms.max(ts);
                index.records += 1;
            }
            Err(_) => index.invalid += 1,
        }
        line.clear();
    }
    Ok(index)
}

/// Publishes the index atomically only after the gzip has been finalized.
fn publish(index: &ArchiveIndex, archive: &Path, staging: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging)?;
    serde_json::to_writer(&file, index)?;
    file.sync_all()?;
    fs::rename(staging, index_path(archive))
}

/// Each worker owns its staging files; concurrent workers may finish the same
/// immutable input. A crash or index write failure leaves a scannable archive.
pub(super) fn compress_archive(plain: &Path) -> io::Result<PathBuf> {
    static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut compressed = plain.as_os_str().to_os_string();
    compressed.push(super::ARCHIVE_GZ);
    let compressed = PathBuf::from(compressed);
    let mut staging = compressed.clone().into_os_string();
    let sequence = STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    staging.push(format!(
        ".{}-{sequence}{}",
        std::process::id(),
        super::ARCHIVE_TMP
    ));
    let staging = PathBuf::from(staging);
    let index_staging = staging.clone();
    let source = File::open(plain)?;
    let modified = source.metadata()?.modified()?;
    let staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
    let result = (|| {
        let mut encoder = flate2::write::GzEncoder::new(staged, flate2::Compression::default());
        let mut index = if is_perf_archive(plain) {
            Some(copy_indexed(&source, &mut encoder)?)
        } else {
            io::copy(&mut &source, &mut encoder)?;
            None
        };
        let finished = encoder.finish()?;
        // Keep the existing numbered-archive retention age across compression.
        finished.set_times(fs::FileTimes::new().set_modified(modified))?;
        finished.sync_all()?;
        fs::rename(&staging, &compressed)?;
        if let Some(index) = &mut index {
            index.gzip_bytes = fs::metadata(&compressed)?.len();
            let _ = publish(index, &compressed, &index_staging);
        }
        Ok(())
    })();
    let _ = fs::remove_file(&index_staging);
    if let Err(err) = result {
        let _ = fs::remove_file(&staging);
        return Err(err);
    }
    match fs::remove_file(plain) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    Ok(compressed)
}
