//! Slow OS enumeration has a separate worker; the recorder formats the samples.
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub(crate) struct ProcessSample {
    pub(crate) pid: u32,
    pub(crate) start_us: u64,
    pub(crate) basename: String,
    pub(crate) cpu_ns: u64,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct MachineSample {
    pub(crate) processes: Vec<ProcessSample>,
    pub(crate) enumerated: usize,
    pub(crate) unavailable: usize,
    pub(crate) load1: Option<f64>,
    pub(crate) memory_pressure: Option<u32>,
    pub(crate) page_ins: Option<u64>,
    pub(crate) page_outs: Option<u64>,
}
pub(crate) type MachineHook = fn() -> Option<MachineSample>;
pub(super) struct Sampler {
    stop: mpsc::Sender<()>,
    samples: mpsc::Receiver<(Instant, MachineSample)>,
    previous: BTreeMap<(u32, u64), (Instant, u64)>,
}
impl Sampler {
    pub(super) fn spawn(hook: MachineHook) -> Self {
        let (stop, stopped) = mpsc::channel();
        let (tx, samples) = mpsc::sync_channel(1);
        let _ = std::thread::Builder::new()
            .name("bus-perf-machine".into())
            .spawn(move || loop {
                if let Some(sample) = hook() {
                    if matches!(
                        tx.try_send((Instant::now(), sample)),
                        Err(mpsc::TrySendError::Disconnected(_))
                    ) {
                        break;
                    }
                }
                if stopped.recv_timeout(Duration::from_secs(5))
                    != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            });
        Self {
            stop,
            samples,
            previous: BTreeMap::new(),
        }
    }
    pub(super) fn take(&mut self) -> Option<serde_json::Value> {
        let (now, sample) = self.samples.try_recv().ok()?;
        Some(self.record(now, sample))
    }
    fn record(&mut self, now: Instant, sample: MachineSample) -> serde_json::Value {
        let readable = sample.processes.len();
        let mut next = BTreeMap::new();
        let mut processes: Vec<_> = sample
            .processes
            .into_iter()
            .filter_map(|process| {
                let key = (process.pid, process.start_us);
                let previous = self.previous.get(&key).copied();
                next.insert(key, (now, process.cpu_ns));
                let (at, cpu) = previous?;
                let elapsed = now.saturating_duration_since(at).as_nanos();
                if elapsed == 0 || cpu > process.cpu_ns {
                    return None;
                }
                let pct = (process.cpu_ns - cpu) as f64 / elapsed as f64 * 100.0;
                Some((pct, process))
            })
            .collect();
        self.previous = next;
        processes.sort_by(|a, b| b.0.total_cmp(&a.0));
        let top: Vec<_> = processes.into_iter().take(8).map(|(pct,p)| serde_json::json!({
            "pid":p.pid, "start_us":p.start_us, "basename":p.basename.chars().take(64).collect::<String>(),
            "cpu_pct":(pct * 10.0).round() / 10.0,
        })).collect();
        serde_json::json!({"top_cpu":top,"load1":sample.load1,"memory_pressure":sample.memory_pressure,
            "page_ins":sample.page_ins,"page_outs":sample.page_outs,"interval_s":5,
            "process_coverage":{"scope":"readable_processes","enumerated":sample.enumerated,
                "readable":readable,"unavailable":sample.unavailable}})
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn top_cpu_is_capped_and_pid_reuse_does_not_inherit_cpu() {
        let mut sampler = Sampler::spawn(|| None);
        let now = Instant::now();
        let sample = |cpu_ns, start_us| MachineSample {
            enumerated: 15,
            unavailable: 3,
            processes: (1..=12)
                .map(|pid| ProcessSample {
                    pid,
                    start_us,
                    basename: "rustc".into(),
                    cpu_ns: cpu_ns * u64::from(pid),
                })
                .collect(),
            ..MachineSample::default()
        };
        assert_eq!(
            sampler.record(now, sample(0, 1))["top_cpu"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let record = sampler.record(now + Duration::from_secs(5), sample(1_000_000_000, 1));
        assert_eq!(record["top_cpu"].as_array().unwrap().len(), 8);
        assert_eq!(record["top_cpu"][0]["pid"], 12);
        assert_eq!(record["top_cpu"][0]["cpu_pct"], 240.0);
        assert_eq!(
            record["process_coverage"],
            serde_json::json!({
                "scope":"readable_processes", "enumerated":15, "readable":12, "unavailable":3,
            })
        );
        assert!(
            sampler.record(now + Duration::from_secs(10), sample(2_000_000_000, 2))["top_cpu"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
