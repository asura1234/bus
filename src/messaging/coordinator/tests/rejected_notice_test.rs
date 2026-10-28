use super::*;

#[test]
fn rejected_reply_notice_does_not_block_master_after_unrelated_turns_and_restart() {
    for restart in [false, true] {
        let mut f = Fixture::new();
        let pasted = Arc::new(Mutex::new(Vec::new()));
        f.worker.transport = Box::new(NeverShown(pasted.clone()));
        let source = f.send(Author::Agent(f.orch), "build this");
        f.finish(source, "Retain this important worker reply");
        f.worker.submit_ready().unwrap();
        let notice = f.notices()[0].id;
        let sent_at = f
            .worker
            .state
            .request(notice)
            .unwrap()
            .prompt
            .submitted_at_ms;
        let master = f.worker.state.master_room().unwrap().id;
        let mut state = f.worker.state.clone();
        let developer = state
            .submit_message_from(
                master,
                Draft {
                    text: "MASTER developer correction".into(),
                    files: vec![],
                    recipient_ids: [f.orch].into(),
                },
                Author::Developer,
                io::now_ms(),
            )
            .unwrap()[0];
        // The captured trace alternates unrelated Working/Idle turns, each
        // shorter than the ordinary queued stall window.
        for n in 1..=4 {
            state
                .observe_status(f.orch, RuntimeStatus::Working, sent_at + n * 30_000 - 1)
                .unwrap();
            state
                .observe_status(f.orch, RuntimeStatus::Idle, sent_at + n * 30_000)
                .unwrap();
        }
        f.worker.save(state).unwrap();
        if restart {
            f.reopen();
        }
        f.worker.expire_queued_requests(sent_at + 180_000).unwrap();
        assert_eq!(
            f.worker.state.queued_requests(f.orch).first(),
            Some(&developer),
            "a retained rejected notice must yield to unattempted messages from another room"
        );
        assert_eq!(
            f.worker.state.request(notice).unwrap().phase,
            RequestPhase::Queued
        );
        f.worker.transport = Box::new(Accepting(f.calls.clone()));
        f.worker.withdrawn_at.clear();
        f.worker.submit_ready().unwrap();
        assert_eq!(
            f.worker.state.request(developer).unwrap().phase,
            RequestPhase::Active
        );
        assert!(f
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|text| text.contains("MASTER developer correction")));
        assert!(f
            .worker
            .state
            .request(source)
            .unwrap()
            .reply()
            .unwrap()
            .text
            .contains("Retain this important worker reply"));
    }
}
