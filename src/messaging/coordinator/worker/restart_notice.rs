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
        let reconcile = matches!(record.outcome.as_str(), "restarted" | "rolled_back");
        if reconcile && self.state.restart_receipt_seen(record.at_ms) {
            // A crash after the saved acknowledgement can leave outcome.json.
            // Its old receipt must not touch any successor or newer rebind.
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            return Ok(());
        }
        let mut state = self.state.clone();
        if reconcile {
            let interrupted: Vec<_> = self
                .state
                .agents()
                .filter(|agent| !agent.restore_pending_receipt)
                .map(|agent| agent.id)
                .collect();
            for agent in interrupted {
                state.mark_restored_turn(agent);
            }
            // Persist receipt acknowledgement with the interruption markers.
            // A coordinator reopen cannot lose the successor exclusion.
            state.acknowledge_native_rebinds(record.at_ms);
        }
        // A post error must keep outcome.json. The next tick warns and retries.
        // Nothing to deliver (no live work-room orchestrator) may delete it.
        let posted = state.announce_restart(&record, crate::messaging::storage::io::now_ms())?;
        if posted || reconcile {
            // Persist interruption before consuming its only durable receipt.
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

    #[test]
    fn restart_reconciliation_retained_receipt_only_ack_survives_failed_save_and_reopen() {
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = crate::utils::test_temp::unique_temp_path("bus-receipt-only-ack");
        let _cleanup = Cleanup(dir.clone());
        let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
        let record = crate::utils::restart_record::RestartLog {
            at_ms: 10,
            who: "user:receipt-test".into(),
            old_sha: "old".into(),
            new_sha: "new".into(),
            old_binary: "/old".into(),
            new_binary: "/new".into(),
            outcome: "restarted".into(),
            reason: String::new(),
        };
        let path = crate::utils::restart_record::outcome_path(&dir);
        crate::utils::restart_record::write_json(&path, &record).unwrap();
        worker
            .store
            .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
        assert!(
            worker.announce_recorded_restart().is_err(),
            "receipt identity must require persistence even with no request or agent changes"
        );
        assert!(
            path.exists(),
            "failed acknowledgment must retain the receipt"
        );
        drop(worker);
        let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
        worker.announce_recorded_restart().unwrap();
        assert!(!path.exists());
        drop(worker);
        let mut worker = Worker::open(dir, Box::new(NoTransport)).unwrap();
        crate::utils::restart_record::write_json(&path, &record).unwrap();
        worker
            .store
            .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
        worker
            .announce_recorded_restart()
            .expect("durably acknowledged replay only removes the leftover file");
        assert!(!path.exists());
    }
}
