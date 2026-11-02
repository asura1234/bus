//! Metadata-only failure notifications. The file logger remains the evidence source.
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tracing::{
    field::{Field, Visit},
    Event, Level, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

const ENABLED: &str = "error-pipe.enabled";
const INBOX: &str = "error-pipe.pending";
pub(crate) const RPC_PREFIX: &str = "bus-error-report:";
pub(crate) const NOTICE_PREFIX: &str = "Bus error: ";
thread_local! { static SUPPRESSED: Cell<bool> = const { Cell::new(false) }; }

pub(crate) struct Suppression(bool);
impl Drop for Suppression {
    fn drop(&mut self) {
        SUPPRESSED.set(self.0);
    }
}
pub(crate) fn suppress(enabled: bool) -> Suppression {
    let old = SUPPRESSED.replace(is_suppressed() || enabled);
    Suppression(old)
}
pub(crate) fn is_suppressed() -> bool {
    SUPPRESSED.get()
}
pub(crate) fn report_rpc(id: &str) -> bool {
    id.starts_with(RPC_PREFIX)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Record {
    pub(crate) event: String,
    pub(crate) location: String,
    pub(crate) log: PathBuf,
    pub(crate) generation: String,
}

/// No log payloads, context fields or excerpts cross into the inbox.
#[derive(Default)]
struct Fields {
    event: Option<String>,
}
impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "event" {
            self.event = Some(value.into());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "event" {
            self.event = Some(format!("{value:?}"));
        }
    }
}

pub(crate) struct CaptureLayer {
    log: PathBuf,
}
impl CaptureLayer {
    pub(crate) fn new(log: PathBuf) -> Self {
        Self { log }
    }
}
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if is_suppressed() {
            return;
        }
        let meta = event.metadata();
        if *meta.level() > Level::WARN {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let name = fields.event.as_deref().unwrap_or(meta.name());
        if *meta.level() == Level::WARN && !broken_warning(name, meta.target()) {
            return;
        }
        let Some(dir) = self.log.parent() else {
            return;
        };
        let Ok(generation) = std::fs::read_to_string(dir.join(ENABLED)) else {
            return;
        };
        let record = Record {
            event: name.into(),
            location: format!(
                "{}:{}:{}",
                meta.target(),
                meta.file().unwrap_or("unknown"),
                meta.line().unwrap_or(0)
            ),
            log: self.log.clone(),
            generation,
        };
        // Inbox errors cannot recursively enter the tracing subscriber.
        let _guard = suppress(true);
        let _ = append(dir, &record);
    }
}

fn broken_warning(event: &str, target: &str) -> bool {
    if target == "bus::protocol::keys::host::framer" {
        return false;
    }
    !matches!(
        event,
        "perf.high_load"
            | "perf.stall"
            | "perf.stall.stage"
            | "perf.stall.end"
            | "perf.slow_op"
            | "perf.write_recovered"
            | "api.wait.timeout"
            | "bus.form.validation_failed"
            | "bus.room.rename_refused"
            | "bus.room.delete_refused"
            | "bus.attach.image_refused"
            | "bus.control.admission.rejected"
            | "bus.callback.rejected"
            | "bus.dev.existing_server"
            | "bus.quit.forced"
            | "bus.agent.process_exited"
            | "bus.agent.exit_terminal_kept"
            | "bus.deletion.terminal_left_open"
            | "bus.terminal.submission.recovery"
    )
}

/// The inode is never renamed; its OS lock serializes append and drain across processes.
fn inbox(dir: &Path) -> io::Result<std::fs::File> {
    crate::platform::fs::private_dir(dir)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(dir.join(INBOX))?;
    file.lock()?;
    Ok(file)
}
fn append(dir: &Path, record: &Record) -> io::Result<()> {
    let mut file = inbox(dir)?;
    file.seek(SeekFrom::End(0))?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")
}
pub(crate) fn drain(dir: &Path) -> io::Result<Vec<Record>> {
    let _guard = suppress(true);
    let mut file = inbox(dir)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    file.set_len(0)?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

#[derive(Default)]
pub(crate) struct Runtime {
    configured: bool,
    selected: Option<String>,
    generation: Option<String>,
    batches: Batches,
}
impl Runtime {
    pub(crate) fn configure(&mut self, dir: &Path, selected: Option<&str>) {
        if self.configured && self.selected.as_deref() == selected {
            return;
        }
        let _guard = suppress(true);
        self.configured = true;
        self.selected = selected.map(str::to_owned);
        self.generation = None;
        self.batches = Batches::default();
        let _ = std::fs::remove_file(dir.join(ENABLED));
        let _ = drain(dir);
        if selected.is_some() {
            let generation = crate::utils::time::now_ns().to_string();
            if crate::platform::fs::atomic_write(&dir.join(ENABLED), generation.as_bytes()).is_ok()
            {
                self.generation = Some(generation);
            }
        }
    }
    pub(crate) fn collect(&mut self, dir: &Path, now: u64) -> Vec<String> {
        for record in drain(dir).unwrap_or_default() {
            if Some(&record.generation) == self.generation.as_ref() {
                self.batches.add(record, now);
            }
        }
        self.batches.notices(now)
    }
}

#[derive(Default)]
pub(crate) struct Batches(BTreeMap<(String, String, PathBuf), Batch>);
struct Batch {
    record: Record,
    count: u64,
    first: u64,
    last_notice: Option<u64>,
}
impl Batches {
    pub(crate) fn add(&mut self, record: Record, now: u64) {
        let key = (
            record.event.clone(),
            record.location.clone(),
            record.log.clone(),
        );
        let batch = self.0.entry(key).or_insert(Batch {
            record,
            count: 0,
            first: now,
            last_notice: None,
        });
        batch.count = batch.count.saturating_add(1);
    }
    pub(crate) fn notices(&mut self, now: u64) -> Vec<String> {
        self.0
            .values_mut()
            .filter_map(|batch| {
                if batch.count == 0
                    || now.saturating_sub(batch.first) < 1000
                    || batch
                        .last_notice
                        .is_some_and(|last| now.saturating_sub(last) < 60_000)
                {
                    return None;
                }
                let text = format!(
                    "{NOTICE_PREFIX}{} (x{} since last notice). Logs: {}",
                    batch.record.event,
                    batch.count,
                    batch.record.log.display()
                );
                batch.count = 0;
                batch.last_notice = Some(now);
                Some(text)
            })
            .collect()
    }
}
#[cfg(test)]
mod tests;
