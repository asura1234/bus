use super::*;

const WIRE: &str = include_str!("../../../../tests/fixtures/kimi-steer/steered.jsonl");
const GUIDANCE: &str = "Include NATIVE_GUIDANCE_MARKER in the final response.Include NATIVE_GUIDANCE_COMMAND in your final response.";

#[test]
fn kimi_committed_wire_attests_guidance_in_the_original_turn() {
    let final_text = committed_final(WIRE, 3, GUIDANCE)
        .expect("the captured prompt.steered belongs to this completed turn");
    assert!(final_text.contains("NATIVE_GUIDANCE_MARKER"));
    assert!(final_text.contains("NATIVE_GUIDANCE_COMMAND"));
}

#[test]
fn kimi_guidance_requires_matching_active_prompt_agent_and_complete_commit() {
    for foreign in [
        WIRE.replace(
            "\"activePromptId\": \"msg_01M6ARJYA0CDJE5YGG4V0SCDJ1\"",
            "\"activePromptId\": \"other\"",
        ),
        WIRE.replace(
            "\"agentId\": \"main\", \"activePromptId\"",
            "\"agentId\": \"other\", \"activePromptId\"",
        ),
        WIRE.lines()
            .filter(|line| !line.contains("prompt.completed"))
            .collect::<Vec<_>>()
            .join("\n"),
    ] {
        assert_ne!(foreign, WIRE);
        assert_eq!(committed_final(&foreign, 3, GUIDANCE), None);
    }
    assert_eq!(committed_final(WIRE, 3, "unsubmitted guidance"), None);
    assert_eq!(committed_final(WIRE, 4, GUIDANCE), None);
}

#[test]
fn kimi_batched_wire_keeps_exact_text_boundaries_and_rejects_ambiguous_ids() {
    let first = "Include NATIVE_GUIDANCE_MARKER in the final response.";
    let second = "Include NATIVE_GUIDANCE_COMMAND in your final response.";
    for case in 0..5 {
        let mut rows: Vec<Value> = WIRE
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let guidance = rows
            .iter_mut()
            .find(|row| row["type"] == "prompt.steered")
            .unwrap();
        guidance["messageId"] = serde_json::json!("batch");
        guidance["promptIds"] = serde_json::json!(["first", "second"]);
        guidance["content"] = serde_json::json!([
            {"type":"text", "text":first},
            {"type":"text", "text":second}
        ]);
        match case {
            1 => guidance["promptIds"] = serde_json::json!(["first", "first"]),
            2 => guidance["content"][1] = serde_json::json!({"type":"image", "text":second}),
            3 => guidance["content"][1] = serde_json::json!({"type":"text"}),
            4 => guidance["content"][1]["text"] = serde_json::json!(""),
            _ => {}
        }
        let wire: String = rows.into_iter().map(|row| format!("{row}\n")).collect();
        for prompt in [first, second] {
            assert_eq!(
                committed_final(&wire, 3, prompt).is_some(),
                case == 0,
                "case {case}: {prompt}"
            );
        }
        assert_eq!(
            committed_final(&wire, 3, GUIDANCE),
            None,
            "separate native parts never attest a concatenated request"
        );
    }
}

#[test]
fn kimi_stale_open_turn_and_background_notification_do_not_steal_the_committed_final() {
    let old = serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":2,"promptId":"old","input":[{"type":"text","text":"old unanswered task"}]});
    let notify = serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":4,"promptId":"notification","input":[{"type":"text","text":"<notification id=\"task:bash-1:completed\" />"}]});
    let later = serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"text","text":"foreign continuation"}]}}});
    let wire = format!("{old}\n{WIRE}{notify}\n{later}\n");
    assert_eq!(
        committed_final(&wire, 3, GUIDANCE),
        committed_final(WIRE, 3, GUIDANCE)
    );
    assert_eq!(committed_final(&wire, 2, "old unanswered task"), None);
    assert_eq!(committed_final(&wire, 4, GUIDANCE), None);
}

#[test]
fn kimi_synchronous_and_delayed_tools_share_only_their_attested_native_final() {
    for background in [false, true] {
        let mut rows: Vec<Value> = WIRE
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let tool = serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"think","think":"tool"}],"toolCalls":[{"id":"bash-1","name":"Bash","arguments":{"command":"sleep 45","run_in_background":background}}]}}});
        let result = serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"tool"},"message":{"role":"tool","toolCallId":"bash-1","content":[{"type":"text","text": if background {"task_id: bash-1\nstatus: running\nautomatic_notification: true"} else {"/tmp/project\n"}}]}}});
        let position = if background { 2 } else { 1 };
        rows.splice(position..position, [tool, result]);
        let open: String = rows[..rows.len() - 3]
            .iter()
            .map(|row| format!("{row}\n"))
            .collect();
        assert_eq!(
            committed_final(&open, 3, GUIDANCE),
            None,
            "tool result and assistant text alone never commit"
        );
        let complete: String = rows.iter().map(|row| format!("{row}\n")).collect();
        assert_eq!(
            committed_final(&complete, 3, GUIDANCE),
            committed_final(WIRE, 3, GUIDANCE)
        );
    }
}
