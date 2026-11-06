//! One restart outcome notice per tick, then the helper's file is removed.
use super::Worker;

impl Worker {
    /// A failed save leaves the file so the next tick can try again.
    pub(in crate::messaging::coordinator) fn announce_recorded_restart(
        &mut self,
    ) -> Result<(), String> {
        let path = crate::utils::restart_record::outcome_path(&self.data_dir);
        let Some(record) = crate::utils::restart_record::read_outcome(&path)? else {
            return Ok(());
        };
        let mut state = self.state.clone();
        // A post error must keep outcome.json. The next tick warns and retries.
        // Nothing to deliver (no live work-room orchestrator) may delete it.
        let posted = state.announce_restart(&record, crate::messaging::storage::io::now_ms())?;
        if posted {
            self.save(state)?;
        }
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::native::{Transport, TransportError};
    use crate::protocol::api::schema::{Method, ResponseResult};

    struct NoTransport;
    impl Transport for NoTransport {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            panic!("restart receipt must not contact a provider");
        }
    }

    #[test]
    fn restart_failure_receipt_logs_once_without_creating_chat() {
        for outcome in ["rolled_back", "refused", "rollback_failed"] {
            let dir = crate::utils::test_temp::unique_temp_path(&format!(
                "bus-restart-receipt-{outcome}"
            ));
            let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
            let mut record = crate::utils::restart_record::RestartLog {
                at_ms: 10,
                who: "user:receipt-test".into(),
                old_sha: "old".into(),
                new_sha: "new".into(),
                old_binary: "/old".into(),
                new_binary: "/new".into(),
                outcome: String::new(),
                reason: String::new(),
            };
            record.outcome = outcome.into();
            record.reason = "RESTART_RECEIPT_CAUSE".into();
            let path = crate::utils::restart_record::outcome_path(&worker.data_dir);
            crate::utils::restart_record::write_json(&path, &record).unwrap();
            let capture = crate::utils::logging::test_capture::Capture::default();
            capture.run(|| {
                worker.announce_recorded_restart().unwrap();
                assert!(!path.exists());
                worker.announce_recorded_restart().unwrap();
            });
            assert!(worker
                .state
                .requests()
                .all(|request| !request.prompt.text.contains("RESTART_RECEIPT_CAUSE")));
            let logs = capture.text();
            assert_eq!(logs.matches("RESTART_RECEIPT_CAUSE").count(), 1, "{logs}");
            assert!(
                logs.contains("bus.restart.diagnostic") && logs.contains(outcome),
                "{logs}"
            );
            drop(worker);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
