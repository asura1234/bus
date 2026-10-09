#[test]
fn identify_known_agents() {
    assert_eq!(identify_agent("pi"), Some(AgentKind::Pi));
    assert_eq!(identify_agent("claude"), Some(AgentKind::Claude));
    assert_eq!(identify_agent("claude-code"), Some(AgentKind::Claude));
    assert_eq!(identify_agent("codex"), Some(AgentKind::Codex));
    assert_eq!(identify_agent("gemini"), Some(AgentKind::Gemini));
    assert_eq!(identify_agent("cursor"), Some(AgentKind::Cursor));
    assert_eq!(identify_agent("cursor-agent"), Some(AgentKind::Cursor));
    assert_eq!(identify_agent("devin"), Some(AgentKind::Devin));
    assert_eq!(identify_agent("devin-cli"), Some(AgentKind::Devin));
    assert_eq!(identify_agent("agy"), Some(AgentKind::Antigravity));
    assert_eq!(
        identify_agent("antigravity-cli"),
        Some(AgentKind::Antigravity)
    );
    assert_eq!(identify_agent("cline"), Some(AgentKind::Cline));
    assert_eq!(identify_agent("omp"), Some(AgentKind::Omp));
    assert_eq!(identify_agent("mastracode"), Some(AgentKind::Mastracode));
    assert_eq!(identify_agent("mastra-code"), Some(AgentKind::Mastracode));
    assert_eq!(identify_agent("opencode"), Some(AgentKind::OpenCode));
    assert_eq!(identify_agent("opencode.exe"), Some(AgentKind::OpenCode));
    assert_eq!(identify_agent("opencode2"), Some(AgentKind::OpenCode));
    assert_eq!(identify_agent("opencode2.exe"), Some(AgentKind::OpenCode));
    assert_eq!(identify_agent("kimi"), Some(AgentKind::Kimi));
    assert_eq!(identify_agent("Kimi Code"), Some(AgentKind::Kimi));
    assert_eq!(identify_agent("kiro"), Some(AgentKind::Kiro));
    assert_eq!(identify_agent("kiro-cli"), Some(AgentKind::Kiro));
    assert_eq!(identify_agent("copilot"), Some(AgentKind::GithubCopilot));
    assert_eq!(identify_agent("ghcs"), Some(AgentKind::GithubCopilot));
    assert_eq!(identify_agent("grok"), Some(AgentKind::Grok));
    assert_eq!(identify_agent("grok-build"), Some(AgentKind::Grok));
    assert_eq!(identify_agent("hermes"), Some(AgentKind::Hermes));
    assert_eq!(identify_agent("hermes-agent"), Some(AgentKind::Hermes));
    assert_eq!(identify_agent("kilo"), Some(AgentKind::Kilo));
    assert_eq!(identify_agent("kilo-code"), Some(AgentKind::Kilo));
    assert_eq!(identify_agent("qwen"), Some(AgentKind::Qwen));
    assert_eq!(identify_agent("Qwen Code"), Some(AgentKind::Qwen));
    assert_eq!(identify_agent("maki"), Some(AgentKind::Maki));
    assert_eq!(identify_agent("muse"), Some(AgentKind::Muse));
    assert_eq!(identify_agent("muse-code"), Some(AgentKind::Muse));
    assert_eq!(identify_agent("muse-cli"), Some(AgentKind::Muse));
    assert_eq!(
        identify_agent("muse-bin-0.1.0-R708.1"),
        Some(AgentKind::Muse)
    );
    assert_eq!(identify_agent("muse-bin-1.2.3"), Some(AgentKind::Muse));
    assert_eq!(
        identify_agent("/home/user/.local/bin/muse-bin-0.2.1-R1215.1"),
        Some(AgentKind::Muse)
    );
    assert_eq!(
        identify_agent(r"C:\Users\user\muse-bin-0.2.1-R1215.1.exe"),
        Some(AgentKind::Muse)
    );
}

