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

const ITERATIONS: &str =
    include_str!("../../../../tests/fixtures/kimi-steer/thinking-iterations-2.4.1.jsonl");

#[test]
fn kimi_inner_agent_iterations_keep_the_last_outer_prompt_final() {
    let rows: Vec<Value> = ITERATIONS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let lead = rows[0]["input"][0]["text"].as_str().unwrap();
    let guidance = rows
        .iter()
        .find(|row| row["type"] == "prompt.steered")
        .unwrap()["content"][0]["text"]
        .as_str()
        .unwrap();
    // Real 2.4.1 wire: outer prompt 1 contains agent iterations 1 and 2.
    for prompt in [lead, guidance] {
        assert_eq!(
            committed_final(ITERATIONS, 1, prompt).as_deref(),
            Some("N265_THINK_GUIDED_13631")
        );
    }
}

#[test]
fn kimi_next_outer_prompt_uses_its_own_internal_iteration_counter() {
    for inner in [2, 3, 40] {
        let rows = [
            serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":2,"promptId":"next","input":[{"type":"text","text":"next task"}]}),
            serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"text","text":"next final"}]}}}),
            serde_json::json!({"type":"agent.turn.ended","turnId":inner,"outcome":"done"}),
            serde_json::json!({"type":"turn.ended","agentId":"main","turnId":2,"reason":"completed"}),
            serde_json::json!({"type":"prompt.completed","agentId":"main","promptId":"next","reason":"completed"}),
        ];
        let wire = format!(
            "{ITERATIONS}{}",
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>()
        );
        assert_eq!(
            committed_final(&wire, 2, "next task").as_deref(),
            Some("next final"),
            "internal iteration {inner}"
        );
    }
}

#[test]
fn kimi_latest_internal_failure_and_incomplete_outer_commit_are_not_an_old_final() {
    let mut rows: Vec<Value> = ITERATIONS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let lead = rows[0]["input"][0]["text"].as_str().unwrap().to_owned();
    let open = rows
        .iter()
        .filter(|row| row["type"] != "turn.ended" && row["type"] != "prompt.completed")
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    assert_eq!(
        committed_outcome(&open, 1, &lead),
        None,
        "internal iterations cannot commit an open outer prompt"
    );
    let latest = rows
        .iter_mut()
        .rev()
        .find(|row| row["type"] == "agent.turn.ended")
        .unwrap();
    latest["outcome"] = serde_json::json!("failed");
    latest["errorMessage"] = serde_json::json!("later native iteration failed");
    for row in rows
        .iter_mut()
        .filter(|row| row["type"] == "turn.ended" || row["type"] == "prompt.completed")
    {
        row["reason"] = serde_json::json!("failed");
    }
    let failed = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    assert_eq!(
        committed_outcome(&failed, 1, &lead),
        Some(Outcome::Failed("later native iteration failed".into()))
    );
}

#[test]
fn kimi_silent_internal_iteration_does_not_reuse_an_earlier_answer() {
    let mut rows: Vec<Value> = ITERATIONS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let lead = rows[0]["input"][0]["text"].as_str().unwrap().to_owned();
    rows.iter_mut()
        .rev()
        .find(|row| row["type"] == "agent.message.appended")
        .unwrap()["message"]["message"]["content"] = serde_json::json!([]);
    let wire = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    assert_eq!(committed_outcome(&wire, 1, &lead), Some(Outcome::NoReply));
}

#[test]
fn kimi_latest_internal_output_requires_its_own_iteration_end_before_outer_commit() {
    let mut rows: Vec<Value> = ITERATIONS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let lead = rows[0]["input"][0]["text"].as_str().unwrap().to_owned();
    let last_end = rows
        .iter()
        .rposition(|row| row["type"] == "agent.turn.ended")
        .unwrap();
    rows.remove(last_end);
    let wire = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    assert_eq!(
        committed_outcome(&wire, 1, &lead),
        None,
        "an earlier iteration cannot attest later output"
    );
}

#[test]
fn kimi_guidance_between_internal_iterations_remains_owned_until_outer_commit() {
    let mut rows: Vec<Value> = ITERATIONS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let position = rows
        .iter()
        .position(|row| row["type"] == "prompt.steered")
        .unwrap();
    let guidance = rows.remove(position);
    let prompt = guidance["content"][0]["text"].as_str().unwrap().to_owned();
    let position = rows
        .iter()
        .position(|row| row["type"] == "agent.turn.ended")
        .unwrap()
        + 1;
    rows.insert(position, guidance);
    let wire = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    assert_eq!(
        committed_final(&wire, 1, &prompt).as_deref(),
        Some("N265_THINK_GUIDED_13631")
    );
}

#[test]
fn kimi_think_only_message_after_text_keeps_the_same_iteration_answer() {
    let mut rows: Vec<Value> = ITERATIONS
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let lead = rows[0]["input"][0]["text"].as_str().unwrap().to_owned();
    let position = rows
        .iter()
        .rposition(|row| row["type"] == "agent.message.appended")
        .unwrap()
        + 1;
    rows.insert(position, serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"think","think":"same iteration reasoning fragment"}]}}}));
    let wire = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    assert_eq!(
        committed_final(&wire, 1, &lead).as_deref(),
        Some("N265_THINK_GUIDED_13631")
    );
}
