use super::*;
use std::os::unix::fs::PermissionsExt;

/// The exited pane, whose accepted close is followed by the data dir going
/// read-only, so the save right after the close fails.
struct ReadOnlyAfterClose {
    pane: ExitedPane,
    dir: PathBuf,
    armed: bool,
}

impl Transport for ReadOnlyAfterClose {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let close = matches!(method, Method::PaneCloseIfIdentity(_));
        let result = self.pane.request(method);
        if close && result.is_ok() && std::mem::take(&mut self.armed) {
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        result
    }
}

/// The close pass closes the pane, then its own save fails; then storage
/// recovers and the next poll runs.
fn close_then_fail_its_save(f: &mut Fixture) -> ExitedPane {
    exited_worker(f);
    let pane = ExitedPane::install(f, 0);
    f.worker.transport = Box::new(ReadOnlyAfterClose {
        pane: ExitedPane {
            calls: pane.calls.clone(),
            closes: pane.closes.clone(),
            refuse_closes: 0,
            open: true,
        },
        dir: f.dir.clone(),
        armed: true,
    });
    let failed = f.worker.poll();
    std::fs::set_permissions(&f.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err(), "the close pass's save must fail");
    assert_eq!(pane.closes.lock().unwrap().len(), 1, "Bus closed the pane");
    let (events, _received) = mpsc::channel();
    f.worker
        .retry_storage(std::time::Instant::now() + Duration::from_secs(2), &events);
    assert!(f.worker.storage_pause.is_none());
    f.worker.poll().unwrap();
    pane
}

#[test]
fn a_terminal_closed_before_a_failed_save_is_not_reported_still_open() {
    let mut f = Fixture::new();
    close_then_fail_its_save(&mut f);
    let snapshot = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .exit_snapshot
        .clone()
        .unwrap();
    let master = f.worker.state.master_room().unwrap().id;
    let notice = f
        .worker
        .state
        .notices(master)
        .map(|p| p.text.clone())
        .find(|t| t.contains("Agent process exited"))
        .expect("exit notice");
    assert!(
        !notice.contains("its terminal is still open"),
        "closed terminal reported open: terminal_closed={:?}\n{notice}",
        snapshot.terminal_closed
    );
}

#[test]
fn an_agent_whose_terminal_closed_before_a_failed_save_can_still_be_deleted() {
    let mut f = Fixture::new();
    close_then_fail_its_save(&mut f);
    let (events, _received) = mpsc::channel();
    f.worker
        .command(BusCommand::DeleteAgent(f.agent), &events)
        .unwrap();
    assert!(
        f.worker.state.agent(f.agent).is_none(),
        "deletion kept the agent: {:?}",
        f.worker.snapshot().error
    );
}
