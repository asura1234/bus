use crate::agents::providers::{
    parse,
    spool::{append, boundary, initialize, records, Manifest, Parsed, RoutingKey},
    ProviderKind as Provider,
};
use crate::utils::time as bus_io;

#[cfg(test)]
#[test]
fn codex_background_sessions_without_transcripts_are_not_terminal_callbacks() {
    for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
        let value = serde_json::json!({"hook_event_name":event,"session_id":"background-title-or-memory","transcript_path":null,"turn_id":"background-turn","prompt":"Generate title","last_assistant_message":"Title"});
        assert_eq!(parse(Provider::Codex, &value).unwrap(), Parsed::Ignore);
    }
}

#[cfg(test)]
#[test]
fn claude_stop_with_text_is_final_despite_live_background_work() {
    for extra in [
        serde_json::json!({"background_tasks":[{"task_id":"task-1","status":"running"}],"session_crons":[]}),
        serde_json::json!({"background_tasks":[],"session_crons":[{"cron_id":"cron-1","status":"running"}]}),
    ] {
        let mut value = serde_json::json!({
            "hook_event_name":"Stop",
            "session_id":"claude-session",
            "prompt_id":"prompt-1",
            "last_assistant_message":"Still waiting on a background reviewer"
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            parse(Provider::ClaudeCode, &value).unwrap(),
            Parsed::Final {
                session: "claude-session".into(),
                turn: "prompt-1".into(),
                text: "Still waiting on a background reviewer".into(),
            }
        );
    }

    // Shape captured from Claude Code 2.1.281: a shell left running by an
    // earlier turn is listed on every later Stop and must not hold the reply.
    for background_tasks in [
        serde_json::json!([]),
        serde_json::json!([
            {"id":"bi0m87z1r","type":"shell","status":"running","command":"find / -name x | head -3","description":"Find source"},
            {"id":"bl9tk0sxp","type":"shell","status":"running","command":"sleep 240","description":"Sleep"}
        ]),
    ] {
        let settled = serde_json::json!({
            "hook_event_name":"Stop",
            "session_id":"claude-session",
            "prompt_id":"prompt-1",
            "last_assistant_message":"Final review",
            "background_tasks":background_tasks,
            "session_crons":[]
        });
        assert!(matches!(
            parse(Provider::ClaudeCode, &settled).unwrap(),
            Parsed::Final { text, .. } if text == "Final review"
        ));
    }
    let agent_beside_shell = serde_json::json!({
        "hook_event_name":"Stop",
        "session_id":"claude-session",
        "prompt_id":"prompt-1",
        "last_assistant_message":"Waiting for reviewers",
        "background_tasks":[
            {"id":"shell-1","type":"shell","status":"running"},
            {"id":"agent-1","type":"local_agent","status":"running"}
        ],
        "session_crons":[]
    });
    assert!(matches!(
        parse(Provider::ClaudeCode, &agent_beside_shell).unwrap(),
        Parsed::Final { text, .. } if text == "Waiting for reviewers"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn held_append_lock_keeps_hooks_durable_and_replays_before_submission_boundary() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-spool-held-lock");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::Cursor,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        let lease = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(dir.join("append.lock"))
            .unwrap();
        lease.lock().unwrap();
        let value = json!({"hook_event_name":"afterAgentResponse","conversation_id":"s","generation_id":"before","text":"final"});
        for _ in 0..2 {
            append(&dir, "launch", Provider::Cursor, value.clone(), &[], false)
                .expect("a held lock must defer publication, not discard the hook");
        }
        let pending: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e.file_name().to_string_lossy().starts_with("pending-"))
            .collect();
        assert_eq!(
            pending.len(),
            2,
            "both attempts must survive their hook process"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(pending[0].path()).unwrap())
                .unwrap()["value"],
            value
        );
        assert!(
            records(&dir).unwrap().is_empty(),
            "polling must not wait for a held append lock"
        );
        drop(lease);
        // No writer is left to retry: the boundary must recover both envelopes
        // and deduplicate them before fencing a new submission.
        assert_eq!(boundary(&dir).unwrap(), 1);
        let captured = records(&dir).unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].1.value, value);
        assert_eq!(captured[0].1.sequence, 1);
        append(&dir, "launch", Provider::Cursor,
            json!({"hook_event_name":"afterAgentResponse","conversation_id":"s","generation_id":"after","text":"new final"}), &[], false).unwrap();
        assert_eq!(records(&dir).unwrap()[1].1.sequence, 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn polling_replays_exited_writer_and_retries_failed_publication() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-spool-retry");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::Cursor,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        // A publication IO failure happens after staging; removing the fault
        // lets the normal consumer recover without another callback invocation.
        std::fs::create_dir(dir.join("sequence")).unwrap();
        let value = json!({"hook_event_name":"afterAgentResponse","conversation_id":"s","generation_id":"t","text":"final"});
        assert!(append(&dir, "launch", Provider::Cursor, value.clone(), &[], false).is_err());
        std::fs::remove_dir(dir.join("sequence")).unwrap();
        let captured = records(&dir).unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].1.value, value);
        assert_eq!(boundary(&dir).unwrap(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staged_hooks_with_equal_capture_milliseconds_follow_file_write_order() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-spool-write-order");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::Cursor,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        let lease =
            crate::agents::providers::spool::try_append_lock(&dir.join("append.lock")).unwrap();
        for text in ["first", "second"] {
            append(&dir, "launch", Provider::Cursor,
                json!({"hook_event_name":"afterAgentResponse","conversation_id":"s","generation_id":text,"text":text}), &[], false).unwrap();
        }
        let pending: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("pending-"))
            .collect();
        for entry in pending {
            let mut record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
            let first = record["value"]["text"] == "first";
            record["at_ms"] = json!(1_000);
            std::fs::write(entry.path(), serde_json::to_vec(&record).unwrap()).unwrap();
            let path = dir.join(if first {
                "pending-z.json"
            } else {
                "pending-a.json"
            });
            std::fs::rename(entry.path(), &path).unwrap();
            // Explicit timestamps avoid a wall-clock race while representing
            // two ordered writes that share a capture millisecond.
            let stamp = std::time::UNIX_EPOCH
                + std::time::Duration::from_nanos(if first {
                    1_000_000_000
                } else {
                    1_000_500_000
                });
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(stamp))
                .unwrap();
        }
        drop(lease);
        let captured = records(&dir).unwrap();
        assert_eq!(
            captured
                .iter()
                .map(|(_, record)| record.value["text"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(boundary(&dir).unwrap(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_requires_real_turn_binding_and_rejects_notify() {
        assert!(parse(
            Provider::Codex,
            &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"same","transcript_path":"/tmp/interactive.jsonl"})
        )
        .is_err());
        assert!(parse(
            Provider::Codex,
            &json!({"type":"agent-turn-complete","turn-id":"old","input-messages":["same"]})
        )
        .is_err());
        assert_eq!(parse(Provider::Codex, &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","turn_id":"t","prompt":"same","transcript_path":"/tmp/interactive.jsonl"})).unwrap(), Parsed::Started {session:"s".into(),turn:"t".into(),prompt:"same".into()});
    }

    #[test]
    fn claude_long_paste_framing_is_unwrapped_to_the_submitted_text() {
        let text = "Reply with just: pong\n\n- one\n- two\n\nDone.";
        let started = |prompt: String| {
            parse(
                Provider::ClaudeCode,
                &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt_id":"p","prompt":prompt}),
            )
            .unwrap()
        };
        let expected = |prompt: &str| Parsed::Started {
            session: "s".into(),
            turn: "p".into(),
            prompt: prompt.into(),
        };
        // Shape captured from Claude Code 2.1.281 for a 1227-byte Bus paste.
        let framed =
            format!("\n\n<pasted_content id=\"b603\">\n{text}\n</pasted_content id=\"b603\">\n");
        assert_eq!(started(framed), expected(text));
        // Mismatched ids, typed text around the block and short prompts stay verbatim.
        let mismatched = format!("<pasted_content id=\"a1\">\n{text}\n</pasted_content id=\"b2\">");
        assert_eq!(started(mismatched.clone()), expected(&mismatched));
        let typed =
            format!("note\n\n<pasted_content id=\"a1\">\n{text}\n</pasted_content id=\"a1\">");
        assert_eq!(started(typed.clone()), expected(&typed));
        assert_eq!(started(text.into()), expected(text));
        // Codex prompts are never unwrapped.
        let framed = format!("<pasted_content id=\"a1\">\n{text}\n</pasted_content id=\"a1\">");
        assert_eq!(
            parse(Provider::Codex, &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","turn_id":"t","prompt":framed,"transcript_path":"/tmp/i.jsonl"})).unwrap(),
            Parsed::Started { session: "s".into(), turn: "t".into(), prompt: framed.clone() }
        );
    }

    #[test]
    fn claude_requires_prompt_id_and_cursor_generation() {
        assert!(parse(
            Provider::ClaudeCode,
            &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"p"})
        )
        .is_err());
        assert!(parse(
            Provider::Cursor,
            &json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"s","prompt":"p"})
        )
        .is_err());
    }

    #[test]
    fn concurrent_hook_writers_publish_all_records_and_duplicates_are_stable() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-callback");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::Cursor,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        let writers: Vec<_> = (0..12).map(|i| {
            let dir = dir.clone();
            std::thread::spawn(move || append(&dir,"launch",Provider::Cursor,json!({"hook_event_name":"afterAgentResponse","conversation_id":"s","generation_id":format!("t{i}"),"text":"final"}),&[],false).unwrap())
        }).collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let captured = records(&dir).unwrap();
        assert_eq!(captured.len(), 12);
        assert_eq!(boundary(&dir).unwrap(), 12);
        append(
            &dir,
            "launch",
            Provider::Cursor,
            captured[0].1.value.clone(),
            &[],
            false,
        )
        .unwrap();
        assert_eq!(records(&dir).unwrap().len(), 12);
        assert!(append(
            &dir,
            "wrong-launch",
            Provider::Cursor,
            json!({}),
            &[],
            false
        )
        .is_err());
        assert_eq!(
            parse(
                Provider::Cursor,
                &json!({"hook_event_name":"sessionStart","conversation_id":"s"})
            )
            .unwrap(),
            Parsed::Session {
                session: "s".into(),
                source: None
            }
        );
        assert_eq!(
            parse(
                Provider::Codex,
                &json!({"hook_event_name":"SessionStart","session_id":"s","source":"compact","transcript_path":"/tmp/r.jsonl"})
            )
            .unwrap(),
            Parsed::Session {
                session: "s".into(),
                source: Some("compact".into())
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn neutral_spool_preserves_manifest_and_record_wire_fields() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-neutral-spool-wire");
        let manifest = Manifest {
            routing_key: RoutingKey(9),
            provider: Provider::ClaudeCode,
            launch_id: "launch".into(),
        };
        initialize(&dir, &manifest).unwrap();
        let wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            wire,
            json!({"agent_id":9,"provider":"claude_code","launch_id":"launch"})
        );
        let payload = json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"p","last_assistant_message":"reply"});
        append(
            &dir,
            "launch",
            Provider::ClaudeCode,
            payload.clone(),
            &[],
            false,
        )
        .unwrap();
        let captured = records(&dir).unwrap();
        assert_eq!(captured.len(), 1);
        let wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&captured[0].0).unwrap()).unwrap();
        assert_eq!(
            wire["manifest"],
            json!({"agent_id":9,"provider":"claude_code","launch_id":"launch"})
        );
        assert_eq!(wire["sequence"], 1);
        assert_eq!(wire["value"], payload);
        assert_eq!(wire.as_object().unwrap().len(), 5);
        assert_eq!(
            wire["id"],
            bus_io::digest(&serde_json::to_vec(&("launch", &payload)).unwrap())
        );
        assert!(wire["at_ms"].as_u64().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn session_start_records_from_different_processes_are_kept_apart() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-spool-reporter");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::ClaudeCode,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        let old = [crate::platform::ProcessInstance { pid: 100, birth: 1 }];
        let new = [crate::platform::ProcessInstance { pid: 200, birth: 2 }];
        // A restart that resumes the same conversation sends identical bytes; while
        // the exited process's record is still pending, it must not swallow this one.
        let start = json!({"hook_event_name":"SessionStart","session_id":"s","source":"resume"});
        append(
            &dir,
            "launch",
            Provider::ClaudeCode,
            start.clone(),
            &old,
            false,
        )
        .unwrap();
        append(
            &dir,
            "launch",
            Provider::ClaudeCode,
            start.clone(),
            &old,
            false,
        )
        .unwrap();
        append(&dir, "launch", Provider::ClaudeCode, start, &new, false).unwrap();
        // Other callbacks keep the existing one-record-per-payload dedupe.
        let stop = json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"p","last_assistant_message":"reply"});
        append(
            &dir,
            "launch",
            Provider::ClaudeCode,
            stop.clone(),
            &old,
            false,
        )
        .unwrap();
        append(&dir, "launch", Provider::ClaudeCode, stop, &new, false).unwrap();
        let captured = records(&dir).unwrap();
        let reporters: Vec<_> = captured
            .iter()
            .map(|(_, record)| record.reporter.clone())
            .collect();
        assert_eq!(reporters, vec![old.to_vec(), new.to_vec(), old.to_vec()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_record_spooled_before_reporters_existed_still_parses() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-spool-legacy-record");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::Codex,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        let legacy = json!({
            "id":"legacy","sequence":1,"at_ms":1,
            "manifest":{"agent_id":1,"provider":"codex","launch_id":"launch"},
            "value":{"hook_event_name":"SessionStart","session_id":"s","source":"startup","transcript_path":"/tmp/r.jsonl"}
        });
        std::fs::write(
            dir.join("event-legacy.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let captured = records(&dir).unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].1.reporter.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn neutral_spool_rejects_provider_mismatch_and_sequence_overflow_without_events() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-neutral-spool-overflow");
        initialize(
            &dir,
            &Manifest {
                routing_key: RoutingKey(1),
                provider: Provider::Cursor,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        assert!(append(&dir, "launch", Provider::Codex, json!({}), &[], false).is_err());
        assert_eq!(boundary(&dir).unwrap(), 0);
        std::fs::write(dir.join("sequence"), u64::MAX.to_string()).unwrap();
        assert!(append(&dir, "launch", Provider::Cursor, json!({}), &[], false).is_err());
        // Overflow cannot publish, but the hook's durable input is retained and
        // the boundary fails closed instead of allowing a new submission.
        assert!(boundary(&dir).is_err());
        assert!(records(&dir).is_err());
        assert!(!std::fs::read_dir(&dir)
            .unwrap()
            .map(Result::unwrap)
            .any(|e| e.file_name().to_string_lossy().starts_with("event-")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn neutral_spool_keeps_private_modes_and_refuses_manifest_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::utils::test_temp::unique_temp_path("bus-neutral-spool-permissions");
        let manifest = Manifest {
            routing_key: RoutingKey(1),
            provider: Provider::Cursor,
            launch_id: "launch".into(),
        };
        initialize(&dir, &manifest).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let path = dir.join("manifest.json");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), &path).unwrap();
        assert!(initialize(&dir, &manifest).is_err());
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
