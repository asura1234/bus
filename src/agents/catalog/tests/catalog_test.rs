#[test]
fn identify_known_agents() {
    assert_eq!(identify_agent("pi"), Some(Agent::Pi));
    assert_eq!(identify_agent("claude"), Some(Agent::Claude));
    assert_eq!(identify_agent("claude-code"), Some(Agent::Claude));
    assert_eq!(identify_agent("codex"), Some(Agent::Codex));
    assert_eq!(identify_agent("gemini"), Some(Agent::Gemini));
    assert_eq!(identify_agent("cursor"), Some(Agent::Cursor));
    assert_eq!(identify_agent("cursor-agent"), Some(Agent::Cursor));
    assert_eq!(identify_agent("devin"), Some(Agent::Devin));
    assert_eq!(identify_agent("devin-cli"), Some(Agent::Devin));
    assert_eq!(identify_agent("agy"), Some(Agent::Antigravity));
    assert_eq!(identify_agent("antigravity-cli"), Some(Agent::Antigravity));
    assert_eq!(identify_agent("cline"), Some(Agent::Cline));
    assert_eq!(identify_agent("omp"), Some(Agent::Omp));
    assert_eq!(identify_agent("mastracode"), Some(Agent::Mastracode));
    assert_eq!(identify_agent("mastra-code"), Some(Agent::Mastracode));
    assert_eq!(identify_agent("opencode"), Some(Agent::OpenCode));
    assert_eq!(identify_agent("opencode.exe"), Some(Agent::OpenCode));
    assert_eq!(identify_agent("opencode2"), Some(Agent::OpenCode));
    assert_eq!(identify_agent("opencode2.exe"), Some(Agent::OpenCode));
    assert_eq!(identify_agent("kimi"), Some(Agent::Kimi));
    assert_eq!(identify_agent("Kimi Code"), Some(Agent::Kimi));
    assert_eq!(identify_agent("kiro"), Some(Agent::Kiro));
    assert_eq!(identify_agent("kiro-cli"), Some(Agent::Kiro));
    assert_eq!(identify_agent("copilot"), Some(Agent::GithubCopilot));
    assert_eq!(identify_agent("ghcs"), Some(Agent::GithubCopilot));
    assert_eq!(identify_agent("grok"), Some(Agent::Grok));
    assert_eq!(identify_agent("grok-build"), Some(Agent::Grok));
    assert_eq!(identify_agent("hermes"), Some(Agent::Hermes));
    assert_eq!(identify_agent("hermes-agent"), Some(Agent::Hermes));
    assert_eq!(identify_agent("kilo"), Some(Agent::Kilo));
    assert_eq!(identify_agent("kilo-code"), Some(Agent::Kilo));
    assert_eq!(identify_agent("qwen"), Some(Agent::Qwen));
    assert_eq!(identify_agent("Qwen Code"), Some(Agent::Qwen));
    assert_eq!(identify_agent("maki"), Some(Agent::Maki));
    assert_eq!(identify_agent("muse"), Some(Agent::Muse));
    assert_eq!(identify_agent("muse-code"), Some(Agent::Muse));
    assert_eq!(identify_agent("muse-cli"), Some(Agent::Muse));
    assert_eq!(identify_agent("muse-bin-0.1.0-R708.1"), Some(Agent::Muse));
    assert_eq!(identify_agent("muse-bin-1.2.3"), Some(Agent::Muse));
    assert_eq!(
        identify_agent("/home/user/.local/bin/muse-bin-0.2.1-R1215.1"),
        Some(Agent::Muse)
    );
    assert_eq!(
        identify_agent(r"C:\Users\user\muse-bin-0.2.1-R1215.1.exe"),
        Some(Agent::Muse)
    );
}

#[test]
fn parse_known_agent_labels() {
    assert_eq!(parse_agent_label("pi"), Some(Agent::Pi));
    assert_eq!(parse_agent_label("claude"), Some(Agent::Claude));
    assert_eq!(parse_agent_label("cursor-agent"), Some(Agent::Cursor));
    assert_eq!(parse_agent_label("devin-cli"), Some(Agent::Devin));
    assert_eq!(parse_agent_label("agy"), Some(Agent::Antigravity));
    assert_eq!(parse_agent_label("antigravity"), Some(Agent::Antigravity));
    assert_eq!(parse_agent_label("omp"), Some(Agent::Omp));
    assert_eq!(parse_agent_label("mastracode"), Some(Agent::Mastracode));
    assert_eq!(parse_agent_label("mastra code"), Some(Agent::Mastracode));
    assert_eq!(parse_agent_label("opencode.exe"), Some(Agent::OpenCode));
    assert_eq!(parse_agent_label("copilot"), Some(Agent::GithubCopilot));
    assert_eq!(parse_agent_label("kimi-code"), Some(Agent::Kimi));
    assert_eq!(
        parse_agent_label("github-copilot"),
        Some(Agent::GithubCopilot)
    );
    assert_eq!(parse_agent_label("amp-local"), Some(Agent::Amp));
    assert_eq!(parse_agent_label("kiro-cli"), Some(Agent::Kiro));
    assert_eq!(parse_agent_label("grok-build"), Some(Agent::Grok));
    assert_eq!(parse_agent_label("hermes-agent"), Some(Agent::Hermes));
    assert_eq!(parse_agent_label("qwen-code"), Some(Agent::Qwen));
    assert_eq!(parse_agent_label("maki"), Some(Agent::Maki));
    assert_eq!(parse_agent_label("kilo-code"), Some(Agent::Kilo));
}

#[test]
fn every_agent_label_round_trips_through_canonical_and_alias_parsers() {
    for agent in Agent::ALL {
        let label = agent_label(agent);
        assert_eq!(parse_canonical_agent_label(label), Some(agent));
        assert_eq!(parse_agent_label(label), Some(agent));
    }
}

#[test]
fn every_agent_has_a_canonical_interactive_executable() {
    let expected = [
        (Agent::Pi, "pi"),
        (Agent::Claude, "claude"),
        (Agent::Codex, "codex"),
        (Agent::Gemini, "gemini"),
        (
            Agent::Cursor,
            if cfg!(windows) {
                "cursor-agent.cmd"
            } else {
                "cursor-agent"
            },
        ),
        (Agent::Devin, "devin"),
        (Agent::Antigravity, "agy"),
        (Agent::Cline, "cline"),
        (Agent::Omp, "omp"),
        (Agent::Mastracode, "mastracode"),
        (Agent::OpenCode, "opencode"),
        (Agent::GithubCopilot, "copilot"),
        (Agent::Kimi, "kimi"),
        (Agent::Kiro, "kiro-cli"),
        (Agent::Droid, "droid"),
        (Agent::Amp, "amp"),
        (Agent::Grok, "grok"),
        (Agent::Hermes, "hermes"),
        (Agent::Kilo, "kilo"),
        (Agent::Qodercli, "qodercli"),
        (Agent::Qwen, "qwen"),
        (Agent::Maki, "maki"),
        (Agent::Muse, "muse"),
    ];
    assert_eq!(expected.len(), Agent::ALL.len());
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
    assert_eq!(identify_agent("Pi"), Some(Agent::Pi));
    assert_eq!(identify_agent("CLAUDE"), Some(Agent::Claude));
    assert_eq!(identify_agent("Codex"), Some(Agent::Codex));
    assert_eq!(identify_agent("Devin"), Some(Agent::Devin));
}
