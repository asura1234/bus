//! Process logging policy chosen from `--dev` / BUS_LOG, checked end to end.
use crate::utils::logging::test_capture::run_to_file;
use crate::utils::logging::{
    Retention, DEFAULT_MAX_LOG_BYTES, DEFAULT_RETAINED_LOG_FILES, DEV_FILTER,
};
use std::fs;
use std::time::Duration;

const HOUR: Duration = Duration::from_secs(60 * 60);

/// Live report: BUS_LOG (and `--dev`) set `bus=trace`, yet the logs had no TRACE
/// lines. The process policy does reach the file writer at TRACE; only the
/// payload modules the dev filter silences stay out.
#[test]
fn bus_log_trace_policy_reaches_the_log_file() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved = ["BUS_DEV", "BUS_LOG"].map(|key| (key, std::env::var_os(key)));
    for dev in [true, false] {
        if dev {
            std::env::set_var("BUS_DEV", "1");
        } else {
            std::env::remove_var("BUS_DEV");
        }
        std::env::set_var("BUS_LOG", DEV_FILTER);
        let options = super::logging_options();
        let dir = crate::utils::test_temp::unique_temp_path("bus-trace-policy");
        let path = dir.join("bus.log");
        run_to_file(dir.clone(), "bus.log", &options, || {
            tracing::trace!(target: "bus::server::main_loop", "SERVER_TRACE");
            tracing::trace!(target: "bus::client::run", "CLIENT_TRACE");
            tracing::trace!(target: "bus::protocol::keys::host", "SECRET_KEYS");
            tracing::debug!(target: "bus::client::host_terminal::input", "SECRET_INPUT");
        });
        let logs = fs::read_to_string(&path).unwrap();
        assert!(
            logs.contains("TRACE bus::server::main_loop: SERVER_TRACE")
                && logs.contains("TRACE bus::client::run: CLIENT_TRACE"),
            "dev={dev}: {logs}"
        );
        assert!(!logs.contains("SECRET"), "dev={dev}: {logs}");
        let _ = fs::remove_dir_all(dir);
    }
    for (key, value) in saved {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

/// Live report: dev logs held about an hour because 3 numbered 5 MB files
/// filled in ~20 minutes at the measured ~1 MB/min client rate.
#[test]
fn dev_logging_options_keep_a_day_of_logs_under_a_hard_cap() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved = std::env::var_os("BUS_DEV");
    std::env::set_var("BUS_DEV", "1");
    let dev = super::logging_options();
    std::env::remove_var("BUS_DEV");
    let release = super::logging_options();
    if let Some(saved) = saved {
        std::env::set_var("BUS_DEV", saved);
    }

    let Retention::Archive(policy) = dev.retention else {
        panic!("dev builds must archive by age: {:?}", dev.retention);
    };
    assert!(policy.max_age >= 24 * HOUR, "{policy:?}");
    assert!(policy.max_total_bytes <= 1024 * 1024 * 1024, "{policy:?}");
    assert_eq!(
        (release.max_bytes, release.retention),
        (
            DEFAULT_MAX_LOG_BYTES,
            Retention::Generations(DEFAULT_RETAINED_LOG_FILES)
        ),
        "release retention is unchanged"
    );
}
