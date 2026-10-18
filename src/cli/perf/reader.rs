//! Streaming perf JSONL reader. Only records inside the selected window become Values.
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::utils::logging::{archive_index, archived_files};

#[derive(Default)]
pub(super) struct Scan {
    pub(super) records: Vec<Value>,
    pub(super) stats: Stats,
}

#[derive(Default)]
pub(super) struct Stats {
    pub(super) files_read: u64,
    pub(super) archives_skipped: u64,
    pub(super) records_read: u64,
    pub(super) records_skipped: u64,
    pub(super) invalid: u64,
    pub(super) unreadable: u64,
}

pub(super) fn read_records(data_dir: &Path, from_ms: Option<u64>) -> Scan {
    read_records_with(data_dir, from_ms, open)
}

pub(super) fn open(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = BufReader::new(std::fs::File::open(path)?);
    if path.extension().is_some_and(|ext| ext == "gz") {
        Ok(Box::new(BufReader::new(flate2::read::GzDecoder::new(file))))
    } else {
        Ok(Box::new(file))
    }
}

pub(super) fn read_records_with(
    data_dir: &Path,
    from_ms: Option<u64>,
    mut open: impl FnMut(&Path) -> io::Result<Box<dyn BufRead>>,
) -> Scan {
    let dir = data_dir.join("perf");
    let mut scan = Scan::default();
    for role in super::ROLES {
        let live = dir.join(format!("{role}.jsonl"));
        for path in archived_files(&live)
            .into_iter()
            .chain(std::iter::once(live.clone()))
        {
            if path != live && skip_archive(&path, from_ms, &mut scan.stats) {
                continue;
            }
            match open(&path) {
                Ok(reader) => {
                    scan.stats.files_read += 1;
                    read_lines(reader, from_ms, &mut scan);
                }
                Err(error) if path == live && error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => scan.stats.unreadable += 1,
            }
        }
    }
    scan.records.sort_by_key(super::ts);
    scan
}

fn skip_archive(path: &Path, from_ms: Option<u64>, stats: &mut Stats) -> bool {
    let Some(from) = from_ms else { return false };
    let Some(index) = archive_index::read(path) else {
        return false;
    };
    if index.last_ts_ms >= from {
        return false;
    }
    stats.archives_skipped += 1;
    stats.records_skipped += index.records;
    stats.invalid += index.invalid;
    true
}

fn read_lines(mut reader: Box<dyn BufRead>, from_ms: Option<u64>, scan: &mut Scan) {
    let mut line = Vec::new();
    loop {
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                scan.stats.unreadable += 1;
                break;
            }
        }
        match archive_index::timestamp(&line) {
            Ok(ts) => {
                scan.stats.records_read += 1;
                if from_ms.is_some_and(|from| ts < from) {
                    scan.stats.records_skipped += 1;
                } else if let Ok(record) = serde_json::from_slice::<Value>(&line) {
                    scan.records.push(record);
                } else {
                    scan.stats.invalid += 1;
                }
            }
            Err(_) => scan.stats.invalid += 1,
        }
        line.clear();
    }
}
