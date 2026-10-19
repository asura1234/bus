//! Reply delivery integration contract; historical scenarios and reproduction
//! evidence are inventoried in docs/reply-delivery-tests.md.
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{Provider, RequestPhase};
use serde_json::json;

#[tokio::test]
async fn captured_reply_reaches_its_source_room_once_for_both_providers_and_authors() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        for master in [false, true] {
            let mut f = Delivery::new(&[provider, provider], master);
            let agent = f.agents[0];
            let sender = f.agents[1];
            let room = if master { f.master } else { f.work };
            for (i, author) in [None, Some(sender)].into_iter().enumerate() {
                let request = f.send(agent, room, "same status message", false, author);
                f.tick();
                assert_eq!(f.typed(agent).len(), i + 1);
                f.start(agent, &format!("turn-{i}"), "same status message");
                f.stop(agent, &format!("turn-{i}"), "answer", json!([]), json!([]));
                f.tick();
                f.reopen();
                f.assert_reply(request, "answer");
            }
        }
    }
}

#[tokio::test]
async fn independently_answered_turns_keep_five_distinct_replies_while_shells_live() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], false);
    let agent = f.agents[0];
    let mut requests = Vec::new();
    for n in 0..5 {
        let prompt = format!("message-{n}");
        let turn = format!("turn-{n}");
        let reply = format!("answer-{n}");
        let request = f.send(agent, f.work, &prompt, false, None);
        f.tick();
        f.start(agent, &turn, &prompt);
        let shells: Vec<_> = (0..=n).map(|i| json!({"id":format!("shell-{i}"),"type":"shell","status":"running","command":"sleep 600"})).collect();
        f.stop(agent, &turn, &reply, json!(shells), json!([]));
        requests.push((request, reply));
    }
    f.elapse(3_600_000);
    for (request, reply) in requests {
        f.assert_reply(request, &reply);
    }
}

#[tokio::test]
async fn followup_to_held_master_turn_reaches_work_room_and_master_once() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let agent = f.agents[0];
    let first = f.send(agent, f.master, "start work", false, None);
    f.tick();
    f.start(agent, "one", "start work");
    f.stop(
        agent,
        "one",
        "working on it",
        json!([{"id":"shell","type":"shell","status":"running","command":"sleep 600"}]),
        json!([]),
    );
    assert_eq!(
        f.state().request(first).unwrap().phase,
        RequestPhase::Active
    );
    let follow = f.submit_ui(agent, f.work, "include tests");
    f.tick();
    assert_eq!(f.typed(agent).last().unwrap(), "include tests");
    // This provider folds the follow-up into the original generation and emits
    // no additional submit hook, a distinct path from separate answered turns.
    f.stop(
        agent,
        "one",
        "work and tests complete",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.assert_reply(first, "work and tests complete");
    f.assert_reply(follow, "work and tests complete");
    assert_eq!(f.state().room(f.master).unwrap().unread_count, 1);
    assert_eq!(f.state().room(f.work).unwrap().unread_count, 1);
    f.assert_no_stalls();
}

#[tokio::test]
async fn held_interim_survives_no_stop_wake_restart_and_releases_queued_request() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], false);
    let agent = f.agents[0];
    let first = f.send(agent, f.work, "first", false, None);
    f.tick();
    f.start(agent, "one", "first");
    f.stop(
        agent,
        "one",
        "captured before wake",
        json!([{"id":"shell","type":"shell","status":"running","command":"sleep 600"}]),
        json!([]),
    );
    let next = f.send(agent, f.work, "queued", true, None);
    f.tick();
    assert_eq!(f.state().request(next).unwrap().phase, RequestPhase::Queued);
    f.start(agent, "wake", "<task-notification>\n<task-id>shell</task-id>\n<status>completed</status>\n</task-notification>");
    f.reopen();
    f.status(agent, AgentState::Idle);
    f.tick();
    f.elapse(31_000);
    f.assert_reply(first, "captured before wake");
    f.start(agent, "two", "queued");
    f.stop(agent, "two", "queued answer", json!([]), json!([]));
    f.assert_reply(next, "queued answer");
}

