//! Inspect only syntax we know, without an option allowlist. Unknown syntax is
//! passed through and ends inspection: guessing arity could reject a value.
use super::super::ProviderKind;

/// Known option values and literal tails are not flags. After opaque syntax,
/// avoid adding a second explicit spelling without guessing its arity.
pub(super) fn contains_flag(provider: ProviderKind, args: &[String], aliases: &[&str]) -> bool {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            return false;
        }
        if aliases.contains(&arg.as_str()) {
            return true;
        }
        if !arg.starts_with('-') {
            index += 1;
        } else if let Some(end) = option_end(provider, args, index) {
            index = end;
        } else {
            return args[index + 1..]
                .iter()
                .take_while(|arg| arg.as_str() != "--")
                .any(|arg| aliases.contains(&arg.as_str()));
        }
    }
    false
}

/// Option positions plus the first positional command/prompt. Later options
/// still count; values and the tail after `--` are never modes or selectors.
pub(crate) fn argument_positions(provider: ProviderKind, args: &[String]) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut index = 0;
    let mut seen_positional = false;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            if !seen_positional {
                positions.push(index);
                seen_positional = true;
            }
            index += 1;
            continue;
        }
        positions.push(index);
        match option_end(provider, args, index) {
            Some(end) => index = end,
            None => break,
        }
    }
    positions
}

/// A bound conversation already consumed its startup prompt/selector. Keep
/// option spans, including options after a prompt, in their original order.
/// Unknown arity preserves the entire remaining input rather than guessing.
pub(crate) fn resumable_options(provider: ProviderKind, args: &[String]) -> Vec<String> {
    let mut options = Vec::new();
    let mut index = 0;
    let mut seen_positional = false;
    let mut codex_selector = false;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            if !seen_positional
                && provider == ProviderKind::Codex
                && matches!(arg.as_str(), "resume" | "fork")
            {
                codex_selector = true;
                index += 1;
                if args.get(index).is_some_and(|value| !value.starts_with('-')) {
                    index += 1;
                }
            } else {
                index += 1;
            }
            seen_positional = true;
            continue;
        }
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        let selector = match provider {
            ProviderKind::Codex => codex_selector && matches!(name, "--last" | "--all"),
            ProviderKind::ClaudeCode => matches!(
                name,
                "--resume" | "-r" | "--session-id" | "--continue" | "-c" | "--fork-session"
            ),
            ProviderKind::Cursor => matches!(name, "--resume" | "--continue"),
        };
        if selector {
            index += 1;
            if !arg.contains('=')
                && matches!(name, "--resume" | "-r" | "--session-id")
                && args.get(index).is_some_and(|value| !value.starts_with('-'))
            {
                index += 1;
            }
            continue;
        }
        let Some(end) = option_end(provider, args, index) else {
            options.extend_from_slice(&args[index..]);
            break;
        };
        options.extend_from_slice(&args[index..end]);
        index = end;
    }
    options
}

fn option_end(provider: ProviderKind, args: &[String], index: usize) -> Option<usize> {
    let arg = &args[index];
    if arg.contains('=')
        || (provider == ProviderKind::Codex
            && arg.len() > 2
            && ["-c", "-m", "-p", "-s", "-a", "-C", "-i"]
                .iter()
                .any(|prefix| arg.starts_with(prefix)))
    {
        return Some(index + 1);
    }
    match arity(provider, arg)? {
        0 => Some(index + 1),
        1 => Some((index + 2).min(args.len())),
        _ => {
            let mut end = index + 1;
            while args.get(end).is_some_and(|value| !value.starts_with('-')) {
                end += 1;
            }
            Some(end)
        }
    }
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
