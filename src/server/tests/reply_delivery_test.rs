//! Reply delivery integration contract; historical scenarios and reproduction
//! evidence are inventoried in docs/reply-delivery-tests.md.
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{Provider, RequestPhase};
use serde_json::json;

#[tokio::test]
async fn provider_fixture_descheduling_is_distinct_from_a_real_submission_timeout() {
    for real_time in [true, false] {
        let mut f = Delivery::new(&[Provider::Codex], false);
        let agent = f.agents[0];
        if real_time {
            f.real_submission_time(agent);
        }
        f.pause_next_provider_pump(std::time::Duration::from_secs(3));
        let request = f.send(agent, f.work, "delayed fake provider", false, None);
        f.tick();
        let response = f
            .last_submission_response()
            .expect("no native submission response");
        eprintln!("real_time={real_time}; native response={response}");
        if real_time {
            // The provider was not serviced within the real confirmation window.
            // Its native refusal/timeout must remain observable, with no Enter.
            assert!(
                matches!(
                    response["error"]["code"].as_str(),
                    Some("timeout" | "agent_prompt_not_shown")
                ),
                "{response}"
            );
            assert!(f.typed(agent).is_empty(), "{response}");
            assert!(matches!(
                f.state().request(request).unwrap().phase,
                RequestPhase::Queued | RequestPhase::Submitting
            ));
        } else {
            assert_eq!(response["result"]["type"], "agent_prompted", "{response}");
            assert_eq!(f.typed_content(agent), ["delayed fake provider"]);
            assert_eq!(
                f.state().request(request).unwrap().phase,
                RequestPhase::Active
            );
            assert!(!f.state().request(request).unwrap().uncertain_outcome);
            f.tick();
            assert_eq!(f.typed(agent).len(), 1, "must not submit twice");
        }
    }
}

#[tokio::test]
async fn open_messaging_pending_cross_room_messages_do_not_coalesce() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let sender = f.agents[0];
    let recipient = f.add_owned(f.master, Provider::Codex, "remote", false);
    let master = f.master;
    let work = f.work;
    f.status(recipient, AgentState::Working);
    f.tick();
    let first = f.send(recipient, master, "developer task", false, None);
    let next = f.send(recipient, work, "worker question", false, Some(sender));
    f.tick();
    f.status(recipient, AgentState::Idle);
    f.tick();
    assert_eq!(f.state().request(next).unwrap().phase, RequestPhase::Queued);
    assert_eq!(f.state().request(next).unwrap().group, None);
    let payload = f.typed(recipient).last().unwrap().clone();
    assert_eq!(f.typed_content(recipient).last().unwrap(), "developer task");
    f.start(recipient, "developer", &payload);
    f.stop(
        recipient,
        "developer",
        "developer answer",
        json!([]),
        json!([]),
    );
    f.elapse(31_000);
    let payload = f.typed(recipient).last().unwrap().clone();
    assert!(payload.ends_with("worker question"));
    f.start(recipient, "worker", &payload);
    f.stop(recipient, "worker", "worker answer", json!([]), json!([]));
    f.elapse(31_000);
    f.assert_reply(first, "developer answer");
    f.assert_reply(next, "worker answer");
}