#[tokio::test]
#[ignore = "stall-cx: https://github.com/asura1234/bus/pull/107; captured Stop discarded with cron plus live shell"]
async fn long_lived_shell_and_cron_cannot_discard_captured_reply_or_block_queue() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let agent = f.agents[0];
    let first = f.send(agent, f.master, "status", false, None);
    f.tick();
    f.start(agent, "one", "status");
    f.stop(
        agent,
        "one",
        "captured status",
        json!([{"id":"shell","type":"shell","status":"running","command":"sleep 600"}]),
        json!([{"id":"cron","schedule":"* * * * *"}]),
    );
    let next = f.send(agent, f.master, "queued", true, None);
    f.tick();
    f.elapse(3_600_000);
    f.assert_reply(first, "captured status");
    f.start(agent, "two", "queued");
    f.stop(agent, "two", "next answer", json!([]), json!([]));
    f.assert_reply(next, "next answer");
}

#[tokio::test]
async fn coalesced_sends_share_one_answer_and_one_history_publication() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut f = Delivery::new(&[provider], false);
        let agent = f.agents[0];
        f.status(agent, AgentState::Blocked);
        f.tick();
        let ids: Vec<_> = ["first", "second", "third"]
            .iter()
            .map(|text| f.send(agent, f.work, text, false, None))
            .collect();
        f.tick();
        f.status(agent, AgentState::Idle);
        f.tick();
        let typed = f.typed(agent);
        assert_eq!(typed.len(), 1);
        let prompt = if provider == Provider::ClaudeCode {
            format!(
                "<pasted_content id=\"atomic\">\n{}\n</pasted_content id=\"atomic\">",
                typed[0]
            )
        } else {
            typed[0].clone()
        };
        f.start(agent, "folded", &prompt);
        f.stop(agent, "folded", "shared answer", json!([]), json!([]));
        f.elapse(6_000);
        let unread = f.state().room(f.work).unwrap().unread_count;
        f.reopen();
        f.tick();
        for request in ids {
            f.assert_reply(request, "shared answer");
        }
        assert_eq!(f.state().room(f.work).unwrap().unread_count, unread);
        assert_eq!(unread, 1, "one reply publication, despite three requests");
    }
}

#[tokio::test]
async fn direct_bus_control_shells_publish_equally_for_developer_and_agent() {
    for command in [
        "bus history --room 0",
        "'/tmp/Bus Tools/bus' send --to worker --async --text go",
        "bus agent read worker --source recent --lines 20",
    ] {
        let mut f = Delivery::new(&[Provider::ClaudeCode, Provider::ClaudeCode], true);
        let agent = f.agents[0];
        let sender = f.agents[1];
        for (n, author) in [None, Some(sender)].into_iter().enumerate() {
            let request = f.send(agent, f.master, "status", false, author);
            f.tick();
            let turn = format!("dispatch-{n}");
            f.start(agent, &turn, "status");
            f.stop(agent, &turn, "dispatched", json!([{"id":format!("control-{n}"),"type":"shell","status":"running","command":command}]), json!([]));
            f.assert_reply(request, "dispatched");
            f.assert_no_stalls();
        }
    }
}

#[tokio::test]
async fn captured_reply_survives_unrelated_later_turn_and_duplicate_stop() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut f = Delivery::new(&[provider], false);
        let agent = f.agents[0];
        let request = f.send(agent, f.work, "owned message", false, None);
        f.tick();
        f.start(agent, "owned", "owned message");
        f.stop(agent, "owned", "owned reply", json!([]), json!([]));
        f.start(agent, "unrelated", "typed directly in terminal");
        f.stop(agent, "unrelated", "unrelated reply", json!([]), json!([]));
        f.stop(agent, "owned", "owned reply", json!([]), json!([]));
        f.reopen();
        f.assert_reply(request, "owned reply");
        assert_eq!(f.state().room(f.work).unwrap().unread_count, 1);
    }
}

