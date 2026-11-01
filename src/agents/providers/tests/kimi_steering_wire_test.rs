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