#[tokio::test]
async fn open_messaging_cross_room_session_delivery_has_its_own_reply() {
    for sender_in_master in [false, true] {
        let mut f = Delivery::new(&[Provider::Codex], sender_in_master);
        let sender = f.agents[0];
        let source = if sender_in_master { f.master } else { f.work };
        let destination = if sender_in_master { f.work } else { f.master };
        let recipient = f.add_owned(destination, Provider::ClaudeCode, "remote-peer", false);
        let request = f.send(
            recipient,
            source,
            "cross-room question",
            false,
            Some(sender),
        );
        f.tick();
        let payload = f.typed(recipient).last().unwrap().clone();
        let expected = format!("agent-0 (room {}):\ncross-room question", source.0);
        assert_eq!(f.typed_content(recipient).last().unwrap(), &expected);
        f.reopen();
        f.start(recipient, "cross-room-turn", &payload);
        assert!(f.state().request(request).unwrap().trusted_start_bound);
        f.stop(
            recipient,
            "cross-room-turn",
            "cross-room answer",
            json!([]),
            json!([]),
        );
        f.elapse(31_000);
        f.assert_reply(request, "cross-room answer");
        let history = f.call("room.history", json!({"room":source.0.to_string()}));
        assert_eq!(
            history["messages"][0]["prompt"]["author"],
            json!({"agent":sender})
        );
        assert_eq!(
            history["messages"][0]["prompt"]["text"],
            "cross-room question"
        );
        assert!(
            f.call("room.history", json!({"room":destination.0.to_string()}))["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn worker_to_orchestrator_delivers_queued_message_into_its_session_and_settles() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let worker = f.agents[0];
    let orchestrator = f.add_orchestrator();
    assert_eq!(f.state().requests().count(), 0);
    f.status(orchestrator, AgentState::Working);
    f.tick();
    let sent = f.call(
        "message.send",
        json!({"room":f.work.0.to_string(),
        "as":worker.0.to_string(),"to":["orchestrator"],"text":"I am blocked","queue":true}),
    );
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    f.tick();
    assert_eq!(
        f.state().request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(f.typed(orchestrator).is_empty());
    f.reopen();
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    let payload = f.typed(orchestrator).last().unwrap().clone();
    assert!(payload.contains("I am blocked"), "{payload}");
    f.start(orchestrator, "worker-blocker", &payload);
    let delivered = f.state().request(request).unwrap();
    assert_eq!(delivered.agent_id, orchestrator);
    assert_eq!(delivered.room_id, f.work);
    assert_eq!(
        delivered.provider_session_id.as_deref(),
        Some(format!("session-{}", orchestrator.0).as_str())
    );
    assert!(delivered.trusted_start_bound);
    f.stop(
        orchestrator,
        "worker-blocker",
        "Try the fallback",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.assert_reply(request, "Try the fallback");
    let history = f.call("room.history", json!({"room":f.work.0.to_string()}));
    assert_eq!(
        history["messages"][0]["prompt"]["author"],
        json!({"agent":worker})
    );
    assert_eq!(
        history["messages"][0]["delivery"]["requests"][0]["stage"],
        "replied"
    );
    assert_eq!(
        history["messages"][0]["delivery"]["requests"][0]["reply"]["text"],
        "Try the fallback"
    );
    assert!(f.call("room.history", json!({"room":"master"}))["messages"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn worker_message_during_orchestrator_background_hold_keeps_the_held_master_reply() {
    // `from_worker` = a worker-initiated work-room message; otherwise the
    // developer's own follow-up in MASTER, the pre-existing steering path.
    for from_worker in [true, false] {
        let mut f = Delivery::new(&[Provider::Codex], false);
        let worker = f.agents[0];
        let orchestrator = f.add_orchestrator();
        let master = f.master;
        let held = f.send(orchestrator, master, "status", false, None);
        f.tick();
        let payload = f.typed(orchestrator).last().cloned().unwrap_or_else(|| {
            panic!(
                "native prompt not recorded; App response: {:?}",
                f.last_submission_response()
            )
        });
        f.start(orchestrator, "one", &payload);
        f.stop(
            orchestrator,
            "one",
            "captured master status",
            json!([{"id":"shell","type":"shell","status":"running","command":"sleep 600"}]),
            json!([]),
        );
        let follow_up = if from_worker {
            let sent = f.call(
                "message.send",
                json!({"room":f.work.0.to_string(),"as":worker.0.to_string(),
                    "to":["orchestrator"],"text":"I am blocked","queue":false}),
            );
            f.request_for_message(sent["message_id"].as_u64().unwrap())
        } else {
            f.send(orchestrator, master, "I am blocked", false, None)
        };
        f.tick();
        assert_eq!(f.state().request(follow_up).unwrap().group, None);
        f.assert_reply(held, "captured master status");
        let typed = f.typed(orchestrator).last().unwrap().clone();
        assert!(
            typed.contains("I am blocked"),
            "from_worker={from_worker}: {typed}"
        );
        f.start(orchestrator, "two", &typed);
        f.stop(
            orchestrator,
            "two",
            "answer to blocker",
            json!([]),
            json!([]),
        );
        f.elapse(31_000);
        f.assert_reply(follow_up, "answer to blocker");
        f.assert_reply(held, "captured master status");
    }
}

#[tokio::test]
async fn worker_message_to_orchestrator_busy_with_a_master_turn_gets_its_own_reply() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let worker = f.agents[0];
    let orchestrator = f.add_orchestrator();
    let master = f.master;
    let developer = f.send(orchestrator, master, "status", false, None);
    f.tick();
    let payload = f.typed(orchestrator).last().cloned().unwrap_or_else(|| {
        panic!(
            "native prompt not recorded; App response: {:?}",
            f.last_submission_response()
        )
    });
    f.start(orchestrator, "one", &payload);
    let sent = f.call(
        "message.send",
        json!({"room":f.work.0.to_string(),"as":worker.0.to_string(),
            "to":["orchestrator"],"text":"I am blocked"}),
    );
    let blocker = f.request_for_message(sent["message_id"].as_u64().unwrap());
    f.tick();
    f.stop(
        orchestrator,
        "one",
        "report for the developer",
        json!([]),
        json!([]),
    );
    f.elapse(31_000);
    f.assert_reply(developer, "report for the developer");
    assert_eq!(
        f.state().request(blocker).unwrap().group,
        None,
        "a work-room message joined the orchestrator's MASTER turn: {:?}",
        f.state().request(blocker).unwrap()
    );
    let typed = f.typed(orchestrator).last().unwrap().clone();
    assert!(typed.contains("I am blocked"), "{typed}");
    f.start(orchestrator, "worker-turn", &typed);
    assert!(f.state().request(blocker).unwrap().trusted_start_bound);
    f.stop(
        orchestrator,
        "worker-turn",
        "answer for the worker",
        json!([]),
        json!([]),
    );
    f.elapse(31_000);
    f.assert_reply(blocker, "answer for the worker");
    f.assert_reply(developer, "report for the developer");
}

#[tokio::test]
async fn worker_message_typed_into_orchestrator_names_its_worker_and_room() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let worker = f.agents[0];
    let orchestrator = f.add_orchestrator();
    f.call(
        "message.send",
        json!({"room":f.work.0.to_string(),"as":worker.0.to_string(),
            "to":["orchestrator"],"text":"I am blocked"}),
    );
    f.tick();
    let typed = f.typed(orchestrator).last().unwrap().clone();
    let name = f.state().agent(worker).unwrap().name.clone();
    assert!(
        typed.lines().next() == Some(format!("{name} (room {}):", f.work.0).as_str())
            && f.typed_content(orchestrator).last().unwrap()
                == &format!("{name} (room {}):\nI am blocked", f.work.0),
        "the orchestrator cannot tell this worker message from a developer prompt: {typed:?}"
    );
    let request = f.state().requests().next().unwrap().id;
    f.reopen();
    f.start(orchestrator, "attributed-worker", &typed);
    assert!(f.state().request(request).unwrap().trusted_start_bound);
    f.stop(
        orchestrator,
        "attributed-worker",
        "worker answer",
        json!([]),
        json!([]),
    );
    f.elapse(31_000);
    f.assert_reply(request, "worker answer");
}

#[tokio::test]
async fn work_room_message_never_types_into_orchestrators_own_running_turn() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let worker = f.agents[0];
    let orchestrator = f.add_orchestrator();
    // The orchestrator is in a turn it started itself (no Bus request bound).
    f.status(orchestrator, AgentState::Working);
    f.tick();
    let before = f.typed(orchestrator).len();
    let sent = f.call(
        "message.send",
        json!({"room":f.work.0.to_string(),"as":worker.0.to_string(),
            "to":["orchestrator"],"text":"I am blocked"}),
    );
    f.tick();
    f.tick();
    assert_eq!(
        f.typed(orchestrator)[before..].to_vec(),
        Vec::<String>::new(),
        "work-room message typed into the orchestrator's running MASTER turn"
    );
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    assert_eq!(
        f.state().request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert_eq!(f.state().request(request).unwrap().group, None);
    f.status(orchestrator, AgentState::Idle);
    f.tick();
    let payload = f.typed(orchestrator).last().unwrap().clone();
    assert_eq!(
        f.typed_content(orchestrator).last().unwrap(),
        &format!("agent-0 (room {}):\nI am blocked", f.work.0)
    );
    f.start(orchestrator, "independent-worker-turn", &payload);
    f.stop(
        orchestrator,
        "independent-worker-turn",
        "worker answer",
        json!([]),
        json!([]),
    );
    f.elapse(31_000);
    f.assert_reply(request, "worker answer");
}

#[tokio::test]
async fn numbered_native_draft_edits_never_publish_blocked_or_resolved_notices() {
    for in_master in [false, true] {
        let mut f = Delivery::new(&[Provider::ClaudeCode], in_master);
        let agent = f.agents[0];
        let room = if in_master { f.master } else { f.work };
        for draft in [
            "1. Confirm the bug\r\n  2. Add",
            "1. Confirm the bug\r\n  2. Add a regression test",
            "1. Confirm the bug\r\n  2. Add a regression test\r\n  3. Run the gates",
            "1. Confirm the bug\r\n  2. Add a regression test",
            "",
        ] {
            f.draw(agent, Some(&format!(
                "\x1b[2J\x1b[H────────────────────────\r\n❯ {draft}\r\n────────────────────────\r\n  ⏵⏵ auto mode on"
            )));
            for _ in 0..3 {
                f.tick();
            }
            f.reopen();
            f.tick();
            assert_eq!(f.state().notices(room).count(), 0, "draft edit: {draft}");
            assert_eq!(f.state().notices(f.master).count(), 0);
            assert!(f.state().agent(agent).unwrap().dialog_notice.is_none());
        }
        assert!(f.state().requests().next().is_none());
    }
}

#[tokio::test]
async fn numbered_codex_native_draft_edits_never_publish_blocked_notices() {
    for in_master in [false, true] {
        let mut f = Delivery::new(&[Provider::Codex], in_master);
        let agent = f.agents[0];
        let room = if in_master { f.master } else { f.work };
        for draft in [
            "1. Confirm the bug\r\n  2. Add",
            "1. Confirm the bug\r\n  2. Add a regression test",
            "1. Confirm the bug\r\n  2. Add a regression test\r\n  3. Run the gates",
            "",
        ] {
            f.draw(agent, Some(&format!(
                "\x1b[2J\x1b[H• Done. The parser is fixed.\r\n\r\n› {draft}\r\n\r\n  gpt-5.4 · minimal · ~/work/bus"
            )));
            for _ in 0..3 {
                f.tick();
            }
            assert_eq!(f.state().notices(room).count(), 0, "draft edit: {draft}");
            assert!(f.state().agent(agent).unwrap().dialog_notice.is_none());
        }
    }
}

#[tokio::test]
async fn dialog_episodes_publish_once_despite_detection_flaps_and_restart() {
    let dialog = "\x1b[2J\x1b[HDo you want to proceed?\r\n ❯ 1. Yes\r\n   2. No\r\n\r\nEnter to confirm · Esc to cancel";
    for in_master in [false, true] {
        let mut f = Delivery::new(&[Provider::ClaudeCode], in_master);
        let agent = f.agents[0];
        let room = if in_master { f.master } else { f.work };
        for episode in 1..=2 {
            f.draw(agent, Some(dialog));
            for _ in 0..3 {
                f.tick();
            }
            // A synchronized redraw with no readable list metadata must not
            // settle an open episode. The native observation is unavailable.
            f.draw(agent, Some("\x1b[?2026h\x1b[2J\x1b[H"));
            for _ in 0..3 {
                f.tick();
            }
            assert!(f.state().agent(agent).unwrap().dialog_notice.is_some());
            f.draw(agent, Some(&format!("{dialog}\x1b[?2026l")));
            f.reopen();
            for status in [AgentState::Blocked, AgentState::Idle, AgentState::Working] {
                f.status(agent, status);
                f.draw(
                    agent,
                    Some(&dialog.replace("❯ 1. Yes\r\n   2. No", "  1. Yes\r\n ❯ 2. No")),
                );
                for _ in 0..3 {
                    f.tick();
                }
            }
            let notices: Vec<_> = f.state().notices(room).collect();
            // One blocked notice per episode; a closed dialog posts nothing.
            assert_eq!(notices.len(), episode);
            assert_eq!(
                notices
                    .iter()
                    .filter(|n| n.text.contains("Blocked, needs help"))
                    .count(),
                episode
            );
            assert!(f.state().requests().next().is_none());
            // Provider output after the answer closes the native dialog.
            f.draw(agent, None);
            for _ in 0..4 {
                f.tick();
            }
            f.reopen();
            for _ in 0..3 {
                f.tick();
            }
            let notices: Vec<_> = f.state().notices(room).collect();
            assert_eq!(notices.len(), episode);
            assert!(f.state().agent(agent).unwrap().dialog_notice.is_none());
            if !in_master {
                assert_eq!(f.state().notices(f.master).count(), 0);
            }
        }
    }
}

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
                let payload = f.typed(agent).last().unwrap().clone();
                f.start(agent, &format!("turn-{i}"), &payload);
                f.stop(agent, &format!("turn-{i}"), "answer", json!([]), json!([]));
                f.tick();
                f.reopen();
                f.assert_reply(request, "answer");
            }
        }
    }
}

#[tokio::test]
async fn cursor_injected_user_rows_and_advanced_transcript_keep_the_owned_final() {
    for omitted_suffix in ["", "\n\n[REDACTED]"] {
        let mut f = Delivery::new(&[Provider::Cursor], false);
        let agent = f.agents[0];
        let request = f.send(agent, f.work, "review", false, None);
        f.tick();
        f.start(agent, "review-turn", "review");
        let prompt = f.typed(agent).last().unwrap().clone();
        for row in [
            json!({"role":"assistant","message":{"content":[{"type":"text","text":format!("Checking.{omitted_suffix}")},{"type":"tool_use","name":"Read"}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"<available_subagent_types>...</available_subagent_types>"}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":prompt}]}}),
            json!({"role":"assistant","message":{"content":[{"type":"tool_use","name":"Read"}]}}),
            json!({"role":"assistant","message":{"content":[{"type":"text","text":"Ready"}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"newer request"}]}}),
            json!({"role":"assistant","message":{"content":[{"type":"text","text":"Newer answer"}]}}),
            json!({"type":"turn_ended","status":"success"}),
        ] {
            f.append_transcript(agent, row);
        }
        f.record(agent, json!({"hook_event_name":"afterAgentResponse","generation_id":"review-turn","text":"Checking.Ready"}));
        f.record(
            agent,
            json!({"hook_event_name":"stop","generation_id":"review-turn","status":"completed"}),
        );
        f.status(agent, AgentState::Idle);
        f.tick();
        f.reopen();
        f.assert_reply(request, "Ready");
        assert_eq!(f.state().room(f.work).unwrap().unread_count, 1);
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
async fn queued_worker_notice_after_held_master_turn_reaches_work_room_and_master_once() {
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
        RequestPhase::Completed
    );
    let follow = f.blocked_worker_notice(agent);
    f.tick();
    assert_eq!(f.state().request(follow).unwrap().group, None);
    let before_work = f.state().room(f.work).unwrap().unread_count;
    f.assert_reply(first, "working on it");
    let typed = f.typed(agent).last().unwrap().clone();
    assert!(typed.contains("Blocked, needs help to continue"), "{typed}");
    f.start(agent, "worker-notice", &typed);
    f.stop(
        agent,
        "worker-notice",
        "worker notice acknowledged",
        json!([]),
        json!([]),
    );
    f.elapse(6_000);
    f.assert_reply(follow, "worker notice acknowledged");
    f.assert_reply(first, "working on it");
    assert_eq!(f.state().room(f.master).unwrap().unread_count, 1);
    assert_eq!(
        f.state().room(f.work).unwrap().unread_count,
        before_work + 1
    );
    f.assert_no_stalls();
}

#[tokio::test]
async fn captured_stop_survives_a_no_stop_wake_and_restart_before_the_next_request() {
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
    f.start(agent, "wake", "<task-notification>\n<task-id>shell</task-id>\n<status>completed</status>\n</task-notification>");
    f.reopen();
    f.status(agent, AgentState::Idle);
    f.tick();
    f.elapse(31_000);
    f.assert_reply(first, "captured before wake");
    let next = f.send(agent, f.work, "queued", true, None);
    f.tick();
    f.start(agent, "two", "queued");
    f.stop(agent, "two", "queued answer", json!([]), json!([]));
    f.assert_reply(next, "queued answer");
}

#[tokio::test]
async fn background_pending_cannot_discard_captured_reply_or_block_queue() {
    for master in [false, true] {
        for (label, tasks, crons) in [
            (
                "cron",
                json!([]),
                json!([{"id":"cron","schedule":"* * * * *"}]),
            ),
            (
                "non-shell",
                json!([{"id":"agent","type":"local_agent","status":"running"}]),
                json!([]),
            ),
            (
                "shell-and-cron",
                json!([{"id":"shell","type":"shell","status":"running","command":"sleep 600"}]),
                json!([{"id":"cron","schedule":"* * * * *"}]),
            ),
            (
                "shell-and-agent",
                json!([{"id":"shell","type":"shell","status":"running","command":"sleep 600"},{"id":"agent","type":"local_agent","status":"running"}]),
                json!([]),
            ),
        ] {
            let mut f = Delivery::new(&[Provider::ClaudeCode], master);
            let agent = f.agents[0];
            let room = if master { f.master } else { f.work };
            let first = f.send(agent, room, "status", false, None);
            f.tick();
            f.start(agent, "one", "status");
            f.stop(agent, "one", "captured status", tasks, crons);
            let next = f.send(agent, room, "queued", true, None);
            f.tick();
            f.elapse(3_600_000);
            f.assert_reply(first, "captured status");
            f.start(agent, "two", "queued");
            f.stop(agent, "two", "next answer", json!([]), json!([]));
            f.assert_reply(next, "next answer");
            f.reopen();
            f.assert_reply(first, "captured status");
            f.assert_reply(next, "next answer");
            f.assert_no_stalls();
            eprintln!("background_pending: {label}, master={master}: both requests replied once after reopen");
        }
    }
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
            let payload = f.typed(agent).last().unwrap().clone();
            f.start(agent, &turn, &payload);
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
async fn flagged_nested_provider_callbacks_do_not_steal_active_request_identity() {
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
    let sent = f.call("message.send", json!({"room":"master","to":[agent.0.to_string()],"text":"please review this screenshot","files":[image]}));
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    let text = f.correlated_text(request);
    f.captured_composer(
        agent,
        &include_str!("../api/agents/tests/fixtures/claude_image_paste.txt")
            .replace("please review this screenshot", &text),
    );
    f.tick();
    assert_eq!(
        f.typed(agent).len(),
        1,
        "native image composer must receive Enter"
    );
    f.start(
        agent,
        "image",
        &format!("[Image #1]<pasted_content id=\"image\">\n{text}\n</pasted_content id=\"image\">"),
    );
    f.stop(agent, "image", "screenshot reviewed", json!([]), json!([]));
    f.assert_reply(request, "screenshot reviewed");
}

/// Live 2026-10-21: text long enough for Claude to collapse, sent with an image,
/// shows as `[Image #1][Pasted text #2 +3 lines]`. Bus withdrew it as never shown
/// every 20 s and the messages queued behind it never ran.
#[tokio::test]
async fn claude_image_chip_beside_a_collapsed_paste_is_submitted_and_binds_its_reply() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let agent = f.agents[0];
    f.captured_composer(
        agent,
        &include_str!("../api/agents/tests/fixtures/claude_image_long_paste.txt")
            .replace("+3 lines", "+4 lines"),
    );
    let image = f.dir.join("attachment.png");
    image::RgbImage::new(2, 2).save(&image).unwrap();
    let text = "first line\nsecond line\nthird line\nfourth line";
    let sent = f.call(
        "message.send",
        json!({"room":"master","to":[agent.0.to_string()],"text":text,"files":[image]}),
    );
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    let text = f.correlated_text(request);
    f.tick();
    assert_eq!(
        f.typed(agent).len(),
        1,
        "an image chip beside a collapsed paste is our draft and must receive Enter"
    );
    f.start(
        agent,
        "image",
        &format!("[Image #1]<pasted_content id=\"image\">\n{text}\n</pasted_content id=\"image\">"),
    );
    f.stop(agent, "image", "templates reviewed", json!([]), json!([]));
    f.assert_reply(request, "templates reviewed");
}

/// An image needs its own native paste even when the Bus message also carries
/// correlation text. Both must reach the same submitted provider message.
#[tokio::test]
async fn codex_image_and_correlation_text_are_submitted_together_and_bind_the_reply() {
    let mut f = Delivery::new(&[Provider::Codex], true);
    let agent = f.agents[0];
    let image = f.dir.join("attachment.png");
    image::RgbImage::new(2, 2).save(&image).unwrap();
    let sent = f.call(
        "message.send",
        json!({"room":"master","to":[agent.0.to_string()],"text":"","files":[image]}),
    );
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    let text = f.correlated_text(request);
    let expected = format!("[Image #1] {}", text.trim_end_matches('\n'));
    f.captured_composer(
        agent,
        &include_str!("../api/agents/tests/fixtures/codex_image_only_paste.txt")
            .replace("[Image #1]", &expected),
    );
    f.tick();
    assert_eq!(
        f.typed(agent).len(),
        1,
        "the correlated image attachment draft must receive Enter"
    );
    assert_eq!(
        f.attached_images(agent),
        [vec![image.to_string_lossy().into_owned()]]
    );
    assert_eq!(f.typed(agent), [expected.as_str()]);
    f.start(agent, "image", &expected);
    assert!(f.state().request(request).unwrap().trusted_start_bound);
    f.stop(agent, "image", "image reviewed", json!([]), json!([]));
    f.assert_reply(request, "image reviewed");
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
    // This is a continuously live waiter in the scripted provider timeline;
    // host scheduling must not expire its lease between scripted status reads.
    let waiter_time = std::time::Instant::now();
    f.set_reply_waiter_time(waiter_time);
    let agent = f.agents[0];
    let orch = f.add_orchestrator();
    let request = f.send(agent, f.work, "owned work", false, Some(orch));
    f.tick();
    let payload = f.typed(agent).last().unwrap().clone();
    f.start(agent, "owned", &payload);
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
    f.call(
        "message.status",
        json!({"message":message.0.to_string(),"as":orch.0.to_string()}),
    );
    f.tick();
    let typed = f.typed(orch);
    assert_eq!(typed.len(), 1, "typed payloads: {typed:#?}");
    assert!(
        typed[0].contains("autonomous result"),
        "typed payloads: {typed:#?}"
    );
    assert!(
        !typed[0].contains("owned result"),
        "a live waiter owns the original reply; typed payloads: {typed:#?}"
    );
    f.assert_reply(request, "owned result");
    f.set_reply_waiter_time(waiter_time + std::time::Duration::from_secs(3));
    f.tick();
    assert_eq!(
        f.state()
            .requests()
            .filter(|r| r.agent_id == orch && r.prompt.text.contains("owned result"))
            .count(),
        1,
        "the same unacknowledged reply routes once its waiter lease expires"
    );
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
        // Commit the status poll before the Stop exists: the injected fault
        // must hit the callback batch's durable write and deferred ack.
        f.status(agent, AgentState::Idle);
        f.tick();
        f.record(agent, stop);
        f.fail_next_save();
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| f.failed_tick());
        let log = capture.text();
        assert!(log.contains("bus.storage.failed"), "{log}");
        assert!(
            !log.contains("stage=\"poll\"") && !log.contains("stage=poll"),
            "{log}"
        );
        assert_eq!(
            f.state().request(request).unwrap().phase,
            RequestPhase::Active
        );
        let stop_count = std::fs::read_dir(f.dir.join(format!("callbacks/launch-{}", agent.0)))
            .unwrap()
            .filter_map(|p| std::fs::read(p.unwrap().path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter(|record| record["value"]["hook_event_name"] == "Stop")
            .count();
        assert_eq!(
            stop_count, 1,
            "failed callback save must not acknowledge its Stop"
        );
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
        let payload = f.typed(agent).last().unwrap().clone();
        f.start(agent, "build", &payload);
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
        assert_eq!(f.typed(agent).len(), 1, "timeout recovery must not retype the prompt");
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
async fn transcript_unmatched_diagnostic_is_not_rearmed_by_status_flapping_or_restart() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], false);
    let agent = f.agents[0];
    f.send(agent, f.work, "lost reply", false, None);
    f.tick();
    f.start(agent, "lost", "lost reply");
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.tick();
    assert_eq!(f.stall_diagnostics(), 1);
    f.status(agent, AgentState::Working);
    f.tick();
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.reopen();
    f.tick();
    assert_eq!(f.stall_diagnostics(), 1);
}

// One cycle is also a normal integration guard: both rounds must really hold
// Claude replies, consume batches and publish distinct response text once.
#[tokio::test]
async fn batched_reply_cycles_hold_fresh_shells_after_reopen() {
    let mut f = soak_fixture();
    for round in 0..2 {
        delivery_cycle(&mut f, round);
        stall_cycle(&mut f, round);
        orchestrator_inbox_burst(round);
        f.reopen();
        f.provider_startup();
    }
    assert_eq!(f.state().requests().count(), 50);
    assert_eq!(f.stall_diagnostics(), 2);
}

#[tokio::test]
#[ignore = "separate soak target; default 1 minute, BUS_REPLY_SOAK_MINUTES=30 for long mode"]
async fn soak_many_agents_concurrent_messages_and_background_shells() {
    run_soak();
}

fn soak_fixture() -> Delivery {
    let providers: Vec<_> = (0..12)
        .map(|n| {
            if n % 2 == 0 {
                Provider::ClaudeCode
            } else {
                Provider::Codex
            }
        })
        .collect();
    Delivery::new(&providers, false)
}

fn delivery_cycle(f: &mut Delivery, round: usize) {
    let agents = f.agents.clone();
    let room = f.work;
    let shared = std::sync::Mutex::new(&mut *f);
    let expected = std::thread::scope(|scope| {
        let handles: Vec<_> = agents
            .iter()
            .enumerate()
            .map(|(n, &agent)| {
                let shared = &shared;
                scope.spawn(move || {
                    let prompt = format!("soak-{round}-{n}");
                    let first = shared
                        .lock()
                        .unwrap()
                        .send(agent, room, &prompt, false, None);
                    let next = shared.lock().unwrap().send(
                        agent,
                        room,
                        &format!("queued-{round}-{n}"),
                        true,
                        None,
                    );
                    (n, agent, first, next)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    drop(shared);
    f.tick();
    for (phase, queued) in [("first", false), ("queued", true)] {
        for &(n, agent, _, _) in &expected {
            let prompt = if queued {
                format!("queued-{round}-{n}")
            } else {
                format!("soak-{round}-{n}")
            };
            f.spool_start(agent, &format!("{phase}-turn-{round}-{n}"), &prompt);
        }
        f.tick(); // All agents' starts are consumed in this batch.
        for &(n, agent, _, _) in &expected {
            let reply = format!("reply-{phase}-{round}-{n}");
            f.spool_stop(agent, &format!("{phase}-turn-{round}-{n}"), &reply,
                json!([{"id":"persistent-shell","type":"shell","status":"running","command":"sleep 600"},
                       {"id":format!("own-{phase}-{round}-{n}"),"type":"shell","status":"running","command":"sleep 600"}]), json!([]));
        }
        f.tick(); // All Stops are consumed together, not one tick per hook.
        for &(_, agent, first, next) in &expected {
            if f.state().agent(agent).unwrap().provider == Provider::ClaudeCode {
                let request = if queued { next } else { first };
                assert_eq!(
                    f.state().request(request).unwrap().phase,
                    RequestPhase::Completed,
                    "round {round} {phase}: Stop must complete despite running shells"
                );
            }
        }
        f.elapse(3_600_000);
        for &(n, _, first, next) in &expected {
            f.assert_reply(
                if queued { next } else { first },
                &format!("reply-{phase}-{round}-{n}"),
            );
        }
    }
}

fn stall_cycle(f: &mut Delivery, round: usize) {
    let agent = f.agents[0];
    let prompt = format!("stall-probe-{round}");
    let turn = format!("stall-turn-{round}");
    let request = f.send(agent, f.work, &prompt, false, None);
    f.tick();
    f.start(agent, &turn, &prompt);
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.tick();
    assert_eq!(
        f.stall_diagnostics_for(request),
        1,
        "the phase must really record a diagnostic"
    );
    f.status(agent, AgentState::Working);
    f.tick();
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.reopen();
    assert_eq!(
        f.stall_diagnostics_for(request),
        1,
        "same request diagnosed twice after flapping/restart"
    );
    f.stop(
        agent,
        &turn,
        &format!("late-reply-{round}"),
        json!([]),
        json!([]),
    );
    f.assert_reply(request, &format!("late-reply-{round}"));
    assert_eq!(f.stall_diagnostics_for(request), 1);
}

fn run_soak() {
    use std::time::{Duration, Instant};
    let minutes: f64 = std::env::var("BUS_REPLY_SOAK_MINUTES")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .expect("positive number of minutes");
    assert!(minutes.is_finite() && minutes > 0.0 && minutes <= 1440.0);
    let mut f = soak_fixture();
    let began = Instant::now();
    let deadline = began + Duration::from_secs_f64(minutes * 60.0);
    let mut round = 0;
    while Instant::now() < deadline {
        delivery_cycle(&mut f, round);
        assert_eq!(
            f.stall_diagnostics(),
            round,
            "delivery phase recorded an unexpected stall diagnostic"
        );
        stall_cycle(&mut f, round);
        orchestrator_inbox_burst(round);
        f.reopen();
        // Startup is explicit: an unknown shell after coordinator restart is
        // deliberately treated as inherited by production. This profile needs
        // fresh provider-owned shell holds on every subsequent cycle as well.
        f.provider_startup();
        round += 1;
    }
    let completed = round * 25;
    assert!(completed >= 25);
    assert_eq!(f.state().requests().count(), completed);
    assert!(f
        .state()
        .requests()
        .all(|r| r.phase == RequestPhase::Completed));
    let notices: Vec<_> = f.state().notices(f.work).collect();
    assert!(
        notices.is_empty(),
        "unexpected work-room notices: {notices:?}"
    );
    assert_eq!(
        f.state().room(f.work).unwrap().unread_count as usize,
        completed + notices.len()
    );
    assert_eq!(f.stall_diagnostics(), round);
    f.assert_publications();
    eprintln!("reply soak: {completed} completed load requests plus {} orchestrator inbox requests, {round} rounds, 12 load agents, {:?}, {} intentional stall diagnostics, {} idle-worker notices, no repeated stall diagnostics", round * 30, began.elapsed(), f.stall_diagnostics(), notices.len());
}

/// Negative control: the contract must notice a second visible publication,
/// even though room.history still contains just one request map entry.
#[tokio::test]
async fn reply_contract_rejects_second_visible_publication() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let agent = f.agents[0];
    let request = f.send(agent, f.master, "oracle prompt", false, None);
    f.tick();
    f.start(agent, "original", "oracle prompt");
    f.stop(agent, "original", "oracle reply", json!([]), json!([]));
    f.assert_reply(request, "oracle reply");
    f.start(
        agent,
        "separate-report",
        "<task-notification>separate report</task-notification>",
    );
    let before = f.state().room(f.master).unwrap().unread_count;
    f.call("message.send", json!({"room":f.master.0.to_string(),"as":agent.0.to_string(),"to":["human"],"text":"oracle reply"}));
    assert_eq!(
        f.state().room(f.master).unwrap().unread_count,
        before + 1,
        "negative control must actually publish"
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.assert_reply(request, "oracle reply")
    }));
    assert!(
        result.is_err(),
        "a second visible publication escaped the reply contract"
    );
}

#[tokio::test]
async fn orchestrator_inbox_admits_master_peers_before_background_shells_finish() {
    for restart in [false, true] {
        let mut f = Delivery::new(&[Provider::ClaudeCode, Provider::Codex], true);
        let orch = f.agents[0];
        let sender = f.agents[1];
        let held = f.send(orch, f.work, "notice", false, None);
        f.tick();
        f.start(orch, "held", "notice");
        f.stop(
            orch,
            "held",
            "notice answer",
            json!([{"id":"watcher","type":"shell","status":"running","command":"sleep 3600"}]),
            json!([]),
        );
        let next = f.send(orch, f.master, "peer question", true, Some(sender));
        if restart {
            f.reopen();
        } else {
            f.tick();
        }
        let payload = f.typed(orch).last().unwrap().clone();
        assert!(
            payload.ends_with("peer question"),
            "inbox blocked by held reply: {payload}"
        );
        assert_eq!(f.state().request(next).unwrap().group, None);
        f.assert_reply(held, "notice answer");
        f.start(orch, "peer", &payload);
        // A replay from the old turn cannot overwrite the new request.
        f.stop(orch, "held", "notice answer", json!([]), json!([]));
        assert!(f.state().request(next).unwrap().pending_final.is_none());
        f.stop(orch, "peer", "peer answer", json!([]), json!([]));
        f.elapse(31_000);
        f.assert_reply(next, "peer answer");
        f.reopen();
        f.assert_reply(held, "notice answer");
        f.assert_reply(next, "peer answer");
    }
}

#[tokio::test]
async fn orchestrator_inbox_steers_real_blocked_notices_and_preserves_the_held_answer() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let orch = f.agents[0];
    let held = f.send(orch, f.master, "status", false, None);
    f.tick();
    f.start(orch, "status-turn", "status");
    let ordinary = f.send(orch, f.master, "ordinary followup", true, None);
    // Queue the actual notice while the provider is still generating.
    let notice = f.blocked_worker_notice(orch);
    assert_eq!(
        f.typed(orch).len(),
        2,
        "the real blocked notice must reach the live turn"
    );
    assert_eq!(f.state().request(notice).unwrap().group, Some(held));
    let payload = f.typed(orch).last().unwrap().clone();
    assert!(
        payload.contains("Blocked, needs help"),
        "notice did not take priority: {payload}"
    );
    f.spool_stop(
        orch,
        "status-turn",
        "captured status",
        json!([{"id":"watcher","type":"shell","status":"running","command":"sleep 3600"}]),
        json!([]),
    );
    // The notice starts its own turn before an idle boundary can leave it
    // unclaimed. Both hooks are reconciled before settlement in this tick.
    f.spool_start(orch, "notice-turn", &payload);
    f.tick();
    assert_eq!(
        f.state().request(ordinary).unwrap().phase,
        RequestPhase::Queued
    );
    assert_eq!(
        f.state().request(held).unwrap().phase,
        RequestPhase::Completed
    );
    // The provider ran the steered input as a separate turn. Its submit hook
    // detaches the notice while preserving the captured earlier answer.
    assert_eq!(f.state().request(notice).unwrap().group, None);
    f.assert_reply(held, "captured status");
    f.stop(orch, "notice-turn", "help given", json!([]), json!([]));
    f.elapse(31_000);
    f.assert_reply(notice, "help given");
    assert_eq!(f.typed_content(orch).last().unwrap(), "ordinary followup");
    f.start(orch, "ordinary-turn", "ordinary followup");
    f.stop(
        orch,
        "ordinary-turn",
        "ordinary answer",
        json!([]),
        json!([]),
    );
    f.elapse(31_000);
    f.assert_reply(ordinary, "ordinary answer");
}

#[tokio::test]
async fn orchestrator_inbox_human_turn_and_interrupt_cannot_renew_an_old_hold() {
    let mut f = Delivery::new(&[Provider::ClaudeCode, Provider::Codex], true);
    let orch = f.agents[0];
    let sender = f.agents[1];
    let held = f.send(orch, f.work, "notice", false, None);
    f.tick();
    f.start(orch, "held", "notice");
    f.stop(
        orch,
        "held",
        "original answer",
        json!([{"id":"watcher","type":"shell","status":"running","command":"sleep 3600"}]),
        json!([]),
    );
    // Exact first-party transcript metadata from the incident's direct input.
    f.human_start(orch, "human", "typed directly");
    assert_eq!(
        f.state().request(held).unwrap().provider_turn_id.as_deref(),
        Some("held")
    );
    let next = f.send(orch, f.master, "peer question", true, Some(sender));
    f.tick();
    assert_eq!(f.state().request(next).unwrap().phase, RequestPhase::Queued);
    // Interrupted Claude turn: no Stop hook, only a subsequent idle observation.
    f.status(orch, AgentState::Idle);
    f.elapse(31_000);
    f.assert_reply(held, "original answer");
    let payload = f.typed(orch).last().unwrap().clone();
    assert!(
        payload.ends_with("peer question"),
        "interrupt stranded the queue: {payload}"
    );
    f.start(orch, "peer", &payload);
    f.stop(orch, "peer", "peer answer", json!([]), json!([]));
    f.elapse(31_000);
    f.assert_reply(next, "peer answer");
}

fn orchestrator_inbox_burst(round: usize) {
    let mut f = Delivery::new(&[Provider::ClaudeCode, Provider::Codex], true);
    let orch = f.agents[0];
    let sender = f.agents[1];
    let held = f.send(orch, f.work, "work notice", false, None);
    f.tick();
    f.start(orch, "held", "work notice");
    let mut requests = Vec::new();
    for n in 0..28 {
        let prompt = format!("inbox-{round}-{n}");
        let room = if n % 2 == 0 { f.master } else { f.work };
        requests.push((f.send(orch, room, &prompt, true, Some(sender)), prompt));
    }
    let blocker = f.blocked_worker_notice(orch);
    let payload = f.typed(orch).last().unwrap().clone();
    assert!(
        payload.contains("Blocked, needs help"),
        "28 queued messages hid a dialog: {payload}"
    );
    assert_eq!(f.state().request(blocker).unwrap().group, Some(held));
    f.spool_stop(
        orch,
        "held",
        "held reply",
        json!([{"id":"long-wait","type":"shell","status":"running","command":"sleep 3600"}]),
        json!([]),
    );
    f.spool_start(orch, "blocker", &payload);
    f.tick();
    assert_eq!(
        f.state().request(held).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(f.state().request(blocker).unwrap().group, None);
    f.assert_reply(held, "held reply");
    f.stop(orch, "blocker", "blocker reply", json!([]), json!([]));
    f.elapse(31_000);
    f.assert_reply(blocker, "blocker reply");
    for (n, (request, prompt)) in requests.into_iter().enumerate() {
        let payload = f.typed(orch).last().unwrap().clone();
        assert!(payload.ends_with(&prompt), "wrong FIFO turn: {payload}");
        assert_eq!(f.state().request(request).unwrap().group, None);
        let turn = format!("inbox-turn-{round}-{n}");
        f.start(orch, &turn, &payload);
        // Every turn launches another long wait: a backlog must still drain
        // without waiting for a shell exit or aging a hold to its cap.
        f.stop(orch, &turn, &format!("answer-{prompt}"),
            json!([{"id":format!("wait-{n}"),"type":"shell","status":"running","command":"sleep 3600"}]), json!([]));
        if n == 27 {
            f.elapse(3_600_000);
        }
        f.assert_reply(request, &format!("answer-{prompt}"));
        if n < 27 {
            assert_eq!(
                f.state()
                    .request(request)
                    .unwrap()
                    .failure_reason
                    .as_deref(),
                None,
                "turn {n} must complete at Stop without a recovery reason"
            );
        }
        if n % 7 == 0 {
            f.reopen();
            f.record(
                orch,
                json!({"hook_event_name":"SessionStart","source":"startup"}),
            );
            f.tick();
        }
    }
    f.assert_publications();
    f.assert_no_stalls();
}

#[tokio::test]
async fn orchestrator_inbox_textless_background_stop_does_not_block_or_invent_a_reply() {
    let mut f = Delivery::new(&[Provider::Codex], false);
    let orch = f.add_orchestrator();
    let held = f.send(orch, f.work, "notice", false, None);
    f.tick();
    f.start(orch, "held", "notice");
    f.stop(orch, "held", "", json!([]), json!([{"cron_id":"cron"}]));
    let next = f.send(orch, f.master, "next", true, None);
    f.tick();
    assert_eq!(f.typed_content(orch).last().unwrap(), "next");
    assert_eq!(
        f.state().request(held).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(f.state().request(held).unwrap().pending_final.is_none());
    f.start(orch, "next", "next");
    f.stop(orch, "next", "next answer", json!([]), json!([]));
    f.elapse(31_000);
    f.assert_reply(next, "next answer");
}

#[tokio::test]
async fn orchestrator_inbox_unclaimed_steering_is_uncertain_without_blocking_the_next_room() {
    let mut f = Delivery::new(&[Provider::ClaudeCode], true);
    let orch = f.agents[0];
    let lead = f.send(orch, f.master, "first", false, None);
    f.tick();
    f.start(orch, "shared", "first");
    let folded = f.send(orch, f.master, "folded followup", false, None);
    f.tick();
    assert_eq!(f.state().request(folded).unwrap().group, Some(lead));
    assert!(!f.state().request(folded).unwrap().trusted_start_bound);
    f.stop(
        orch,
        "shared",
        "combined answer",
        json!([{"id":"wait","type":"shell","status":"running","command":"sleep 3600"}]),
        json!([]),
    );
    let next = f.send(orch, f.work, "new room", true, None);
    // No matching hook or transcript claimed this input. At the idle boundary
    // it must surface uncertainty immediately, without borrowing the answer.
    f.tick();
    assert_eq!(
        f.typed_content(orch).last().unwrap(),
        "new room",
        "folded steering blocked the inbox"
    );
    f.assert_reply(lead, "combined answer");
    assert_eq!(
        f.state().request(folded).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(
        f.state().request(folded).unwrap().failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(f.state().request(folded).unwrap().pending_final.is_none());
    assert!(f.state().request(folded).unwrap().uncertain_outcome);
    assert_eq!(f.state().request(next).unwrap().group, None);
    f.start(orch, "new", "new room");
    f.stop(orch, "new", "new answer", json!([]), json!([]));
    f.elapse(31_000);
    f.assert_reply(next, "new answer");
}
