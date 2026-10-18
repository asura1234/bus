//! `bus perf`: summarizes the session's perf JSONL files after a freeze.
//!
//! It reads files only, so it works while Bus is hung or after it exited.
use serde_json::Value;
use std::io::Write as _;
use std::path::Path;

mod reader;
use reader::read_records;

/// A timer or a perf-thread wake-up this late marks a window as slow.
const SLOW_MS: f64 = 100.0;
const ROLES: [&str; 2] = ["client", "server"];
const DEFAULT_SINCE_MS: u64 = 30 * 60_000;

pub(super) fn run(data_dir: &Path, args: &[String]) -> std::io::Result<()> {
    let since = parse_since(args).map_err(std::io::Error::other)?;
    let from_ms = since.map(|since| crate::utils::time::now_ms().saturating_sub(since));
    let span = since.map_or("all records".to_owned(), |ms| {
        format!("last {} min", ms / 60_000)
    });
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "perf: {} ({span})", data_dir.join("perf").display())?;
    stdout.flush()?;
    let scan = read_records(data_dir, from_ms);
    writeln!(
        stdout,
        "{} records\n{}",
        scan.records.len(),
        summarize(&scan.records, local_clock)
    )?;
    let stats = scan.stats;
    writeln!(std::io::stderr().lock(),
        "perf scan: read {} files, skipped {} archives; read {} records, skipped {} records, invalid {} records, unreadable {} files",
        stats.files_read, stats.archives_skipped, stats.records_read,
        stats.records_skipped, stats.invalid, stats.unreadable,
    )?;
    Ok(())
}

