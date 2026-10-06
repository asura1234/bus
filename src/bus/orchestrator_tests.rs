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
fn embedded_skill_and_docs_match_the_repository_copies() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        WORKFLOW_CREATE_SKILL,
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
    assert!(text.contains("/data/docs/how-to-bus-cli.md"));
    let unassigned = fill(DEFAULT_PROMPT, &values(None, data));
    assert!(unassigned.contains("Your room: none yet"), "{unassigned}");
}

#[test]
fn default_folder_is_under_the_data_dir_and_names_are_single_folders() {
    assert_eq!(
        default_folder(Path::new("/data"), " orch "),
        Path::new("/data/orchestrators/orch")
    );
    assert!(check_folder_name("claude-orch").is_ok());
    for bad in ["", "..", ".", "a/b", "a\\b"] {
        assert!(check_folder_name(bad).is_err(), "{bad}");
    }
}

#[cfg(unix)]
#[test]
fn creation_writes_prompts_skills_and_docs_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let data = temp_root("create");
    private_dir(&data).unwrap();
    let folder = default_folder(&data, "orch");
    ensure_folder(&data, &folder, "orch").unwrap();
    let values = values(Some(("pr-123", 7)), &data);
    write_folder(&data, &folder, None, &values).unwrap();
    write_docs(&data).unwrap();

    let mode = std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let expected = fill(DEFAULT_PROMPT, &values);
    for file in ["CLAUDE.md", "AGENTS.md"] {
        assert_eq!(
            std::fs::read_to_string(folder.join(file)).unwrap(),
            expected
        );
    }
    for dir in [".claude/skills", ".agents/skills"] {
        let skill = folder.join(dir).join("workflow-create/SKILL.md");
        assert_eq!(
            std::fs::read_to_string(skill).unwrap(),
            WORKFLOW_CREATE_SKILL
        );
    }
    let mut entries: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, [".agents", ".claude", "AGENTS.md", "CLAUDE.md"]);
    for (name, _) in DOCS {
        assert!(docs_dir(&data).join(name).is_file(), "{name}");
    }
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn custom_prompts_fill_placeholders_or_stay_verbatim() {
    let data = temp_root("custom");
    let folder = data.join("custom");
    private_dir(&folder).unwrap();
    let values = values(Some(("pr-123", 7)), &data);
    write_folder(
        &data,
        &folder,
        Some("Run {{ROOM_NAME}} as {{AGENT_NAME}}."),
        &values,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(folder.join("AGENTS.md")).unwrap(),
        "Run pr-123 as orch."
    );
    write_folder(&data, &folder, Some("Plain text."), &values).unwrap();
    assert_eq!(
        std::fs::read_to_string(folder.join("CLAUDE.md")).unwrap(),
        "Plain text."
    );
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn creation_refuses_to_replace_files_bus_did_not_write() {
    let data = temp_root("refuse");
    let folder = data.join("repo");
    private_dir(&folder).unwrap();
    std::fs::write(folder.join("CLAUDE.md"), "the team's file").unwrap();
    let error = write_folder(&data, &folder, None, &values(None, &data)).unwrap_err();
    assert!(error.contains("not written by Bus"), "{error}");
    assert_eq!(
        std::fs::read_to_string(folder.join("CLAUDE.md")).unwrap(),
        "the team's file"
    );
    assert!(!folder.join("AGENTS.md").exists());
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn a_recreated_orchestrator_reuses_its_bus_written_folder() {
    let data = temp_root("reuse");
    let folder = default_folder(&data, "orch");
    ensure_folder(&data, &folder, "orch").unwrap();
    write_folder(&data, &folder, None, &values(None, &data)).unwrap();
    write_folder(&data, &folder, None, &values(Some(("b", 2)), &data)).unwrap();
    assert!(std::fs::read_to_string(folder.join("CLAUDE.md"))
        .unwrap()
        .contains("Your room: b (id 2)"));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn reassign_rewrites_only_files_the_human_has_not_edited() {
    let data = temp_root("retarget");
    let folder = default_folder(&data, "orch");
    ensure_folder(&data, &folder, "orch").unwrap();
    // A TUI submission: the default prompt already filled in.
    let first = values(Some(("pr-1", 1)), &data);
    write_folder(&data, &folder, Some(&fill(DEFAULT_PROMPT, &first)), &first).unwrap();
    std::fs::write(folder.join("AGENTS.md"), "my own rules").unwrap();

    let outcome = retarget(&data, &folder, Some(("pr-2".into(), RoomId(2)))).unwrap();
    assert_eq!(outcome.rewritten, [folder.join("CLAUDE.md")]);
    assert_eq!(outcome.kept, [folder.join("AGENTS.md")]);
    assert!(outcome.notice().unwrap().contains("AGENTS.md"));
    assert!(std::fs::read_to_string(folder.join("CLAUDE.md"))
        .unwrap()
        .contains("Your room: pr-2 (id 2)"));
    assert_eq!(
        std::fs::read_to_string(folder.join("AGENTS.md")).unwrap(),
        "my own rules"
    );

    // Still unedited after the rewrite, so a later reassign rewrites it again.
    let outcome = retarget(&data, &folder, None).unwrap();
    assert_eq!(outcome.rewritten, [folder.join("CLAUDE.md")]);
    assert!(std::fs::read_to_string(folder.join("CLAUDE.md"))
        .unwrap()
        .contains("Your room: none yet"));
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn reassign_keeps_a_custom_prompt_and_ignores_folders_bus_never_wrote() {
    let data = temp_root("retarget-custom");
    let folder = data.join("custom");
    private_dir(&folder).unwrap();
    assert_eq!(retarget(&data, &folder, None).unwrap(), Retarget::default());
    write_folder(&data, &folder, Some("Plain text."), &values(None, &data)).unwrap();
    let outcome = retarget(&data, &folder, Some(("pr-2".into(), RoomId(2)))).unwrap();
    assert!(outcome.rewritten.is_empty());
    assert_eq!(outcome.kept.len(), 2);
    assert_eq!(
        std::fs::read_to_string(folder.join("CLAUDE.md")).unwrap(),
        "Plain text."
    );
    std::fs::remove_dir_all(data).unwrap();
}
