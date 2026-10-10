use super::*;

fn temp_root(label: &str) -> PathBuf {
    crate::utils::test_temp::unique_temp_path(&format!("bus-orch-{label}"))
}

fn values(room: Option<(&str, u64)>, data: &Path) -> PromptValues {
    PromptValues {
        room: room.map(|(name, id)| (name.into(), RoomId(id))),
        agent: "orch".into(),
        docs: docs_dir(data),
    }
}

#[test]
fn embedded_docs_match_the_repository_copies() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        DEFAULT_PROMPT,
        std::fs::read_to_string(repo.join("orchestration/prompt.md")).unwrap()
    );
    assert_eq!(
        WORKFLOW_CREATE,
        std::fs::read_to_string(repo.join("workflows/create.md")).unwrap()
    );
    let sources = [
        ("how-to-bus-cli.md", "orchestration/how-to-bus-cli.md"),
        ("orchestrator-guide.md", "orchestration/guide.md"),
        ("orchestrator-rules.md", "orchestration/rules.md"),
        ("templates/workflow-template.md", "workflows/template.md"),
        ("workflows/auto-merge-pr.md", "workflows/auto-merge-pr.md"),
        (
            "workflows/cross-repo-feature.md",
            "workflows/cross-repo-feature.md",
        ),
    ];
    assert_eq!(DOCS.len(), sources.len());
    for ((name, text), (expected_name, source)) in DOCS.iter().zip(sources) {
        assert_eq!(*name, expected_name);
        assert_eq!(
            *text,
            std::fs::read_to_string(repo.join(source)).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn compaction_notice_instructions_replace_the_worker_from_a_temp_handover_note() {
    let rules = DOCS
        .iter()
        .find(|(name, _)| *name == "orchestrator-rules.md")
        .unwrap()
        .1;
    for text in [DEFAULT_PROMPT, rules] {
        for phrase in [
            "reached N compactions",
            "temp/",
            "same provider and role",
            "brief it from the note",
            "delete the old agent",
        ] {
            assert!(text.contains(phrase), "missing {phrase}: {text}");
        }
        assert!(!text.contains("At most 5 compactions"));
        assert!(!text.contains("After 5"));
    }
}

#[test]
fn fill_replaces_every_placeholder() {
    let data = Path::new("/data");
    let text = fill(DEFAULT_PROMPT, &values(Some(("pr-123", 7)), data));
    assert!(!text.contains("{{"), "{text}");
    assert!(text.contains("Your room: pr-123 (id 7). Your agent name: orch."));
    assert!(text.contains("bus send --room 7 --as orch"));
    assert!(text.contains("/data/docs/workflow-create.md"));
    assert!(text.contains("/data/docs/how-to-bus-cli.md"));
    assert!(text.contains("Orchestrator rules (binding): /data/docs/orchestrator-rules.md"));
    assert!(text.contains("The rules in /data/docs/orchestrator-rules.md are binding"));
    // Reports reach MASTER as the turn's final message; Bus posts it there.
    assert!(text.contains("Bus shows it in MASTER"), "{text}");
    assert!(!text.contains("--to human"), "{text}");
    assert!(text.contains("Parallelism is king"), "{text}");
    assert!(text.contains("Compartmentalize, within reason"), "{text}");
    assert!(text.contains("Size the team to the current step"), "{text}");
    assert!(text.contains("Brief workers directly"), "{text}");
    // Tasks go out with --async in the background, never a blocking wait.
    assert!(text.contains("bus send --room 7 --as orch --to AGENT --async --text"));
    assert!(text.contains("run_in_background"));
    assert!(!text.contains("bus wait"));
    let unassigned = fill(DEFAULT_PROMPT, &values(None, data));
    assert!(unassigned.contains("Your room: none yet"), "{unassigned}");
}

#[test]
fn compiled_default_rejects_unknown_or_unfinished_placeholders() {
    assert!(supported_prompt_placeholders(DEFAULT_PROMPT));
    assert!(supported_prompt_placeholders("Plain text with {braces}."));
    assert!(supported_prompt_placeholders(
        "{{DOCS}}{{ROOM_ID}}{{AGENT_NAME}}{{ROOM_NAME}}"
    ));
    for invalid in [
        "{{UNKNOWN}}",
        "{{ROOM_ID",
        "{{",
        "{{DOCS}} then {{MISSING}}",
    ] {
        assert!(!supported_prompt_placeholders(invalid), "{invalid}");
    }
}

#[test]
fn embedded_text_is_written_unchanged_at_arbitrary_output_locations() {
    let data = temp_root("relocated-output");
    let authored = DOCS
        .iter()
        .copied()
        .chain([("workflow-create.md", WORKFLOW_CREATE)])
        .collect::<Vec<_>>();
    // No process-wide cwd mutation: these unrelated output trees exercise the
    // compiled bytes without requiring a checkout beside either destination.
    for destination in [
        data.join("working-directory/session"),
        data.join("installed/bin/session"),
    ] {
        let docs = write_docs(&destination).unwrap();
        for (name, text) in &authored {
            assert_eq!(
                std::fs::read(docs.join(name)).unwrap(),
                text.as_bytes(),
                "{name}"
            );
        }
        let prompt = fill(DEFAULT_PROMPT, &values(Some(("pr-123", 7)), &destination));
        assert!(!prompt.contains("{{"));
        let spool = destination.join("callbacks/launch");
        private_dir(&spool).unwrap();
        let path = write_prompt(&spool, &prompt).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), prompt.as_bytes());
    }
    std::fs::remove_dir_all(data).unwrap();
}

