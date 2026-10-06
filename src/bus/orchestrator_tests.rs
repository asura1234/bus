use super::*;

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("bus-orch-{label}-{}", crate::bus::io::now_ns()))
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
        WORKFLOW_CREATE,
        std::fs::read_to_string(repo.join("skills/workflow-create/SKILL.md")).unwrap()
    );
    for (name, text) in DOCS {
        assert_eq!(
            *text,
            std::fs::read_to_string(repo.join("docs").join(name)).unwrap(),
            "{name}"
        );
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
    let unassigned = fill(DEFAULT_PROMPT, &values(None, data));
    assert!(unassigned.contains("Your room: none yet"), "{unassigned}");
}

#[cfg(unix)]
#[test]
fn docs_are_written_owner_only_with_workflow_create_as_a_plain_doc() {
    use std::os::unix::fs::PermissionsExt;
    let data = temp_root("docs");
    let root = write_docs(&data).unwrap();
    assert_eq!(root, data.join("docs"));
    for (name, _) in DOCS {
        assert!(root.join(name).is_file(), "{name}");
    }
    let guide = std::fs::read_to_string(root.join("workflow-create.md")).unwrap();
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
    let path = write_prompt(&spool, "Line \"one\".\nLine two.", false).unwrap();
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
    // The value parses as a TOML string back to the exact prompt.
    let parsed: toml::Value = toml::from_str(&format!("v = {value}")).unwrap();
    assert_eq!(parsed["v"].as_str(), Some("Line \"one\".\nLine two."));
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
    let path = write_prompt(&spool, "Run pr-1.", true).unwrap();
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
fn messages_name_the_new_room_or_its_absence() {
    let moved = reassigned_message("orch", Some(("pr-2", RoomId(2))));
    assert!(moved.contains("room pr-2 (id 2)"), "{moved}");
    assert!(moved.contains("bus send --room 2 --as orch"), "{moved}");
    assert!(reassigned_message("orch", None).contains("no longer orchestrate"));
    assert!(prompt_message("Be brief.").ends_with("\n\nBe brief."));
}
