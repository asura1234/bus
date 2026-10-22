//! Synthetic prompt/installation contracts; no Grok process or Bus session is started.
use super::{orchestrator_prompt, prompt_args, prompt_message};
use crate::messaging::{
    model::{Provider, RoomId},
    orchestration::{self, PromptValues},
};
use std::path::Path;

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn grok_e_fresh_and_adopted_sessions_bootstrap_without_launch_flags_or_file_io() {
    let path = Path::new("/not-created/grok e/it's system prompt.md");
    for adopted in [false, true] {
        assert_eq!(prompt_args(path, adopted).unwrap(), None);
        assert_eq!(
            orchestration::prompt_args(Provider::Grok, path, adopted).unwrap(),
            None
        );
    }
}

#[test]
fn grok_e_custom_instructions_survive_verbatim_with_grok_protocol_precedence() {
    let authored = "Keep 中文 and whitespace.

  Custom task: quote `--async` literally.
";
    let prompt = orchestrator_prompt(authored).unwrap();
    assert!(prompt.ends_with(authored));
    let protocol = prompt.strip_suffix(authored).unwrap();
    assert!(flat(protocol)
        .contains("take precedence over conflicting dispatch and waiting instructions"));
    assert!(flat(protocol).contains("including linked shared Bus documentation"));
    assert!(protocol.contains("bus send --room R --as ORCH --to WORKER --text"));
}

#[test]
fn grok_e_fill_values_are_preserved_without_expanding_inserted_placeholders() {
    let template = "Room {{ROOM_NAME}} ({{ROOM_ID}}); agent {{AGENT_NAME}}.
Docs {{DOCS}}; workflows {{WORKFLOWS_DIR}}.";
    let authored = orchestration::fill(
        template,
        &PromptValues {
            room: Some(("literal {{DOCS}} room".into(), RoomId(568))),
            agent: "orch".into(),
            docs: "/synthetic/grok docs".into(),
        },
    );
    let prompt = orchestrator_prompt(&authored).unwrap();
    assert!(prompt.ends_with(&authored));
    assert!(prompt.contains("Room literal {{DOCS}} room (568); agent orch."));
    assert!(prompt.contains("Docs /synthetic/grok docs; workflows "));
    assert_eq!(prompt.matches("{{DOCS}}").count(), 1);
    assert!(!prompt.contains("{{ROOM_ID}}"));
    // The provider must not become another template engine for custom text.
    let literal = "Custom literal {{ROOM_ID}} and {{UNRECOGNIZED}}.";
    assert!(orchestrator_prompt(literal).unwrap().ends_with(literal));
}

#[test]
fn grok_e_default_prompt_retains_room_policy_under_explicit_transport_override() {
    let authored = orchestration::fill(
        orchestration::DEFAULT_PROMPT,
        &PromptValues {
            room: Some(("synthetic work".into(), RoomId(568))),
            agent: "grok-orch".into(),
            docs: "/synthetic/docs".into(),
        },
    );
    let prompt = orchestrator_prompt(&authored).unwrap();
    assert!(prompt.ends_with(&authored));
    let protocol = flat(prompt.strip_suffix(&authored).unwrap());
    assert!(protocol.contains("Do not use `bus send --async`"));
    assert!(protocol.contains("Do not use `bus wait`"));
    assert!(protocol.contains("return to idle"));
    assert!(protocol.contains("Bus-delivered wake"));
    assert!(protocol.contains("worker result or blocker"));
    assert!(prompt.contains("Your room: synthetic work (id 568). Your agent name: grok-orch."));
    assert!(prompt.contains("/synthetic/docs/orchestrator-rules.md"));
}

#[test]
fn grok_e_bootstrap_delivers_full_prompt_and_does_not_replay_work() {
    let authored = "Synthetic task text

Keep this line.
";
    let prompt = orchestrator_prompt(authored).unwrap();
    let message = prompt_message(&prompt);
    assert!(message.ends_with(&prompt));
    let bootstrap = flat(message.strip_suffix(&prompt).unwrap());
    assert!(bootstrap.contains("for the rest of this session"));
    assert!(bootstrap.contains("reply briefly that you are ready"));
    assert!(bootstrap.contains("Do not dispatch or replay worker tasks during this bootstrap"));
}

#[test]
fn grok_e_dispatch_keeps_durable_notifications_and_uncertain_submissions() {
    let prompt = flat(&orchestrator_prompt("Synthetic policy.").unwrap());
    for instruction in [
        "waiter can acknowledge a reply and suppress its Bus-delivered wake",
        "Delivery is not completion",
        "bus message status",
        "bus history --room R",
        "do not resend uncertain work",
        "reconcile durable state",
    ] {
        assert!(prompt.contains(instruction), "missing: {instruction}");
    }
}

#[test]
fn grok_e_dialog_guidance_requires_fresh_fingerprints_and_developer_authority() {
    let prompt = flat(&orchestrator_prompt("Synthetic policy.").unwrap());
    for command in [
        "bus agent dialog WORKER",
        "bus agent read WORKER --source visible",
        "bus agent choose WORKER --option N --fingerprint F",
        r#"bus agent answer WORKER --text "..." --fingerprint F"#,
        "bus agent answer WORKER --skip --fingerprint F",
    ] {
        assert!(prompt.contains(command), "missing: {command}");
    }
    assert!(prompt.contains("fresh fingerprint"));
    assert!(prompt.contains("destructive or outward-facing"));
    assert!(prompt.contains("do not invent an approval"));
}

#[test]
fn grok_e_report_destinations_and_native_evidence_limit_are_explicit() {
    let prompt = flat(&orchestrator_prompt("Synthetic policy.").unwrap());
    for instruction in [
        "final answering a developer message in MASTER belongs in MASTER",
        "autonomous final belongs in the work room",
        "Do not replace an earlier MASTER answer with a later worker wake",
        "bus send --room master --as ORCH --to human --text",
        "Native background-task wake verification is pending because Grok is not signed in",
        "not waived",
        "Synthetic fixtures",
    ] {
        assert!(prompt.contains(instruction), "missing: {instruction}");
    }
}

#[test]
fn grok_e_installed_guide_and_cold_resume_keep_the_same_bootstrap_policy() {
    let root = crate::utils::test_temp::unique_temp_path("grok-e-prompt");
    let docs = orchestration::write_docs(&root).unwrap();
    let guide = std::fs::read_to_string(docs.join("providers/grok.md")).unwrap();
    assert!(guide.contains("bus send --room R --as ORCH --to WORKER --text"));
    assert!(flat(&guide).contains("return to idle"));
    assert!(flat(&guide).contains("not waived"));
    let spool = root.join("callbacks");
    std::fs::create_dir(&spool).unwrap();
    let prompt = orchestrator_prompt("Synthetic persisted room policy.").unwrap();
    let path = orchestration::write_prompt(&spool, &prompt).unwrap();
    assert!(orchestration::resume_prompt_args(Provider::Grok, &spool)
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), prompt);
    std::fs::remove_dir_all(root).unwrap();
}