fn local_clock(ts_ms: u64) -> String {
    i64::try_from(ts_ms / 1_000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map_or_else(
            || format!("{ts_ms}ms"),
            |time| {
                format!(
                    "{:02}-{:02} {:02}:{:02}:{:02}",
                    u8::from(time.month()),
                    time.day(),
                    time.hour(),
                    time.minute(),
                    time.second()
                )
            },
        )
}

/// `--since 30m|2h`, `--all`, or nothing (the last 30 minutes), as milliseconds.
fn parse_since(args: &[String]) -> Result<Option<u64>, String> {
    const USAGE: &str = "usage: bus perf [--since <minutes>m|<hours>h | --all]";
    match args {
        [] => Ok(Some(DEFAULT_SINCE_MS)),
        [flag] if flag == "--all" => Ok(None),
        [flag, value] if flag == "--since" => {
            let (number, unit_ms) = if let Some(minutes) = value.strip_suffix('m') {
                (minutes, 60_000)
            } else if let Some(hours) = value.strip_suffix('h') {
                (hours, 3_600_000)
            } else {
                return Err(USAGE.into());
            };
            let number: u64 = number.parse().map_err(|_| USAGE.to_owned())?;
            Ok(Some(number.saturating_mul(unit_ms)))
        }
        _ => Err(USAGE.into()),
    }
}

fn ts(record: &Value) -> u64 {
    record["ts_ms"].as_u64().unwrap_or(0)
}

fn summarize(records: &[Value], clock: fn(u64) -> String) -> String {
    let mut out = String::new();
    let stalls: Vec<String> = records
        .iter()
        .filter(|record| record["kind"] == "stall")
        .map(|record| stall_line(record, clock))
        .collect();
    section(&mut out, "stalls", "", &stalls);
    let slow: Vec<String> = records
        .iter()
        .filter(|record| record["kind"] == "window" && is_slow(record))
        .map(|record| window_line(record, clock))
        .collect();
    section(
        &mut out,
        "slow windows",
        " (a timer or the perf thread >= 100 ms)",
        &slow,
    );
    // Several clients can share a role file; peaks are per process.
    let mut processes: Vec<(&str, u64)> = records
        .iter()
        .filter(|record| record["kind"] == "window")
        .map(|record| (text(&record["role"]), record["pid"].as_u64().unwrap_or(0)))
        .collect();
    processes.sort_by_key(|(role, pid)| (ROLES.iter().position(|r| r == role), *pid));
    processes.dedup();
    for (role, pid) in processes {
        let windows: Vec<&Value> = records
            .iter()
            .filter(|record| {
                record["kind"] == "window"
                    && record["role"] == role
                    && record["pid"].as_u64().unwrap_or(0) == pid
            })
            .collect();
        out.push_str(&peaks_line(&format!("{role} pid {pid}"), &windows, clock));
    }
    out
}

fn section(out: &mut String, title: &str, note: &str, lines: &[String]) {
    if lines.is_empty() {
        out.push_str(&format!("{title}: none\n"));
        return;
    }
    out.push_str(&format!("{title}{note}:\n"));
    for line in lines {
        out.push_str(&format!("  {line}\n"));
    }
}

fn stall_line(record: &Value, clock: fn(u64) -> String) -> String {
    let head = format!(
        "{} {} {}",
        clock(ts(record)),
        text(&record["role"]),
        text(&record["loop"])
    );
    let busy = record["busy_ms"].as_u64().unwrap_or(0);
    let stage = text(&record["stage"]);
    match record["state"].as_str() {
        Some("began") => format!("{head} blocked in {stage} ({busy} ms so far)"),
        Some("moved") => format!("{head} still blocked, now in {stage} ({busy} ms so far)"),
        Some("ended") => format!("{head} resumed after {busy} ms ({stage})"),
        Some("capture") if record["ok"] == true => format!(
            "{head} stacks captured after {busy} ms in {stage}: perf/{}",
            text(&record["file"])
        ),
        Some("capture") => format!("{head} stack capture after {busy} ms in {stage} failed"),
        _ => format!("{head} {record}"),
    }
}

fn max_ms(timer: &Value) -> f64 {
    timer["max_ms"].as_f64().unwrap_or(0.0)
}

fn is_slow(record: &Value) -> bool {
    let slow_timer = record["timers"]
        .as_object()
        .is_some_and(|timers| timers.values().any(|timer| max_ms(timer) >= SLOW_MS));
    slow_timer || record["watchdog_lag_max_ms"].as_f64().unwrap_or(0.0) >= SLOW_MS
}

fn window_line(record: &Value, clock: fn(u64) -> String) -> String {
    let mut line = format!(
        "{} {} {}",
        clock(ts(record)),
        text(&record["role"]),
        system_context(record)
    );
    let mut culprits: Vec<(&String, &Value)> = record["timers"]
        .as_object()
        .map(|timers| {
            timers
                .iter()
                .filter(|(_, timer)| max_ms(timer) >= SLOW_MS)
                .collect()
        })
        .unwrap_or_default();
    culprits.sort_by(|a, b| max_ms(b.1).total_cmp(&max_ms(a.1)));
    let culprits: Vec<String> = culprits
        .iter()
        .take(5)
        .map(|(name, timer)| format!("{name} max {:.0}ms (n{})", max_ms(timer), timer["n"]))
        .collect();
    if !culprits.is_empty() {
        line.push_str(&format!(" | {}", culprits.join(", ")));
    }
    let gauges: Vec<String> = record["gauges"]
        .as_object()
        .map(|gauges| {
            gauges
                .iter()
                .filter(|(_, gauge)| gauge["max"].as_u64().unwrap_or(0) > 0)
                .map(|(name, gauge)| format!("{name} {}", gauge["max"]))
                .collect()
        })
        .unwrap_or_default();
    if !gauges.is_empty() {
        line.push_str(&format!(" | {}", gauges.join(", ")));
    }
    line
}

fn system_context(record: &Value) -> String {
    let number = |key: &str| record[key].as_f64();
    let mut parts = Vec::new();
    if let Some(cpu) = number("cpu_pct") {
        parts.push(format!("cpu {cpu:.1}%"));
    }
    if let Some(sys) = number("sys_cpu_pct") {
        parts.push(format!("sys {sys:.1}%"));
    }
    if let Some(load) = number("load1") {
        parts.push(format!("load {load:.1}/{}", record["cpus"]));
    }
    if let Some(rss) = number("rss_mb") {
        parts.push(format!("rss {rss:.0}MB"));
    }
    if let Some(swap) = number("swap_used_mb") {
        parts.push(format!("swap {swap:.0}MB"));
    }
    if let Some(lag) = number("watchdog_lag_max_ms") {
        parts.push(format!("lag {lag:.0}ms"));
    }
    parts.join(" ")
}

fn peaks_line(role: &str, windows: &[&Value], clock: fn(u64) -> String) -> String {
    let peak = |key: &str| {
        windows
            .iter()
            .filter_map(|record| Some((record[key].as_f64()?, ts(record))))
            .max_by(|a, b| a.0.total_cmp(&b.0))
    };
    let parts: Vec<String> = [
        ("cpu_pct", "cpu", "%", 1),
        ("rss_mb", "rss", "MB", 0),
        ("fds", "fds", "", 0),
        ("threads", "threads", "", 0),
        ("load1", "load", "", 1),
        ("watchdog_lag_max_ms", "lag", "ms", 0),
    ]
    .into_iter()
    .filter_map(|(key, label, unit, decimals)| {
        let (value, at) = peak(key)?;
        Some(format!("{label} {value:.decimals$}{unit} at {}", clock(at)))
    })
    .collect();
    format!("{role} peaks: {}\n", parts.join(", "))
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("?")
}

#[cfg(test)]
#[path = "tests/perf_test.rs"]
mod tests;
