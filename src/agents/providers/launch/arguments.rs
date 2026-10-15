//! Inspect only syntax we know, without an option allowlist. Unknown syntax is
//! passed through and ends inspection: guessing arity could reject a value.
use super::super::ProviderKind;

/// Option positions plus the first positional command/prompt. Values and the
/// tail after `--` are never interpreted as Bus-owned selectors or modes.
pub(crate) fn argument_positions(provider: ProviderKind, args: &[String]) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            break;
        }
        positions.push(index);
        if !arg.starts_with('-') {
            break;
        }
        if arg.contains('=') {
            index += 1;
            continue;
        }
        // Codex's single-value short options accept attached values too.
        if provider == ProviderKind::Codex
            && arg.len() > 2
            && ["-c", "-m", "-p", "-s", "-a", "-C", "-i"]
                .iter()
                .any(|prefix| arg.starts_with(prefix))
        {
            index += 1;
            continue;
        }
        match arity(provider, arg) {
            Some(0) => index += 1,
            Some(1) => index += 2,
            Some(_) => {
                index += 1;
                while args.get(index).is_some_and(|value| !value.starts_with('-')) {
                    index += 1;
                }
            }
            None => break,
        }
    }
    positions
}

// This metadata only distinguishes option values. A missing name never makes
// argv invalid; providers own validation of flags, choices and required values.
fn arity(provider: ProviderKind, name: &str) -> Option<usize> {
    match provider {
        ProviderKind::Codex => codex_arity(name),
        ProviderKind::ClaudeCode => claude_arity(name),
        ProviderKind::Cursor => cursor_arity(name),
    }
}

fn codex_arity(name: &str) -> Option<usize> {
    match name {
        "-c"
        | "--config"
        | "--enable"
        | "--disable"
        | "--remote"
        | "--remote-auth-token-env"
        | "-m"
        | "--model"
        | "--local-provider"
        | "-p"
        | "--profile"
        | "-s"
        | "--sandbox"
        | "-C"
        | "--cd"
        | "--add-dir"
        | "-a"
        | "--ask-for-approval" => Some(1),
        "-i" | "--image" => Some(2),
        "--yolo"
        | "--dangerously-bypass-approvals-and-sandbox"
        | "--dangerously-bypass-hook-trust"
        | "--approve-for-me"
        | "--strict-config"
        | "--oss"
        | "--worktree"
        | "--search"
        | "--no-alt-screen"
        | "--no-daemon"
        | "--last"
        | "--all" => Some(0),
        _ => None,
    }
}

fn claude_arity(name: &str) -> Option<usize> {
    match name {
        "--model"
        | "--system-prompt"
        | "--append-system-prompt"
        | "--system-prompt-file"
        | "--append-system-prompt-file"
        | "--effort"
        | "--permission-mode"
        | "--settings"
        | "--setting-sources"
        | "--agent"
        | "--agents"
        | "--autocompact"
        | "--debug-file"
        | "--fallback-model"
        | "--input-format"
        | "--output-format"
        | "--json-schema"
        | "--max-budget-usd"
        | "--permission-prompts"
        | "--plugin-dir"
        | "--plugin-url"
        | "-n"
        | "--name"
        | "--session-id"
        | "--system-prompt-snapshot"
        | "--resume"
        | "-r" => Some(1),
        "--add-dir" | "--allowedTools" | "--allowed-tools" | "--disallowedTools"
        | "--disallowed-tools" | "--tools" | "--mcp-config" | "--betas" | "--file" => Some(2),
        "--dangerously-skip-permissions"
        | "--allow-dangerously-skip-permissions"
        | "--verbose"
        | "--ax-screen-reader"
        | "--fork-session"
        | "-c"
        | "--continue"
        | "--bare"
        | "--safe-mode"
        | "--bg"
        | "--background"
        | "-p"
        | "--print"
        | "--chrome"
        | "--no-chrome"
        | "--strict-mcp-config"
        | "--restricted"
        | "--disable-slash-commands" => Some(0),
        _ => None,
    }
}

fn cursor_arity(name: &str) -> Option<usize> {
    match name {
        "--api-key" | "-H" | "--header" | "-e" | "--endpoint" | "--output-format" | "--mode"
        | "--resume" | "--model" | "--sandbox" | "--workspace" | "--add-dir" | "--plugin-dir"
        | "--worktree-base" => Some(1),
        "-f"
        | "--force"
        | "--yolo"
        | "--auto-review"
        | "--approve-mcps"
        | "--trust"
        | "--plan"
        | "--continue"
        | "--skip-worktree-setup"
        | "--stream-partial-output"
        | "-p"
        | "--print" => Some(0),
        _ => None,
    }
}