#[cfg(unix)]
#[test]
fn docs_are_written_owner_only_with_workflow_create_as_a_plain_doc() {
    use std::os::unix::fs::PermissionsExt;
    let data = temp_root("docs");
    let root = write_docs(&data).unwrap();
    assert_eq!(root, data.join("docs"));
    for (name, text) in DOCS {
        assert_eq!(
            std::fs::read_to_string(root.join(name)).unwrap(),
            *text,
            "{name}"
        );
    }
    let rules = std::fs::read_to_string(root.join("orchestrator-rules.md")).unwrap();
    assert!(rules.starts_with("# Orchestrator rules"), "{rules}");
    assert!(rules.contains("End each turn with your report; Bus shows it in MASTER."));
    assert!(!rules.contains("--to human"), "{rules}");
    assert!(rules.contains("**Parallelism is king.**"));
    assert!(rules.contains("**Compartmentalize, within reason.**"));
    assert!(rules.contains("**Size the team to the current step.**"));
    assert!(rules.contains("**Brief workers directly.**"));
    assert!(
        rules.contains("**Keep follow-ups in the running workflow file, not in your context.**")
    );
    assert!(rules.contains("bus send --room ROOM --as YOUR_NAME --to AGENT --async"));
    let guide = std::fs::read_to_string(root.join("workflow-create.md")).unwrap();
    assert_eq!(guide, WORKFLOW_CREATE);
    assert!(guide.starts_with("# workflow-create"), "{guide}");
    assert!(!guide.contains("description:"));
    let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn each_provider_gets_its_own_prompt_delivery() {
    let spool = temp_root("args");
    private_dir(&spool).unwrap();
    let path = write_prompt(&spool, "Line \"one\".\nLine two.").unwrap();
    assert_eq!(path, spool.join(PROMPT_FILE));

    assert_eq!(
        prompt_args(Provider::ClaudeCode, &path, false).unwrap(),
        Some(vec![
            "--append-system-prompt-file".into(),
            path.to_string_lossy().into_owned()
        ])
    );
    let codex = prompt_args(Provider::Codex, &path, false).unwrap().unwrap();
    assert_eq!(codex[0], "-c");
    let value = codex[1].strip_prefix("developer_instructions=").unwrap();
    assert!(!value.contains('\n'), "one typed line: {value}");
    // The value parses as a TOML string that points at the prompt file, so the
    // typed launch command stays short however long the prompt is.
    let parsed: toml::Value = toml::from_str(&format!("v = {value}")).unwrap();
    let instructions = parsed["v"].as_str().unwrap();
    assert!(
        instructions.contains(&*path.to_string_lossy()),
        "{instructions}"
    );
    let long = write_prompt(&spool, &"Orchestrate the room. ".repeat(400)).unwrap();
    let long = prompt_args(Provider::Codex, &long, false).unwrap().unwrap();
    assert!(long[1].len() < 400, "{} bytes typed", long[1].len());
    assert_eq!(prompt_args(Provider::Cursor, &path, false).unwrap(), None);

    assert_eq!(resume_prompt_args(Provider::Codex, &spool).unwrap(), codex);
    assert!(resume_prompt_args(Provider::Cursor, &spool)
        .unwrap()
        .is_empty());
    std::fs::remove_file(&path).unwrap();
    assert!(resume_prompt_args(Provider::ClaudeCode, &spool)
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(spool).unwrap();
}

#[test]
fn an_adopted_session_renders_the_prompt_fresh_or_gets_it_as_a_message() {
    let spool = temp_root("adopted");
    private_dir(&spool).unwrap();
    let path = write_prompt(&spool, "Run pr-1.").unwrap();
    std::fs::write(
        spool.join("adopted-session"),
        "160d1f8b-9023-44b8-9bc7-24333effb185",
    )
    .unwrap();
    let claude = vec![
        "--system-prompt-snapshot".to_owned(),
        "off".into(),
        "--append-system-prompt-file".into(),
        path.to_string_lossy().into_owned(),
    ];
    assert_eq!(
        prompt_args(Provider::ClaudeCode, &path, true).unwrap(),
        Some(claude.clone())
    );
    assert_eq!(prompt_args(Provider::Codex, &path, true).unwrap(), None);
    assert_eq!(prompt_args(Provider::Cursor, &path, true).unwrap(), None);
    // Resumes after a restart keep rendering it fresh.
    assert_eq!(
        resume_prompt_args(Provider::ClaudeCode, &spool).unwrap(),
        claude
    );
    assert!(resume_prompt_args(Provider::Codex, &spool)
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(spool).unwrap();
}

#[test]
fn prompt_message_carries_the_prompt_text() {
    assert!(prompt_message("Be brief.").ends_with("\n\nBe brief."));
}
