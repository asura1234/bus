/// Which agent we detected running in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Omp,
    Mastracode,
    OpenCode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
    Qwen,
    Maki,
    Muse,
}

impl AgentKind {
    #[cfg(test)]
    pub const ALL: [Self; 23] = [
        Self::Pi,
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Cursor,
        Self::Devin,
        Self::Antigravity,
        Self::Cline,
        Self::Omp,
        Self::Mastracode,
        Self::OpenCode,
        Self::GithubCopilot,
        Self::Kimi,
        Self::Kiro,
        Self::Droid,
        Self::Amp,
        Self::Grok,
        Self::Hermes,
        Self::Kilo,
        Self::Qodercli,
        Self::Qwen,
        Self::Maki,
        Self::Muse,
    ];

    pub const SCREEN_MANIFEST_AGENTS: [Self; 21] = [
        Self::Pi,
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Cursor,
        Self::Devin,
        Self::Antigravity,
        Self::Cline,
        Self::OpenCode,
        Self::GithubCopilot,
        Self::Kimi,
        Self::Kiro,
        Self::Droid,
        Self::Amp,
        Self::Grok,
        Self::Hermes,
        Self::Kilo,
        Self::Qodercli,
        Self::Qwen,
        Self::Maki,
        Self::Muse,
    ];
}

pub fn agent_label(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Pi => "pi",
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Gemini => "gemini",
        AgentKind::Cursor => "cursor",
        AgentKind::Devin => "devin",
        AgentKind::Antigravity => "agy",
        AgentKind::Cline => "cline",
        AgentKind::Omp => "omp",
        AgentKind::Mastracode => "mastracode",
        AgentKind::OpenCode => "opencode",
        AgentKind::GithubCopilot => "copilot",
        AgentKind::Kimi => "kimi",
        AgentKind::Kiro => "kiro",
        AgentKind::Droid => "droid",
        AgentKind::Amp => "amp",
        AgentKind::Grok => "grok",
        AgentKind::Hermes => "hermes",
        AgentKind::Kilo => "kilo",
        AgentKind::Qodercli => "qodercli",
        AgentKind::Qwen => "qwen",
        AgentKind::Maki => "maki",
        AgentKind::Muse => "muse",
    }
}

pub fn interactive_agent_executable(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Pi => "pi",
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Gemini => "gemini",
        AgentKind::Cursor => {
            if cfg!(windows) {
                "cursor-agent.cmd"
            } else {
                "cursor-agent"
            }
        }
        AgentKind::Devin => "devin",
        AgentKind::Antigravity => "agy",
        AgentKind::Cline => "cline",
        AgentKind::Omp => "omp",
        AgentKind::Mastracode => "mastracode",
        AgentKind::OpenCode => "opencode",
        AgentKind::GithubCopilot => "copilot",
        AgentKind::Kimi => "kimi",
        AgentKind::Kiro => "kiro-cli",
        AgentKind::Droid => "droid",
        AgentKind::Amp => "amp",
        AgentKind::Grok => "grok",
        AgentKind::Hermes => "hermes",
        AgentKind::Kilo => "kilo",
        AgentKind::Qodercli => "qodercli",
        AgentKind::Qwen => "qwen",
        AgentKind::Maki => "maki",
        AgentKind::Muse => "muse",
    }
}

pub fn parse_agent_label(agent: &str) -> Option<AgentKind> {
    let name = normalized_agent_lookup_name(agent);
    parse_canonical_agent_label(&name).or_else(|| lookup_agent(&name))
}

pub(crate) fn parse_canonical_agent_label(label: &str) -> Option<AgentKind> {
    let agent = lookup_agent(label)?;
    (agent_label(agent) == label).then_some(agent)
}

fn lookup_agent(name: &str) -> Option<AgentKind> {
    let name = path_basename(name);
    match name {
        "pi" => Some(AgentKind::Pi),
        "claude" | "claude-code" => Some(AgentKind::Claude),
        "codex" => Some(AgentKind::Codex),
        "gemini" => Some(AgentKind::Gemini),
        "cursor" | "cursor-agent" => Some(AgentKind::Cursor),
        "devin" | "devin-cli" | "devin cli" => Some(AgentKind::Devin),
        "agy" | "antigravity" | "antigravity-cli" => Some(AgentKind::Antigravity),
        "cline" => Some(AgentKind::Cline),
        "omp" => Some(AgentKind::Omp),
        "mastracode" | "mastra-code" | "mastra code" => Some(AgentKind::Mastracode),
        "opencode" | "opencode2" | "open-code" => Some(AgentKind::OpenCode),
        "copilot" | "github-copilot" | "ghcs" => Some(AgentKind::GithubCopilot),
        "kimi" | "kimi-code" | "kimi code" => Some(AgentKind::Kimi),
        "kiro" | "kiro-cli" => Some(AgentKind::Kiro),
        "droid" => Some(AgentKind::Droid),
        "amp" | "amp-local" => Some(AgentKind::Amp),
        "grok" | "grok-build" => Some(AgentKind::Grok),
        "hermes" | "hermes-agent" => Some(AgentKind::Hermes),
        "kilo" | "kilo-code" | "kilo code" => Some(AgentKind::Kilo),
        "qodercli" | "qoderclicn" | "qoder" | "qodercn" => Some(AgentKind::Qodercli),
        "qwen" | "qwen-code" | "qwen code" => Some(AgentKind::Qwen),
        "maki" => Some(AgentKind::Maki),
        "muse" | "muse-code" | "muse-cli" => Some(AgentKind::Muse),
        _ if is_muse_versioned_binary(name) => Some(AgentKind::Muse),
        _ => None,
    }
}

/// Muse's install-dir launcher script resolves the active release and execs
/// `muse-bin-<version>` (e.g. `muse-bin-0.1.0-R708.1`), so the running
/// process never carries a bare `muse`/`muse-bin` alias. Require a digit
/// immediately after the `muse-bin-` prefix so unrelated binaries such as
/// `muse-binary` or a bare `muse-bin` stay unmatched.
/// Accepts path-qualified `argv0` values by checking only the basename, since
/// the launcher may `exec` with an absolute install-dir path.
fn is_muse_versioned_binary(name: &str) -> bool {
    path_basename(name)
        .strip_prefix("muse-bin-")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

pub(super) fn normalized_agent_lookup_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

pub(super) fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}