#[tokio::test]
async fn nested_provider_hooks_do_not_steal_active_request_identity() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut f = Delivery::new(&[provider], false);
        let agent = f.agents[0];
        let request = f.send(agent, f.work, "outer task", false, None);
        f.tick();
        f.start(agent, "outer", "outer task");
        f.nested_session(agent);
        f.tick();
        assert!(!f.state().agent(agent).unwrap().session_binding_invalidated);
        f.stop(agent, "outer", "outer reply", json!([]), json!([]));
        f.assert_reply(request, "outer reply");
    }
}

#[tokio::test]
async fn claude_image_chip_and_long_paste_bind_to_the_delivered_request() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let agent = f.agents[0];
    let image = f.dir.join("attachment.png");
    image::RgbImage::new(2, 2).save(&image).unwrap();
    let sent = f.call("message.send", json!({"room":"master","to":[agent.0.to_string()],"text":"review this screenshot","files":[image]}));
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    f.tick();
    assert_eq!(
        f.typed(agent).len(),
        1,
        "native image composer must receive Enter"
    );
    f.start(agent, "image", "[Image #1]<pasted_content id=\"image\">\nreview this screenshot\n</pasted_content id=\"image\">");
    f.stop(agent, "image", "screenshot reviewed", json!([]), json!([]));
    f.assert_reply(request, "screenshot reviewed");
}

#[tokio::test]
async fn codex_startup_dialog_keeps_requests_queued_until_composer_recovers() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let agent = f.agents[0];
    f.draw(
        agent,
        Some(include_str!(
            "../../../tests/fixtures/codex-update/chooser-0.162.ansi"
        )),
    );
    let request = f.send(agent, f.work, "do not lose this", true, None);
    f.tick();
    assert_eq!(
        f.state().request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(
        f.typed(agent).is_empty(),
        "the update chooser must never receive a message"
    );
    f.draw(agent, None);
    f.tick();
    f.start(agent, "after-update", "do not lose this");
    f.stop(
        agent,
        "after-update",
        "retained reply",
        json!([]),
        json!([]),
    );
    f.assert_reply(request, "retained reply");
}

#[tokio::test]
async fn autonomous_worker_final_routes_even_while_original_reply_has_a_waiter() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], false);
    let agent = f.agents[0];
    let orch = f.add_orchestrator();
    let request = f.send(agent, f.work, "owned work", false, Some(orch));
    f.tick();
    f.start(agent, "owned", "owned work");
    let message = f.state().request(request).unwrap().prompt.id;
    f.call(
        "message.status",
        json!({"message":message.0.to_string(),"as":orch.0.to_string()}),
    );
    f.stop(agent, "owned", "owned result", json!([]), json!([]));
    f.start(
        agent,
        "autonomous",
        "<task-notification>background task done</task-notification>",
    );
    f.stop(
        agent,
        "autonomous",
        "autonomous result",
        json!([]),
        json!([]),
    );
    f.age_request_progress(3_000);
    f.tick();
    let typed = f.typed(orch);
    assert_eq!(typed.len(), 1);
    assert!(typed[0].contains("autonomous result"));
    assert!(
        !typed[0].contains("owned result"),
        "a live waiter owns the original reply"
    );
    f.assert_reply(request, "owned result");
}