#[test]
fn parse_known_agent_labels() {
    assert_eq!(parse_agent_label("pi"), Some(AgentKind::Pi));
    assert_eq!(parse_agent_label("claude"), Some(AgentKind::Claude));
    assert_eq!(parse_agent_label("cursor-agent"), Some(AgentKind::Cursor));
    assert_eq!(parse_agent_label("devin-cli"), Some(AgentKind::Devin));
    assert_eq!(parse_agent_label("agy"), Some(AgentKind::Antigravity));
    assert_eq!(
        parse_agent_label("antigravity"),
        Some(AgentKind::Antigravity)
    );
    assert_eq!(parse_agent_label("omp"), Some(AgentKind::Omp));
    assert_eq!(parse_agent_label("mastracode"), Some(AgentKind::Mastracode));
    assert_eq!(
        parse_agent_label("mastra code"),
        Some(AgentKind::Mastracode)
    );
    assert_eq!(parse_agent_label("opencode.exe"), Some(AgentKind::OpenCode));
    assert_eq!(parse_agent_label("copilot"), Some(AgentKind::GithubCopilot));
    assert_eq!(parse_agent_label("kimi-code"), Some(AgentKind::Kimi));
    assert_eq!(
        parse_agent_label("github-copilot"),
        Some(AgentKind::GithubCopilot)
    );
    assert_eq!(parse_agent_label("amp-local"), Some(AgentKind::Amp));
    assert_eq!(parse_agent_label("kiro-cli"), Some(AgentKind::Kiro));
    assert_eq!(parse_agent_label("grok-build"), Some(AgentKind::Grok));
    assert_eq!(parse_agent_label("hermes-agent"), Some(AgentKind::Hermes));
    assert_eq!(parse_agent_label("qwen-code"), Some(AgentKind::Qwen));
    assert_eq!(parse_agent_label("maki"), Some(AgentKind::Maki));
    assert_eq!(parse_agent_label("kilo-code"), Some(AgentKind::Kilo));
}

#[test]
fn every_agent_label_round_trips_through_canonical_and_alias_parsers() {
    for agent in AgentKind::ALL {
        let label = agent_label(agent);
        assert_eq!(parse_canonical_agent_label(label), Some(agent));
        assert_eq!(parse_agent_label(label), Some(agent));
    }
}

#[test]
fn every_agent_has_a_canonical_interactive_executable() {
    let expected = [
        (AgentKind::Pi, "pi"),
        (AgentKind::Claude, "claude"),
        (AgentKind::Codex, "codex"),
        (AgentKind::Gemini, "gemini"),
        (
            AgentKind::Cursor,
            if cfg!(windows) {
                "cursor-agent.cmd"
            } else {
                "cursor-agent"
            },
        ),
        (AgentKind::Devin, "devin"),
        (AgentKind::Antigravity, "agy"),
        (AgentKind::Cline, "cline"),
        (AgentKind::Omp, "omp"),
        (AgentKind::Mastracode, "mastracode"),
        (AgentKind::OpenCode, "opencode"),
        (AgentKind::GithubCopilot, "copilot"),
        (AgentKind::Kimi, "kimi"),
        (AgentKind::Kiro, "kiro-cli"),
        (AgentKind::Droid, "droid"),
        (AgentKind::Amp, "amp"),
        (AgentKind::Grok, "grok"),
        (AgentKind::Hermes, "hermes"),
        (AgentKind::Kilo, "kilo"),
        (AgentKind::Qodercli, "qodercli"),
        (AgentKind::Qwen, "qwen"),
        (AgentKind::Maki, "maki"),
        (AgentKind::Muse, "muse"),
    ];
    assert_eq!(expected.len(), AgentKind::ALL.len());
    for (agent, executable) in expected {
        assert_eq!(interactive_agent_executable(agent), executable);
    }
}

#[test]
fn canonical_agent_labels_are_strict() {
    assert_eq!(parse_canonical_agent_label("claude-code"), None);
    assert_eq!(parse_canonical_agent_label("Pi"), None);
    assert_eq!(parse_canonical_agent_label(" pi "), None);
    assert_eq!(parse_canonical_agent_label("opencode.exe"), None);
}

#[test]
fn identify_unknown_processes() {
    assert_eq!(identify_agent("bash"), None);
    assert_eq!(identify_agent("zsh"), None);
    assert_eq!(identify_agent("vim"), None);
    assert_eq!(identify_agent("node"), None);
    assert_eq!(identify_agent("museum"), None);
    assert_eq!(identify_agent("muse-helper"), None);
    assert_eq!(identify_agent("muser"), None);
    assert_eq!(identify_agent("musescore"), None);
    assert_eq!(identify_agent("muse-bin"), None);
    assert_eq!(identify_agent("muse-bin-"), None);
    assert_eq!(identify_agent("muse-binary"), None);
}

#[test]
fn identify_case_insensitive() {
    assert_eq!(identify_agent("Pi"), Some(AgentKind::Pi));
    assert_eq!(identify_agent("CLAUDE"), Some(AgentKind::Claude));
    assert_eq!(identify_agent("Codex"), Some(AgentKind::Codex));
    assert_eq!(identify_agent("Devin"), Some(AgentKind::Devin));
}
