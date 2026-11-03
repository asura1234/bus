use super::*;
use crate::protocol::api::schema::{Method, ResponseResult};
use crate::{
    agents::providers::spool as callbacks,
    messaging::{
        native::{Transport, TransportError},
        storage::{io, state_store::JsonStore},
    },
};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[path = "support_test.rs"]
pub(super) mod support;
use support::*;

#[path = "poll_recovery_test.rs"]
mod poll_recovery;

#[path = "working_directory_test.rs"]
mod working_directory;

#[path = "codex_project_trust_test.rs"]
mod codex_project_trust;

#[test]
fn exit_evidence_updates_lifecycle_without_developer_chat() {
    for (terminal, exited, expected) in [
        ("terminal", true, 0),
        ("foreign", true, 0),
        ("terminal", false, 0),
    ] {
        let pane = serde_json::from_value(json!({"pane_id":"pane", "terminal_id":terminal, "workspace_id":"w1", "tab_id":"t1", "focused":false, "agent_status":"unknown", "revision":1, "agent_process_exited":exited})).unwrap();
        let (mut worker, _, _, dir, _) = fixture(
            Provider::ClaudeCode,
            vec![
                Ok(ResponseResult::AgentList { agents: vec![] }),
                Ok(ResponseResult::PaneInfo { pane }),
            ],
        );
        worker.poll().unwrap();
        let master = worker.state.master_room().unwrap().id;
        assert_eq!(worker.state.notices(master).count(), expected);
        assert!(worker
            .state
            .notices(master)
            .all(|notice| notice.author == Author::Bus
                && notice.text.starts_with(crate::messaging::attention::TAG)));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
    for listing in [
        Ok(ResponseResult::AgentList { agents: vec![] }),
        Err(TransportError {
            timestamps: Default::default(),
            message: "offline".into(),
            code: None,
            definitely_rejected: false,
        }),
    ] {
        let (mut worker, _, _, dir, _) = fixture(Provider::ClaudeCode, vec![listing]);
        let _ = worker.poll();
        assert_eq!(
            worker
                .state
                .notices(worker.state.master_room().unwrap().id)
                .count(),
            0
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn feishu_pending_relaunch_without_exit_evidence_does_not_notify_developer() {
    // Native exit retirement and replacement exits are covered in terminal's
    // process_exit_test; messaging consumes only their public protocol facts.
    let info = serde_json::from_value(json!({
        "pane_id":"pane", "terminal_id":"terminal", "workspace_id":"w1", "tab_id":"t1",
        "focused":false, "agent":null, "name":"updating-codex",
        "agent_status":"unknown", "revision":1,
        "launch_pending":true, "interactive_ready":false
    }))
    .unwrap();
    let pane = serde_json::from_value(json!({
        "pane_id":"pane", "terminal_id":"terminal", "workspace_id":"w1", "tab_id":"t1",
        "focused":false, "agent_status":"unknown", "revision":1,
        "agent_process_exited":false
    }))
    .unwrap();
    let (mut worker, _, _, dir, calls) = fixture(
        Provider::Codex,
        vec![
            Ok(ResponseResult::AgentList { agents: vec![info] }),
            Ok(ResponseResult::PaneInfo { pane }),
        ],
    );
    worker.poll().unwrap();
    assert!(
        calls.lock().unwrap().contains(&"pane.get"),
        "the missing-agent probe must consume native exit evidence"
    );
    let notices = worker
        .state
        .notices(worker.state.master_room().unwrap().id)
        .count();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();

    assert_eq!(
        notices, 0,
        "an automatic update is already relaunching this agent"
    );
}
#[path = "callback_binding_test.rs"]
mod callback_binding_tests;
#[path = "delivery_test.rs"]
mod delivery_tests;
#[path = "queue_test.rs"]
mod queue_tests;
#[path = "submit_budget_test.rs"]
mod submit_budget_tests;

#[path = "resume_test.rs"]
mod resume_tests;

#[path = "binding_test.rs"]
mod binding_tests;
#[path = "left_open_notice_routing_test.rs"]
mod left_open_notice_routing_tests;
#[path = "persistence_test.rs"]
mod persistence_tests;
#[path = "recovery_test.rs"]
mod recovery_tests;

#[path = "compaction_test.rs"]
mod compaction_tests;
#[path = "disk_alert_test.rs"]
mod disk_alert_tests;

#[path = "scheduling_test.rs"]
mod scheduling_tests;

#[path = "save_traffic_test.rs"]
mod save_traffic_tests;

#[path = "control_fast_path_test.rs"]
mod control_fast_path_tests;

#[path = "callback_batch_test.rs"]
mod callback_batch_tests;

#[path = "history_archive_test.rs"]
mod history_archive_tests;

#[path = "history_append_recovery_test.rs"]
mod history_append_recovery_tests;
#[path = "history_readers_test.rs"]
mod history_readers_tests;

#[path = "startup_test.rs"]
mod startup_tests;

#[test]
fn claude_statusline_refresh_is_identity_bound_and_does_not_change_delivery_state() {
    for mismatch in [
        "none", "agent", "launch", "session", "provider", "missing", "corrupt",
    ] {
        let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        let spool = dir.join("callbacks/launch");
        let before = std::fs::read(dir.join("state.json")).unwrap();
        let mut observation = json!({
            "manifest":{"agent_id":agent,"provider":"claude_code","launch_id":"launch"},
            "session_id":"session", "read_at_ms":io::now_ms(),
            "windows":{"five_hour":{"used_percent":12.5,"resets_at":io::now_ms()/1000+300,"window_minutes":300},
                       "weekly":{"used_percent":31,"resets_at":io::now_ms()/1000+10000,"window_minutes":10080}}
        });
        match mismatch {
            "agent" => observation["manifest"]["agent_id"] = json!(999),
            "launch" => observation["manifest"]["launch_id"] = json!("other"),
            "session" => observation["session_id"] = json!("other"),
            "provider" => observation["manifest"]["provider"] = json!("codex"),
            _ => {}
        }
        if mismatch == "corrupt" {
            std::fs::write(spool.join("usage.json"), b"corrupt").unwrap();
        } else if mismatch != "missing" {
            std::fs::write(
                spool.join("usage.json"),
                serde_json::to_vec(&observation).unwrap(),
            )
            .unwrap();
        }
        // Status lines redraw independently of Stop/SessionStart hooks: there
        // are deliberately no event files in this callback poll.
        worker.consume_callbacks(agent, &spool).unwrap();
        worker.dev_enabled = true;
        let response = worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "usage-state".into(),
                method: "state".into(),
                params: json!({}),
            },
            None,
        );
        assert!(response.ok, "{response:?}");
        let usage = &response.result["usage"]["claude"];
        if mismatch == "none" {
            assert_eq!(usage["status"], "observed");
            assert_eq!(usage["five_hour"]["used_percent"], 12.5);
            assert_eq!(usage["weekly"]["used_percent"], 31.0);
            assert_eq!(usage["observed_by_agent"], json!(agent));
        } else {
            assert_eq!(usage["status"], "unknown", "{mismatch}");
        }
        assert_eq!(std::fs::read(dir.join("state.json")).unwrap(), before);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn compact_session_start_counts_one_compaction_per_hook() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    // Each batch is consumed before the next, as the worker does between
    // compactions; identical payloads still spooled together collapse to one.
    for batch in [
        &["startup", "compact", "compact"][..],
        &["resume"],
        &["compact"],
    ] {
        for source in batch {
            record(
                &dir,
                Provider::ClaudeCode,
                json!({"hook_event_name":"SessionStart","session_id":"session","source":source}),
            );
        }
        // A missing source (older providers) is not a compaction.
        record(
            &dir,
            Provider::ClaudeCode,
            json!({"hook_event_name":"SessionStart","session_id":"session"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    let compactions = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(compactions.count, 2);
    assert!(compactions.last_at_ms.is_some());
    drop(worker);
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.agent(agent).unwrap().compactions, compactions);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_final_refreshes_usage_from_its_rollout_and_state_reports_it() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.dev_enabled = true;
    let state = |worker: &mut Worker| {
        worker
            .control_response_with_events(
                &crate::messaging::control::Request {
                    id: format!("state-{}", io::now_ns()),
                    method: "state".into(),
                    params: json!({}),
                },
                None,
            )
            .result
    };
    assert_eq!(state(&mut worker)["usage"]["codex"]["status"], "unknown");
    assert_eq!(state(&mut worker)["usage"]["claude"]["status"], "unknown");

    let rollout = dir.join("rollout-2026-10-05T00-00-00-x.jsonl");
    std::fs::write(
        &rollout,
        json!({"type":"event_msg","payload":{"type":"token_count","rate_limits":{
            "primary":{"used_percent":12.0,"window_minutes":10080,"resets_at":1791788174},
            "secondary":{"used_percent":55.5,"window_minutes":300,"resets_at":1791700000}}}})
        .to_string(),
    )
    .unwrap();
    queue(&mut worker, room, agent, "work");
    worker.submit_ready().unwrap();
    let transcript = rollout.display().to_string();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"work","transcript_path":transcript}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"done","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();

    let usage = &state(&mut worker)["usage"]["codex"];
    assert_eq!(usage["status"], "observed");
    assert_eq!(usage["five_hour"]["used_percent"], 55.5);
    assert_eq!(usage["weekly"]["used_percent"], 12.0);
    assert_eq!(usage["weekly"]["resets_at"], 1791788174);
    assert_eq!(usage["observed_by_agent"], json!(agent));
    assert!(usage["read_at_ms"].as_u64().is_some());

    // A later unreadable rollout keeps the last snapshot instead of erroring.
    std::fs::remove_file(&rollout).unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn2","last_assistant_message":"again","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        state(&mut worker)["usage"]["codex"]["weekly"]["used_percent"],
        12.0
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

mod status_tests {
    //! Agent availability as the pane listing reports it.

    use super::*;

    /// One `agent.list` answer for the fixture's pane: the provider identified
    /// as `agent` (None once the pane runs something else) with its screen
    /// status, or no entry at all once the pane is no longer an agent.
    fn pane_listing(agent: Option<&str>, status: &str) -> Result<ResponseResult, TransportError> {
        let info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
            "terminal_id": "terminal", "pane_id": "pane", "agent": agent,
            "agent_status": status, "agent_session": agent.map(|agent| json!({
                "source": format!("herdr:{agent}"), "agent": agent, "kind": "id", "value": "session"
            })),
            "workspace_id": "w1", "tab_id": "t1", "focused": false,
            "interactive_ready": true, "revision": 1
        }))
        .unwrap();
        Ok(ResponseResult::AgentList { agents: vec![info] })
    }

    #[test]
    fn visible_dialog_poll_keeps_idle_settlement_and_busy_revision() {
        let mut listing = pane_listing(Some("claude"), "idle");
        if let Ok(ResponseResult::AgentList { agents }) = &mut listing {
            agents[0].dialog_id = Some("checkbox-panel".into());
        }
        let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![listing]);
        let busy_revision = worker.state.agent(agent).unwrap().busy_revision;
        worker.poll().unwrap();
        let observed = worker.state.agent(agent).unwrap();
        assert_eq!(observed.status, RuntimeStatus::Idle);
        assert_eq!(observed.busy_revision, busy_revision);
        assert_eq!(observed.shown_status(), RuntimeStatus::Blocked);
        assert!(observed.dialog);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_self_update_shows_its_error_until_the_chooser_is_answered() {
        let error = "Bus could not install the codex update (npm failed). Answer the update prompt in bus-r1-a2's terminal; queued messages wait until then.";
        let mut failed = pane_listing(Some("codex"), "blocked");
        if let Ok(ResponseResult::AgentList { agents }) = &mut failed {
            agents[0].dialog_id = Some("chooser".into());
            agents[0].update_error = Some(error.into());
        }
        let failed_again = failed.clone();
        let (mut worker, agent, room, dir, _) = fixture(
            Provider::Codex,
            vec![
                failed,
                failed_again,
                // The second blocked poll now verifies its notice against a
                // native snapshot. This status-only fixture cannot supply one;
                // keep that read from consuming the following agent.list reply.
                Err(TransportError {
                    timestamps: Default::default(),
                    code: Some("dialog_observation_unavailable".into()),
                    message: "status-only fixture has no screen snapshot".into(),
                    definitely_rejected: true,
                }),
                pane_listing(Some("codex"), "idle"),
            ],
        );
        let shown = |worker: &Worker| worker.state.agent(agent).unwrap().actionable_error.clone();
        worker.poll().unwrap();
        assert_eq!(shown(&worker).as_deref(), Some(error));
        queue(&mut worker, room, agent, "waits for the chooser");
        worker.poll().unwrap();
        assert_eq!(shown(&worker).as_deref(), Some(error));
        worker.poll().unwrap();
        assert_eq!(shown(&worker), None);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_agent_whose_provider_is_not_running_is_unavailable_until_it_returns() {
        let no_agents = Ok(ResponseResult::AgentList { agents: Vec::new() });
        let (mut worker, agent, room, dir, calls) = fixture(
            Provider::Codex,
            vec![
                pane_listing(Some("codex"), "idle"),
                // Codex's own updater replaced its UI: the screen is not Codex.
                pane_listing(Some("codex"), "unknown"),
                // The provider exited and the shell is back.
                no_agents,
                // Another program holds the pane.
                pane_listing(None, "unknown"),
                // Relaunched.
                pane_listing(Some("codex"), "idle"),
            ],
        );
        let status = |worker: &Worker| worker.state.agent(agent).unwrap().status;
        worker.poll().unwrap();
        assert_eq!(status(&worker), RuntimeStatus::Idle);
        let request = queue(&mut worker, room, agent, "held while unavailable");
        for case in ["updater", "exited", "foreign program"] {
            worker.poll().unwrap();
            assert_eq!(status(&worker), RuntimeStatus::Unavailable, "{case}");
            assert_eq!(
                worker.state.room_status(room),
                RuntimeStatus::Blocked,
                "{case}"
            );
            assert_eq!(
                crate::messaging::diagnostics::wait_reason(worker.state.agent(agent).unwrap()),
                Some("agent_unavailable"),
                "{case}"
            );
            worker.submit_ready().unwrap();
            assert_eq!(
                worker.state.request(request).unwrap().phase,
                RequestPhase::Queued,
                "{case}: the delivery stays queued"
            );
        }
        assert!(!calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("agent.prompt")));
        worker.poll().unwrap();
        assert_eq!(status(&worker), RuntimeStatus::Idle);
        assert_eq!(worker.state.room_status(room), RuntimeStatus::Idle);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn composer_busy() -> Result<ResponseResult, TransportError> {
        Err(TransportError {
            timestamps: Default::default(),
            message: "Waiting for the developer's composer".into(),
            code: Some(crate::protocol::api::AGENT_COMPOSER_BUSY.into()),
            definitely_rejected: true,
        })
    }

    fn cursor_limit_listing(reset: u64) -> Result<ResponseResult, TransportError> {
        let info = serde_json::from_value(json!({
            "terminal_id": "terminal", "pane_id": "pane", "name": "bus-r1-a2",
            "agent": "cursor", "agent_status": "idle",
            "agent_session": {"source":"herdr:cursor","agent":"cursor","kind":"id","value":"session"},
            "provider_error": "You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)",
            "provider_quota_reset_at": reset,
            "workspace_id": "w1", "tab_id": "t1", "focused": false,
            "interactive_ready": true, "revision": 1
        }))
        .unwrap();
        Ok(ResponseResult::AgentList { agents: vec![info] })
    }

    #[test]
    fn cursor_monthly_usage_limit_closes_the_turn_without_typing_over_the_restored_draft() {
        let reset = io::now_ms() / 1000 + 3600;
        let (mut worker, agent, room, dir, calls) = fixture(
            Provider::Cursor,
            vec![
                Ok(ResponseResult::Ok {}),
                cursor_limit_listing(reset),
                composer_busy(),
            ],
        );
        let lead = queue(&mut worker, room, agent, "bootstrap");
        worker.submit_ready().unwrap();
        let prompt = delivered_test_prompt(&worker.state, "bootstrap");
        record(
            &dir,
            Provider::Cursor,
            json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-1","prompt":prompt}),
        );
        consume(&mut worker, agent, &dir);
        assert!(
            worker.state.request(lead).unwrap().trusted_start_bound,
            "{:?}",
            worker.state.request(lead).unwrap()
        );
        worker
            .state
            .observe_status(agent, RuntimeStatus::Working, io::now_ms())
            .unwrap();
        let next = queue(&mut worker, room, agent, "later brief");

        worker.poll().unwrap();
        assert_eq!(worker.usage.state_json()["cursor"]["status"], "observed");
        assert!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .is_some(),
            "{:?}",
            worker.state.agent(agent).unwrap()
        );
        assert_eq!(
            worker.state.request(lead).unwrap().phase,
            RequestPhase::Abandoned
        );
        assert_eq!(
            worker.state.request(lead).unwrap().failure_reason.as_deref(),
            Some("provider_error: You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)")
        );
        assert_eq!(
            worker.state.request(next).unwrap().phase,
            RequestPhase::Queued
        );

        worker.submit_ready().unwrap();
        assert_eq!(
            worker.state.request(next).unwrap().phase,
            RequestPhase::Queued
        );
        assert_eq!(worker.state.request(next).unwrap().failure_reason, None);
        assert_eq!(
            calls.lock().unwrap().iter().filter(|call| call.starts_with("agent.prompt")).count(),
            2,
            "the later delivery may inspect the composer, but its definite refusal must not withdraw or replace the restored draft"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_quota_poll_before_pending_start_and_stop_hooks_retains_provider_error() {
        let reset = io::now_ms() / 1000 + 3600;
        let (mut worker, agent, room, dir, _) = fixture(
            Provider::Cursor,
            vec![Ok(ResponseResult::Ok {}), cursor_limit_listing(reset)],
        );
        let request = queue(&mut worker, room, agent, "bootstrap");
        worker.submit_ready().unwrap();
        let prompt = delivered_test_prompt(&worker.state, "bootstrap");
        record(
            &dir,
            Provider::Cursor,
            json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-1","prompt":prompt}),
        );
        record(
            &dir,
            Provider::Cursor,
            json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-1","status":"error","loop_count":0}),
        );
        assert!(!worker.state.request(request).unwrap().trusted_start_bound);
        worker.poll().unwrap();
        consume(&mut worker, agent, &dir);
        assert_eq!(
            worker.state.request(request).unwrap().failure_reason.as_deref(),
            Some("provider_error: You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)"),
            "the poll and callback order used by an ordinary worker tick must retain the observed quota cause"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_quota_observations_for_another_agent_do_not_erase_a_pending_request_error() {
        let reset = io::now_ms() / 1000 + 3600;
        let mut listing = cursor_limit_listing(reset);
        if let Ok(ResponseResult::AgentList { agents }) = &mut listing {
            let mut second = agents[0].clone();
            second.terminal_id = "terminal-b".into();
            second.pane_id = "pane-b".into();
            second.name = Some("bus-r1-a3".into());
            second.agent_session.as_mut().unwrap().value = "session-b".into();
            agents.push(second);
        }
        let (mut worker, agent, room, dir, _) =
            fixture(Provider::Cursor, vec![Ok(ResponseResult::Ok {}), listing]);
        let mut state = worker.state.clone();
        let second = state
            .create_agent(room, "second-cursor", Provider::Cursor, dir.clone(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                second,
                AgentRuntimeIdentity {
                    launch_id: Some("launch-b".into()),
                    terminal_id: Some("terminal-b".into()),
                    pane_id: Some("pane-b".into()),
                    session_id: Some("session-b".into()),
                },
            )
            .unwrap();
        state.confirm_hook_setup(second).unwrap();
        state
            .observe_status(second, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        worker.save(state).unwrap();
        let request = queue(&mut worker, room, agent, "bootstrap");
        worker.submit_ready().unwrap();
        let prompt = delivered_test_prompt(&worker.state, "bootstrap");
        record(
            &dir,
            Provider::Cursor,
            json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-1","prompt":prompt}),
        );
        record(
            &dir,
            Provider::Cursor,
            json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-1","status":"error","loop_count":0}),
        );
        assert!(!worker.state.request(request).unwrap().trusted_start_bound);
        worker.poll().unwrap();
        assert_eq!(
            worker.usage.state_json()["cursor"]["observed_by_agent"],
            json!(second)
        );
        consume(&mut worker, agent, &dir);
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Abandoned
        );
        assert!(worker.state.request(request).unwrap().trusted_start_bound);
        assert_eq!(
            worker.state.request(request).unwrap().failure_reason.as_deref(),
            Some("provider_error: You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)"),
            "another Cursor pane's quota observation must not discard this request's quota cause"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cursor_monthly_usage_limit_pages_once_per_reset() {
        let reset = io::now_ms() / 1000 + 3600;
        let (mut worker, _, _, dir, _) = fixture(
            Provider::Cursor,
            vec![
                cursor_limit_listing(reset),
                cursor_limit_listing(reset + 3600),
            ],
        );
        worker.dev_enabled = true;
        worker.poll().unwrap();
        let start = io::now_ms();
        assert_eq!(worker.usage.state_json()["cursor"]["status"], "observed");
        assert_eq!(
            worker.usage.state_json()["cursor"]["monthly"]["resets_at"],
            reset
        );
        let (events, _) = mpsc::channel();
        for offset in [0, 60_000, 120_000] {
            worker.pump_feishu(&events, start + offset).unwrap();
        }
        let master = worker.state.master_room().unwrap().id;
        assert_eq!(worker.state.notices(master).count(), 1);

        worker.poll().unwrap();
        worker.pump_feishu(&events, start + 180_000).unwrap();
        let notices: Vec<_> = worker
            .state
            .notices(master)
            .map(|notice| notice.text.clone())
            .collect();
        assert_eq!(notices.len(), 2, "{notices:?}");
        assert!(notices[0].contains(&format!("quota:cursor:monthly:{reset}")));
        assert!(notices[1].contains(&format!("quota:cursor:monthly:{}", reset + 3600)));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

mod update_lifecycle_tests {
    use super::*;

    /// What `agent.list` reports for a Codex Bus started while it installs the
    /// update Bus chose in its startup chooser (pinned server-side by
    /// `a_bus_started_codex_installing_its_update_is_listed_working_and_ready`).
    fn codex_installing_its_update() -> Result<ResponseResult, TransportError> {
        let info = serde_json::from_value(json!({
            "terminal_id": "terminal", "pane_id": "pane", "name": "bus-r1-a2",
            "agent": "codex", "agent_status": "working",
            "launch_pending": false, "interactive_ready": true,
            "workspace_id": "w1", "tab_id": "t1", "focused": false, "revision": 1
        }))
        .unwrap();
        Ok(ResponseResult::AgentList { agents: vec![info] })
    }

    #[test]
    fn slow_startup_update_preserves_queued_messages_until_it_finishes() {
        let (mut worker, agent, room, dir, _) =
            fixture(Provider::Codex, vec![codex_installing_its_update()]);
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        worker.save(worker.state.clone()).unwrap();
        worker.poll().unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Working
        );
        let request = queue(&mut worker, room, agent, "brief waits for the update");
        // A three-minute install, past the queued stall grace.
        let expired = worker
            .state
            .expire_stalled_queued_requests(io::now_ms() + 180_000)
            .unwrap();
        let phase = worker.state.request(request).unwrap().phase;
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            expired.is_empty(),
            "slow install expired queued requests: {expired:?}"
        );
        assert_eq!(phase, RequestPhase::Queued);
    }

    #[test]
    fn recovered_update_error_clears_after_coordinator_restart() {
        let error = "Bus could not install the codex update (npm failed). Answer the update prompt in bus-r1-a2's terminal; queued messages wait until then.";
        let listing = |failed: bool| {
            let info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
                "terminal_id": "terminal", "pane_id": "pane", "agent": "codex",
                "agent_status": if failed { "blocked" } else { "idle" },
                "agent_session": {"source":"herdr:codex","agent":"codex","kind":"id","value":"session"},
                "workspace_id":"w1","tab_id":"t1","focused":false,"interactive_ready":true,"revision":1,
                "dialog_id": if failed {Some("chooser")} else {None},
                "update_error": if failed {Some(error)} else {None},
            })).unwrap();
            Ok(ResponseResult::AgentList { agents: vec![info] })
        };
        let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![listing(true)]);
        worker.poll().unwrap();
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .as_deref(),
            Some(error)
        );
        worker.save(worker.state.clone()).unwrap();
        drop(worker);
        let fake = FakeTransport {
            replies: vec![listing(false)].into(),
            calls,
            state_path: dir.join("state.json"),
        };
        let mut reopened = Worker::open(dir.clone(), Box::new(fake)).unwrap();
        reopened.poll().unwrap();
        assert_eq!(
            reopened.state.agent(agent).unwrap().status,
            RuntimeStatus::Idle
        );
        let actual = reopened
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .clone();
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(
            actual, None,
            "recovered agent must not retain a stale update failure"
        );
    }
}

#[test]
fn recovered_update_preserves_an_unrelated_error_after_coordinator_restart() {
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![]);
    worker
        .state
        .observe_update_error(agent, Some("update failed".into()))
        .unwrap();
    worker
        .state
        .set_agent_error(agent, Some("unrelated delivery error".into()))
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    drop(worker);
    let info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
        "terminal_id":"terminal","pane_id":"pane","agent":"codex","agent_status":"idle",
        "agent_session":{"source":"herdr:codex","agent":"codex","kind":"id","value":"session"},
        "workspace_id":"w1","tab_id":"t1","focused":false,"interactive_ready":true,"revision":1
    }))
    .unwrap();
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls,
        state_path: dir.join("state.json"),
    };
    let mut reopened = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    reopened.poll().unwrap();
    let actual = reopened
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .clone();
    let update_error = reopened.state.agent(agent).unwrap().update_error.clone();
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(actual.as_deref(), Some("unrelated delivery error"));
    assert_eq!(update_error, None);
}