#[tokio::test]
async fn identical_autonomous_orchestrator_reports_are_distinct_across_turns() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let agent = f.agents[0];
    for turn in ["first-background-turn", "second-background-turn"] {
        f.start(
            agent,
            turn,
            "<task-notification>finished</task-notification>",
        );
        f.stop(agent, turn, "same final text", json!([]), json!([]));
        f.reopen();
    }
    assert_eq!(
        f.state()
            .notices(f.work)
            .filter(|n| n.text == "same final text")
            .count(),
        2
    );
    assert_eq!(
        f.state().notices(f.master).count(),
        0,
        "routine reports belong to the work room"
    );
    f.stop(
        agent,
        "second-background-turn",
        "same final text",
        json!([]),
        json!([]),
    );
    assert_eq!(
        f.state()
            .notices(f.work)
            .filter(|n| n.text == "same final text")
            .count(),
        2,
        "replaying one turn must not add a third report"
    );
}

#[tokio::test]
async fn codex_rollout_provider_error_is_explicit_and_does_not_poison_next_reply() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let agent = f.agents[0];
    let first = f.send(agent, f.work, "capacity test", false, None);
    f.tick();
    f.start(agent, "capacity", "capacity test");
    let next = f.send(agent, f.work, "retry task", true, None);
    f.append_transcript(agent, json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"capacity","last_agent_message":null,"error":{"message":"Model at capacity","codex_error_info":"server_overloaded"}}}));
    f.status(agent, AgentState::Idle);
    f.tick();
    f.elapse(6_000);
    let r = f.state().request(first).unwrap();
    assert_eq!(r.phase, RequestPhase::Abandoned);
    assert!(r
        .failure_reason
        .as_ref()
        .unwrap()
        .contains("provider_error: Model at capacity"));
    f.start(agent, "retry", "retry task");
    f.stop(agent, "retry", "retry succeeded", json!([]), json!([]));
    f.assert_reply(next, "retry succeeded");
}

#[tokio::test]
async fn failed_reply_persistence_replays_spool_after_restart_without_duplication() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut f = Delivery::new(&[provider], false);
        let agent = f.agents[0];
        let request = f.send(agent, f.work, "durable reply", false, None);
        f.tick();
        f.start(agent, "durable", "durable reply");
        let key = if provider == Provider::Codex {
            "turn_id"
        } else {
            "prompt_id"
        };
        let mut stop = json!({"hook_event_name":"Stop","last_assistant_message":"saved after retry","background_tasks":[],"session_crons":[]});
        stop[key] = json!("durable");
        f.record(agent, stop);
        f.status(agent, AgentState::Idle);
        f.fail_next_save();
        f.failed_tick();
        f.reopen();
        f.tick();
        f.assert_reply(request, "saved after retry");
        assert_eq!(f.state().room(f.work).unwrap().unread_count, 1);
    }
}

#[tokio::test]
async fn worker_reply_wakes_orchestrator_once_without_waiter_and_survives_reopen() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut f = Delivery::new(&[provider], false);
        let agent = f.agents[0];
        let orch = f.add_orchestrator();
        let request = f.send(agent, f.work, "build", false, Some(orch));
        f.tick();
        f.start(agent, "build", "build");
        f.stop(agent, "build", "worker result", json!([]), json!([]));
        f.age_request_progress(3_000);
        f.reopen();
        let typed = f.typed(orch);
        assert_eq!(
            typed.len(),
            1,
            "worker reply must reach orchestrator terminal"
        );
        assert!(typed[0].contains("worker result"));
        f.start(orch, "notice", &typed[0]);
        f.stop(orch, "notice", "acknowledged", json!([]), json!([]));
        f.reopen();
        f.tick();
        assert_eq!(f.typed(orch).len(), 1);
        f.assert_reply(request, "worker result");
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socket_eagain_during_active_reply_preserves_ownership_and_recovers_once() {
    use std::time::{Duration, Instant};
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut f = Delivery::new(&[Provider::Codex], false); let agent = f.agents[0]; f.use_socket();
        let request = f.send(agent, f.work, "slow server", false, None); f.tick(); f.start(agent, "slow", "slow server");
        f.record(agent, json!({"hook_event_name":"Stop","turn_id":"slow","last_assistant_message":"recovered reply"}));
        f.status(agent, AgentState::Idle); f.pause_server(true);
        let began = Instant::now(); f.tick();
        assert!(began.elapsed() >= Duration::from_secs(3), "real kernel receive deadline must expire");
        assert_eq!(f.state().request(request).unwrap().phase, RequestPhase::Active);
        f.pause_server(false); f.tick(); f.tick(); f.assert_reply(request, "recovered reply");
    });
    let log = capture.text();
    assert!(
        log.contains("bus.api.io_error") && log.contains("read_response"),
        "{log}"
    );
    assert!(
        log.contains("WouldBlock") || log.contains("TimedOut"),
        "{log}"
    );
}

