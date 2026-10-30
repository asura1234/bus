use super::*;
use crate::messaging::model::BusState;

fn temp_root(label: &str) -> PathBuf {
    crate::utils::test_temp::unique_temp_path(&format!("bus-orch-{label}"))
}

#[test]
fn every_repository_workflow_is_bundled_at_its_original_name() {
    let data = temp_root("all-workflows");
    let docs = write_docs(&data).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("workflows");
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&repo).unwrap() {
        let path = entry.unwrap().path();
        assert!(path.is_file(), "workflow bundle must cover {path:?}");
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        assert_eq!(
            std::fs::read(docs.join("workflows").join(&name))
                .unwrap_or_else(|error| panic!("workflow {name} was not embedded: {error}")),
            std::fs::read(&path).unwrap(),
            "{name}"
        );
        names.push(name);
    }
    let mut bundled = std::fs::read_dir(docs.join("workflows"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    bundled.sort();
    assert_eq!(names, bundled);
    // First launch after an upgrade refreshes stale embedded bytes, too.
    for name in &names {
        std::fs::write(docs.join("workflows").join(name), "old installation").unwrap();
    }
    write_docs(&data).unwrap();
    for name in &names {
        assert_eq!(
            std::fs::read(docs.join("workflows").join(name)).unwrap(),
            std::fs::read(repo.join(name)).unwrap()
        );
    }
    std::fs::remove_dir_all(data).unwrap();
}

/// `text` with every whitespace run as one space, so a pinned phrase may wrap.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn written_cli_guide_points_to_command_help() {
    let data = temp_root("short-cli-guide");
    let docs = write_docs(&data).unwrap();
    let guide = std::fs::read_to_string(docs.join("how-to-bus-cli.md")).unwrap();
    assert!(guide.contains("bus <cmd> --help"));
    assert!(guide.lines().count() <= 250);
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn workflow_discovery_and_assumption_rules_are_binding() {
    let rules = include_str!("../../../orchestration/rules.md");
    assert!(rules[rules.find("- **").unwrap()..]
        .starts_with("- **Never act on assumptions; ask the developer.**"));
    let rules = flat(rules);
    for phrase in [
        "Acting on assumptions",
        "unknown skill, workflow or tool name",
        "unclear scope or goal",
        "file, PR, branch or resource",
        "conflicting instructions",
        "guessed equivalent",
        "irreversible or outward-facing",
        "explicit consent",
        "I assume",
        "probably",
        "likely means",
        "not installed so I'll",
        "I'll do X instead",
        "[waiting-on-you]",
        "quote the unclear part",
        "offer the options",
        "until the developer answers",
        "workers to ask you instead of assuming",
        "not a skill",
        "read it fully before acting and follow its gates",
    ] {
        assert!(rules.contains(phrase), "missing {phrase}");
    }
    let top = DEFAULT_PROMPT.split("## Your job").next().unwrap();
    assert!(top.contains("Never act on assumptions; ask the developer"));
    let guide = include_str!("../../../orchestration/guide.md");
    for text in [DEFAULT_PROMPT, guide] {
        assert!(text.contains("orchestrator-rules.md#never-act-on-assumptions"));
        assert!(text.contains("orchestrator-rules.md#named-workflows"));
        assert!(!text.contains("Example workflows"));
        assert!(!text.contains("example workflows"));
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(repo.join("workflows")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_str().unwrap();
        assert!(
            rules.contains(&format!("`{name}`")),
            "unlisted workflow: {name}"
        );
    }
    let prompt = fill(
        DEFAULT_PROMPT,
        &values(Some(("test", 7)), Path::new("/data")),
    );
    assert!(
        prompt.contains(&format!("Workflows: {}/", repo.join("workflows").display())),
        "{prompt}"
    );
}

#[test]
fn workflow_paths_use_source_detection_only_for_dev_launches() {
    let root = temp_root("workflow-paths");
    let repo = root.join("checkout");
    std::fs::create_dir_all(repo.join("src/devtools")).unwrap();
    std::fs::create_dir_all(repo.join("workflows")).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "name = \"bus\"").unwrap();
    let repo = repo.canonicalize().unwrap();
    let docs = root.join("installed/session/docs");
    let source = repo.join("workflows");
    let bundled = docs.join("workflows");
    let exe = repo.join("target/workflow-docs/debug/bus");
    let external = root.join("installed/bin/bus");
    for (dev, executable, cwd, expected) in [
        (true, Some(exe.as_path()), None, &source),
        (
            true,
            Some(external.as_path()),
            Some(repo.as_path()),
            &source,
        ),
        (false, Some(exe.as_path()), Some(repo.as_path()), &bundled),
        (
            true,
            Some(external.as_path()),
            Some(root.as_path()),
            &bundled,
        ),
        (true, None, None, &bundled),
    ] {
        assert_eq!(workflows_dir_at(&docs, dev, executable, cwd), *expected);
    }
    std::fs::remove_dir_all(repo.join("workflows")).unwrap();
    assert_eq!(
        workflows_dir_at(&docs, true, Some(&exe), Some(&repo)),
        bundled
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// What MASTER carries (progress, questions, blockers, plan changes) and what
/// stays in the work room, as the prompt and rules word it.
const MASTER_REPORT_PHRASES: [&str; 7] = [
    "Report progress, questions and blockers in MASTER; keep details in your room.",
    "A turn no message started posts in your work room.",
    "a progress report when a step or milestone finishes or fails (a PR merged, a task done, a gate failed);",
    "every question or decision for the developer;",
    "every blocker that waits on the developer (a dialog they must approve, a destructive action);",
    "anything that changes the plan.",
    "Keep in your work room: routine dialog approvals, worker hand-offs, step-by-step details, and \"nothing is waiting on you\" notes.",
];

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
        (
            "bus-communication-guide.md",
            "orchestration/bus-communication-guide.md",
        ),
        ("providers/grok.md", "orchestration/providers/grok.md"),
        ("how-to-bus-cli.md", "orchestration/how-to-bus-cli.md"),
        ("orchestrator-guide.md", "orchestration/guide.md"),
        ("orchestrator-rules.md", "orchestration/rules.md"),
        ("parallel-writes.md", "orchestration/parallel-writes.md"),
        ("templates/workflow-template.md", "workflows/template.md"),
        (
            "templates/investigation-report.md",
            "workflows/investigation-report-template.md",
        ),
        (
            "templates/bug-triage-table.md",
            "workflows/bug-triage-table-template.md",
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
fn parallel_writes_doc_is_installed_and_referenced_at_its_absolute_session_path() {
    let root = temp_root("parallel-writes");
    let session = root.join("sessions/scratch");
    let docs = write_docs(&session).unwrap();
    let installed = docs.join("parallel-writes.md");
    assert!(installed.is_absolute(), "{installed:?}");
    let written = std::fs::read(&installed)
        .unwrap_or_else(|error| panic!("missing installed doc {installed:?}: {error}"));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("orchestration/parallel-writes.md");
    let expected = std::fs::read(source).unwrap();
    assert_eq!(written, expected);
    let prompt = fill(DEFAULT_PROMPT, &values(Some(("scratch", 7)), &session));
    let references = prompt.split("## References").nth(1).unwrap();
    assert!(
        references.contains(installed.to_str().unwrap()),
        "{references}"
    );
    std::fs::write(&installed, "old installation").unwrap();
    write_docs(&session).unwrap();
    assert_eq!(std::fs::read(&installed).unwrap(), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_fix_workflow_and_templates_are_written_at_documented_paths() {
    let data = temp_root("bug-fix-docs");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let docs = write_docs(&data).unwrap();
    for (destination, source) in [
        ("workflows/bug-fix.md", "workflows/bug-fix.md"),
        (
            "templates/investigation-report.md",
            "workflows/investigation-report-template.md",
        ),
        (
            "templates/bug-triage-table.md",
            "workflows/bug-triage-table-template.md",
        ),
    ] {
        let written = std::fs::read(docs.join(destination))
            .unwrap_or_else(|error| panic!("missing bundled doc {destination}: {error}"));
        assert_eq!(
            written,
            std::fs::read(repo.join(source)).unwrap(),
            "{destination}"
        );
    }
    std::fs::remove_dir_all(data).unwrap();
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
    // Replies appear in the request's room for developer and agent senders.
    // Autonomous finals appear in the work room. Progress, questions, blockers
    // and plan changes go to MASTER with `send --to human`.
    for phrase in MASTER_REPORT_PHRASES {
        assert!(flat(&text).contains(phrase), "missing {phrase}: {text}");
    }
    assert!(!text.contains("automatically, whatever woke you"), "{text}");
    assert!(
        text.contains("bus send --room master --as orch --to human"),
        "{text}"
    );
    assert!(text.contains("Parallelism is king"), "{text}");
    assert!(text.contains("Compartmentalize, within reason"), "{text}");
    assert!(text.contains("Size the team to the current step"), "{text}");
    assert!(text.contains("Brief workers directly"), "{text}");
    // The chat rules orchestrators break after compaction live in the Rules
    // section, linked to their full versions in the orchestrator rules.
    let rules_section = flat(&text[text.find("## Rules").unwrap()..]);
    for phrase in [
        "Write every chat message in Markdown",
        "Never write a bare PR, MR or issue number",
        "`gh pr view N --json title,url`",
        "orchestrator-rules.md#chat-markdown",
        "Put `[waiting-on-you]` before every request that needs the developer",
        "orchestrator-rules.md#waiting-on-you",
    ] {
        assert!(
            rules_section.contains(phrase),
            "missing {phrase}: {rules_section}"
        );
    }
    let rules = include_str!("../../../orchestration/rules.md");
    for anchor in ["chat-markdown", "waiting-on-you"] {
        assert!(
            rules.contains(&format!("<a id=\"{anchor}\"></a>")),
            "missing {anchor}"
        );
    }
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
        "{{DOCS}}{{ROOM_ID}}{{AGENT_NAME}}{{ROOM_NAME}}{{WORKFLOWS_DIR}}"
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
        for (name, text) in authored
            .iter()
            .filter(|(name, _)| *name != "orchestrator-rules.md")
        {
            assert_eq!(
                std::fs::read(docs.join(name)).unwrap(),
                text.as_bytes(),
                "{name}"
            );
        }
        let rules = std::fs::read_to_string(docs.join("orchestrator-rules.md")).unwrap();
        assert!(rules.contains(&format!("`{}/`", workflows_dir(&docs).display())));
        assert!(!rules.contains("{{WORKFLOWS_DIR}}"));
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
    for (name, text) in DOCS
        .iter()
        .filter(|(name, _)| *name != "orchestrator-rules.md")
    {
        assert_eq!(
            std::fs::read_to_string(root.join(name)).unwrap(),
            *text,
            "{name}"
        );
    }
    let rules = std::fs::read_to_string(root.join("orchestrator-rules.md")).unwrap();
    assert!(rules.starts_with("# Orchestrator rules"), "{rules}");
    assert!(flat(&rules).contains("`./run dev-control` is for foreground controls."));
    assert!(flat(&rules)
        .contains("For background dispatches, call the existing Bus executable directly;"));
    assert!(flat(&rules).contains(
        "**Report progress, questions and blockers in MASTER; keep details in your room.**"
    ));
    for phrase in MASTER_REPORT_PHRASES {
        assert!(flat(&rules).contains(phrase), "missing {phrase}: {rules}");
    }
    assert!(
        !rules.contains("Bus posts that message to MASTER"),
        "{rules}"
    );
    assert!(rules.contains("bus send --room master --as YOUR_NAME --to human"));
    let communication =
        flat(&std::fs::read_to_string(root.join("bus-communication-guide.md")).unwrap());
    for phrase in [
        "**Send a message only when it changes what the receiver does next.**",
        "## Peer to peer first",
        "## To the orchestrator",
        "## To you (the developer)",
        "**`[waiting-on-you]` means a hard blocker**",
        "## Language and courtesy",
    ] {
        assert!(communication.contains(phrase), "missing {phrase}");
    }
    let cli = flat(&std::fs::read_to_string(root.join("how-to-bus-cli.md")).unwrap());
    for phrase in [
        "A reply lands in the message's room, equally for developer and agent senders.",
        "A turn no message started posts in the orchestrator's work room.",
        "### Who to message",
        "[Bus communication guide](bus-communication-guide.md)",
        "#### Reach the developer from any room",
        "For a developer-only request from any room:",
        "bus send --room master --as claude-orch --to human",
    ] {
        assert!(cli.contains(phrase), "missing {phrase}: {cli}");
    }
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

#[test]
fn fill_keeps_placeholder_text_in_names_literal() {
    let mut state = crate::messaging::model::BusState::new();
    let room_name = "review {{ROOM_ID}} {{AGENT_NAME}} {{DOCS}}";
    let room = state.create_room(room_name).unwrap();
    let agent_name = "orch {{DOCS}}";
    state
        .create_agent(
            room,
            agent_name,
            Provider::ClaudeCode,
            PathBuf::from("/repo"),
            None,
        )
        .unwrap();
    let values = PromptValues {
        room: Some((room_name.into(), room)),
        agent: agent_name.into(),
        docs: PathBuf::from("/data/docs"),
    };
    assert_eq!(
        fill(
            "Room {{ROOM_NAME}}; id {{ROOM_ID}}; agent {{AGENT_NAME}}; docs {{DOCS}}",
            &values
        ),
        format!(
            "Room {room_name}; id {}; agent {agent_name}; docs /data/docs",
            room.0
        )
    );
}

#[test]
fn fill_replaces_placeholders_after_literal_open_braces() {
    let rendered = fill(
        "Room {{{ROOM_NAME}}}; id {{{ROOM_ID}}}; agent {{{AGENT_NAME}}}; docs {{{DOCS}}}",
        &values(Some(("pr-123", 7)), Path::new("/data")),
    );
    assert_eq!(
        rendered,
        "Room {pr-123}; id {7}; agent {orch}; docs {/data/docs}"
    );
}

#[test]
fn fill_matches_sequential_replacement_when_no_value_is_rescanned() {
    let values = PromptValues {
        room: Some(("a{".into(), RoomId(7))),
        agent: "CS".into(),
        docs: PathBuf::from("/data/docs"),
    };
    let sequential = |template: &str| {
        template
            .replace("{{ROOM_NAME}}", "a{")
            .replace("{{ROOM_ID}}", "7")
            .replace("{{AGENT_NAME}}", "CS")
            .replace("{{DOCS}}", "/data/docs")
    };
    for template in [
        "Room {{ROOM_NAME}} ({{ROOM_ID}}), agent {{AGENT_NAME}}, docs {{DOCS}}",
        "{{{ROOM_ID}}} {{{{AGENT_NAME}}}} {{ROOM_ID}",
    ] {
        assert_eq!(fill(template, &values), sequential(template), "{template}");
    }
}

#[test]
fn fill_never_completes_a_placeholder_from_inserted_text() {
    let values = PromptValues {
        room: Some(("ID}}{{ROOM_".into(), RoomId(7))),
        agent: "{{DO".into(),
        docs: PathBuf::from("/data/docs"),
    };
    for (template, expected) in [
        ("{{ROOM_NAME}}{{ROOM_NAME}}", "ID}}{{ROOM_ID}}{{ROOM_"),
        ("{{AGENT_NAME}}CS}}", "{{DOCS}}"),
        ("{{ROOM_NAME}}ID}}", "ID}}{{ROOM_ID}}"),
    ] {
        assert_eq!(fill(template, &values), expected, "{template}");
    }
}

#[test]
fn agent_doc_tier_check_distinguishes_diagnostic_prose_from_commands() {
    let mentions = crate::messaging::coordinator::dev_tier_mentions;
    assert!(mentions("Open a diagnostics PR to add diagnostic logging.").is_empty());
    for command in ["bus diagnostics", "`diagnostics`", "\n  diagnostics"] {
        assert!(
            !mentions(command).is_empty(),
            "missed dev command: {command}"
        );
    }
}

/// The CLI guide's orchestrator section is the one agent doc that names
/// `bus --dev restart`. Every other `--dev` mention stays forbidden.
fn dev_restart_section_removed(name: &str, text: &str) -> String {
    const HEADING: &str = "## Developer restart for orchestrators\n";
    if std::path::Path::new(name)
        .file_name()
        .is_none_or(|file| file != "how-to-bus-cli.md")
    {
        return text.to_owned();
    }
    let start = text
        .find(HEADING)
        .unwrap_or_else(|| panic!("{name} is missing the orchestrator restart section"));
    let rest = &text[start + HEADING.len()..];
    let end = rest
        .find("\n## ")
        .map(|index| start + HEADING.len() + index)
        .unwrap_or(text.len());
    assert!(
        text[start..end].contains("bus --dev restart"),
        "{name} restart section does not name the dev command"
    );
    let mut kept = text[..start].to_owned();
    kept.push_str(&text[end..]);
    kept
}

/// Agents read only these docs, so none of them may teach a dev-tier command
/// or tell agents to start Bus with `--dev`: every session answers the agent
/// tier, and dev tools stay in `bus --dev --help` and the repo's docs/.
/// Orchestrators learn `bus --dev restart` only from the CLI guide section above.
#[test]
fn docs_written_for_agents_name_no_dev_tool_and_no_dev_flag() {
    let data = temp_root("tiers");
    let root = write_docs(&data).unwrap();
    let mut pending = vec![root];
    let mut texts = vec![("prompt.md".to_owned(), DEFAULT_PROMPT.to_owned())];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                texts.push((
                    path.display().to_string(),
                    std::fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }
    assert!(texts.len() > DOCS.len(), "{texts:?}");
    for (name, text) in &texts {
        assert_eq!(
            crate::messaging::coordinator::dev_tier_mentions(text),
            Vec::<String>::new(),
            "{name}"
        );
        let checked = dev_restart_section_removed(name, text);
        assert!(!checked.contains("--dev"), "{name} mentions --dev");
    }
    std::fs::remove_dir_all(data).unwrap();
}

#[test]
fn fresh_launch_instructions_keep_bus_protocol_without_creating_a_task() {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "codex1", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let text = agent_onboarding("codex1", agent, room, Path::new("/bus-docs"));
    assert!(text.contains("Bus messaging: you are codex1"));
    assert!(text.contains("read /bus-docs/bus-communication-guide.md"));
    assert!(text.contains("send only what changes the receiver's next step"));
    assert!(text.contains("Read /bus-docs/how-to-bus-cli.md"));
    assert!(text.contains("keep fixing until every reviewer is Ready"));
    assert!(text.contains("escalate sharp reviewer disagreements, goal changes"));
    assert!(text.contains("destructive or outward-facing actions without existing authorization"));
    assert!(text.contains("Send only the body"));
    assert!(text.contains("use it only for a hard blocker only the developer can clear"));
    assert!(text
        .contains("bus send --room ROOM --as AGENT --to you --text \"[waiting-on-you] ACTION\""));
    assert_eq!(state.requests().count(), 0);
}