#[path = "control_priority_test.rs"]
mod control_priority_tests;

#[path = "provider_recovery_test.rs"]
mod provider_recovery;

#[test]
fn kimi_restore_preserves_a_persisted_provider_hold_until_its_original_request_can_retry() {
    #[cfg(target_os = "macos")]
    use crate::agents::providers::kimi::launch::update_test::resume_home_test::previous_mount;
    use crate::agents::providers::{
        kimi::launch::update_test::{resume_home_test::use_home, Fixture},
        ProviderKind,
    };
    use crate::agents::{
        providers::spool::{self, Manifest, RoutingKey},
        resume::catalog::{self, AgentSessionRef, PersistedAgentSession},
    };
    use crate::messaging::{
        coordinator::{
            resume::{load, NativeResumeContext, NativeResumeFacts},
            Worker,
        },
        identity,
        model::{
            AgentRuntimeIdentity, BusState, CallbackDisposition, CallbackEventKind, Provider,
            ProviderCallback, RuntimeStatus, SubmissionOutcome,
        },
        native::{Transport, TransportError},
        storage::state_store::JsonStore,
    };
    use crate::protocol::api::schema::{AgentInfo, Method, ResponseResult};
    let fixture = Fixture::new();
    crate::platform::fs::private_dir(&fixture.root).unwrap();
    fixture.prepare();
    let home = use_home(&fixture);
    #[cfg(target_os = "macos")]
    previous_mount(&fixture, true);
    let mut state = BusState::new();
    let room = state.create_room("held-restart").unwrap();
    let agent = state
        .create_agent(room, "owner", Provider::Kimi, fixture.root.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("owned-session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    state
        .set_draft_text(room, "finish the retained task")
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 10).unwrap()[0];
    state.begin_submission(request, "launch", 10).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("owned-session".into()),
                provider_turn_id: Some("7".into()),
            },
        )
        .unwrap();
    let started = ProviderCallback {
        callback_id: "held-start".into(),
        sequence: 11,
        occurred_at_ms: 11,
        agent_id: agent,
        launch_id: "launch".into(),
        provider_session_id: Some("owned-session".into()),
        provider_turn_id: Some("7".into()),
        provider_prompt_id: None,
        prompt_payload: Some(state.request(request).unwrap().prompt.rendered_payload()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        state.accept_callback(started.clone()),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "held-error".into(),
            sequence: 12,
            occurred_at_ms: 100,
            prompt_payload: None,
            kind: CallbackEventKind::Error {
                message: "Provider failed".into(),
                provider_error: Some("HTTP 503 Service Unavailable".into()),
            },
            ..started
        }),
        CallbackDisposition::AcceptedError
    );
    let hold = state.agent(agent).unwrap().provider_retry.clone().unwrap();
    let store = JsonStore::new(fixture.root.join("state.json"));
    store.save(&state).unwrap();
    spool::initialize(
        &fixture.hooks.spool,
        &Manifest {
            routing_key: RoutingKey(agent.0),
            provider: ProviderKind::Kimi,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    let session = PersistedAgentSession {
        source: "herdr:kimi".into(),
        agent: "kimi".into(),
        session_ref: AgentSessionRef::id("stale-native-session").unwrap(),
    };
    let plan = catalog::plan(&session.source, &session.agent, &session.session_ref).unwrap();
    let facts = NativeResumeFacts {
        agent_name: Some(identity::managed_name(room, agent)),
        managed_agent: Some("kimi".into()),
        session: Some(session),
    };
    let context = NativeResumeContext {
        root: Some(fixture.root.clone()),
        session_name: Some("bus".into()),
        config_root: fixture.root.join("herdr-config"),
        project: fixture.root.clone(),
        binary: fixture.hooks.binary.clone(),
    };
    let before = std::fs::read(store.path()).unwrap();
    let extras = load(&context, &facts, &plan).unwrap();
    assert_eq!(extras.session.unwrap().session_ref.value, "owned-session");
    assert!(extras
        .env
        .contains(&("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned())));
    assert_eq!(
        std::fs::read(store.path()).unwrap(),
        before,
        "native restore must not rewrite the hold"
    );
    struct RestoredKimi(AgentInfo);
    impl Transport for RestoredKimi {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentListWithDetectionContext(_) => Ok(ResponseResult::AgentList {
                    agents: vec![self.0.clone()],
                }),
                other => panic!("unexpected restore operation: {other:?}"),
            }
        }
    }
    let native = serde_json::from_value(serde_json::json!({
        "terminal_id":"restored-terminal", "pane_id":"restored-pane",
        "name":identity::managed_name(room, agent), "agent":"kimi", "agent_status":"idle",
        "workspace_id":"w1", "tab_id":"t1", "focused":false, "revision":1,
        "interactive_ready":true, "native_ready":true,
        "agent_session":{"source":"herdr:kimi", "agent":"kimi", "kind":"id", "value":"owned-session"}
    })).unwrap();
    let mut worker = Worker::open(fixture.root.clone(), Box::new(RestoredKimi(native))).unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().provider_retry.as_ref(),
        Some(&hold)
    );
    worker.poll().unwrap().unwrap();
    let owner = worker.state.agent(agent).unwrap();
    assert_eq!(
        owner.runtime_identity.pane_id.as_deref(),
        Some("restored-pane")
    );
    assert_eq!(
        owner.runtime_identity.session_id.as_deref(),
        Some("owned-session")
    );
    assert_eq!(owner.provider_retry.as_ref(), Some(&hold));
    assert_eq!(owner.current_request, Some(request));
    assert_eq!(owner.shown_status(), RuntimeStatus::Working);
    assert!(!worker.state.request(request).unwrap().settled());
    let mut persisted = store.load().unwrap().unwrap();
    assert_eq!(
        persisted.agent(agent).unwrap().provider_retry.as_ref(),
        Some(&hold)
    );
    assert!(persisted.retry_provider_requests(hold.retry_at_ms));
    assert_eq!(persisted.queued_requests(agent), &[request]);
    assert_eq!(
        persisted.agent(agent).unwrap().shown_status(),
        RuntimeStatus::Working
    );
}