#[tokio::test]
#[ignore = "stall-cx: https://github.com/asura1234/bus/pull/107; one alert per stalled request across Working/Idle/restart"]
async fn transcript_unmatched_alert_is_not_rearmed_by_status_flapping_or_restart() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], false);
    let agent = f.agents[0];
    f.send(agent, f.work, "lost reply", false, None);
    f.tick();
    f.start(agent, "lost", "lost reply");
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.tick();
    assert_eq!(f.stall_alerts(), 1);
    f.status(agent, AgentState::Working);
    f.tick();
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.reopen();
    f.tick();
    assert_eq!(f.stall_alerts(), 1);
}

#[tokio::test]
#[ignore = "separate soak target; default 1 minute, BUS_REPLY_SOAK_MINUTES=30 for long mode"]
async fn soak_many_agents_concurrent_messages_and_background_shells() {
    use std::time::{Duration, Instant};
    let minutes: f64 = std::env::var("BUS_REPLY_SOAK_MINUTES")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .expect("positive number of minutes");
    assert!(minutes.is_finite() && minutes > 0.0 && minutes <= 1440.0);
    let providers: Vec<_> = (0..12)
        .map(|n| {
            if n % 2 == 0 {
                Provider::ClaudeCode
            } else {
                Provider::Codex
            }
        })
        .collect();
    let mut f = Delivery::new(&providers, false);
    let agents = f.agents.clone();
    let began = Instant::now();
    let deadline = began + Duration::from_secs_f64(minutes * 60.0);
    let mut round = 0;
    let mut completed = 0;
    while Instant::now() < deadline {
        let mut expected = Vec::new();
        // All agents have outstanding work together; each also has queued work
        // behind it while Claude reports a persistent background shell.
        for (n, &agent) in agents.iter().enumerate() {
            let prompt = format!("soak-{round}-{n}");
            let first = f.send(agent, f.work, &prompt, false, None);
            let next = f.send(agent, f.work, &format!("queued-{round}-{n}"), true, None);
            expected.push((agent, first, next, prompt));
        }
        f.tick();
        for (n, &(agent, _, _, ref prompt)) in expected.iter().enumerate() {
            let turn = format!("turn-{round}-{n}");
            f.start(agent, &turn, prompt);
            f.stop(agent, &turn, prompt, json!([{"id":"persistent-shell","type":"shell","status":"running","command":"sleep 600"}]), json!([]));
        }
        f.elapse(3_600_000);
        for (n, (agent, first, next, prompt)) in expected.into_iter().enumerate() {
            f.assert_reply(first, &prompt);
            let queued = format!("queued-{round}-{n}");
            let turn = format!("queued-turn-{round}-{n}");
            f.start(agent, &turn, &queued);
            f.stop(agent, &turn, &queued, json!([{"id":"persistent-shell","type":"shell","status":"running","command":"sleep 600"}]), json!([]));
            f.assert_reply(next, &queued);
            completed += 2;
        }
        f.reopen();
        f.assert_no_stalls();
        round += 1;
    }
    assert!(completed >= 24);
    assert_eq!(f.state().requests().count(), completed);
    assert!(f
        .state()
        .requests()
        .all(|r| r.phase == RequestPhase::Completed));
    eprintln!("reply soak: {completed} completed requests, {round} rounds, 12 agents, {:?}, zero stall alerts", began.elapsed());
}
