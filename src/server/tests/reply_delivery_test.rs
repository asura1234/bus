//! Reply delivery integration contract; historical scenarios and reproduction
//! evidence are inventoried in docs/reply-delivery-tests.md.
use crate::agents::AgentState;
use crate::messaging::coordinator::reply_delivery_support::Delivery;
use crate::messaging::model::{Provider, RequestPhase};
use serde_json::json;

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
            assert_eq!(notices.len(), episode * 2 - 1);
            assert_eq!(
                notices
                    .iter()
                    .filter(|n| n.text.contains("Blocked, needs help"))
                    .count(),
                episode
            );
            assert_eq!(
                notices
                    .iter()
                    .filter(|n| n.text.contains("Resolved; dialog"))
                    .count(),
                episode - 1
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
            assert_eq!(notices.len(), episode * 2);
            assert_eq!(
                notices
                    .iter()
                    .filter(|n| n.text.contains("Resolved; dialog"))
                    .count(),
                episode
            );
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
async fn cursor_injected_user_rows_and_advanced_transcript_keep_the_owned_final() {
    for omitted_suffix in ["", "\n\n[REDACTED]"] {
        let mut f = Delivery::new(&[Provider::Cursor], false);
        let agent = f.agents[0];
        let request = f.send(agent, f.work, "review", false, None);
        f.tick();
        f.start(agent, "review-turn", "review");
        for row in [
            json!({"role":"assistant","message":{"content":[{"type":"text","text":format!("Checking.{omitted_suffix}")},{"type":"tool_use","name":"Read"}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"<available_subagent_types>...</available_subagent_types>"}]}}),
            json!({"role":"user","message":{"content":[{"type":"text","text":"review"}]}}),
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
    let follow = f.blocked_worker_notice(agent);
    f.tick();
    assert!(f
        .typed(agent)
        .last()
        .unwrap()
        .contains("Blocked, needs help to continue"));
    let before_work = f.state().room(f.work).unwrap().unread_count;
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
    assert_eq!(
        f.state().room(f.work).unwrap().unread_count,
        before_work + 1
    );
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
    f.captured_image_composer(agent);
    let image = f.dir.join("attachment.png");
    image::RgbImage::new(2, 2).save(&image).unwrap();
    let sent = f.call("message.send", json!({"room":"master","to":[agent.0.to_string()],"text":"please review this screenshot","files":[image]}));
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
    f.tick();
    assert_eq!(
        f.typed(agent).len(),
        1,
        "native image composer must receive Enter"
    );
    f.start(agent, "image", "[Image #1]<pasted_content id=\"image\">\nplease review this screenshot\n</pasted_content id=\"image\">");
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
        include_str!("../api/agents/tests/fixtures/claude_image_long_paste.txt"),
    );
    let image = f.dir.join("attachment.png");
    image::RgbImage::new(2, 2).save(&image).unwrap();
    let text = "first line\nsecond line\nthird line\nfourth line";
    let sent = f.call(
        "message.send",
        json!({"room":"master","to":[agent.0.to_string()],"text":text,"files":[image]}),
    );
    let request = f.request_for_message(sent["message_id"].as_u64().unwrap());
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
    f.call(
        "message.status",
        json!({"message":message.0.to_string(),"as":orch.0.to_string()}),
    );
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

// One cycle is also a normal integration guard: both rounds must really hold
// Claude replies, consume batches and publish distinct response text once.
#[tokio::test]
async fn batched_reply_cycles_hold_fresh_shells_after_reopen() {
    let mut f = soak_fixture();
    for round in 0..2 {
        delivery_cycle(&mut f, round);
        stall_cycle(&mut f, round);
        f.reopen();
        f.provider_startup();
    }
    assert_eq!(f.state().requests().count(), 50);
    assert_eq!(f.stall_alerts(), 2);
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
                assert!(
                    f.state().request(request).unwrap().shell_hold.is_some(),
                    "round {round} {phase}: Claude reply must actually wait on its own shell"
                );
                assert_eq!(
                    f.state().request(request).unwrap().phase,
                    RequestPhase::Active
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
        f.stall_alerts_for(request),
        1,
        "the phase must really trigger an alert"
    );
    f.status(agent, AgentState::Working);
    f.tick();
    f.status(agent, AgentState::Idle);
    f.tick();
    f.age_request_progress(600_000);
    f.reopen();
    assert_eq!(
        f.stall_alerts_for(request),
        1,
        "same request alerted twice after flapping/restart"
    );
    f.stop(
        agent,
        &turn,
        &format!("late-reply-{round}"),
        json!([]),
        json!([]),
    );
    f.assert_reply(request, &format!("late-reply-{round}"));
    assert_eq!(f.stall_alerts_for(request), 1);
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
            f.stall_alerts(),
            round,
            "delivery phase raised an unexpected alert"
        );
        stall_cycle(&mut f, round);
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
    assert_eq!(
        f.state().room(f.work).unwrap().unread_count as usize,
        completed
    );
    assert_eq!(f.stall_alerts(), round);
    f.assert_publications();
    eprintln!("reply soak: {completed} completed requests, {round} rounds, 12 agents, {:?}, {} intentional alerts, no repeated alerts", began.elapsed(), f.stall_alerts());
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
