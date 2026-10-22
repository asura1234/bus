//! Metadata from Grok Build 1.0.5 help; unknown options stay opaque.
use crate::agents::providers::launch::is_session_uuid;

fn name(arg: &str) -> &str {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    if name.len() > 2 {
        for short in ["-m", "-p", "-r", "-s", "-w"] {
            if name.starts_with(short) {
                return short;
            }
        }
    }
    name
}

pub(crate) fn option_arity(arg: &str) -> Option<usize> {
    let option = name(arg);
    if option != arg {
        return Some(0);
    } // attached value
    match option {
        "--agent"
        | "--agents"
        | "--allow"
        | "--allowedTools"
        | "--deny"
        | "--disallowedTools"
        | "--cwd"
        | "--debug-file"
        | "--disallowed-tools"
        | "--json-schema"
        | "--leader-socket"
        | "-m"
        | "--model"
        | "--max-turns"
        | "--output-format"
        | "-p"
        | "--single"
        | "--permission-mode"
        | "--prompt-file"
        | "--prompt-json"
        | "-r"
        | "--resume"
        | "--reasoning-effort"
        | "--effort"
        | "--rules"
        | "-s"
        | "--session-id"
        | "--sandbox"
        | "--system-prompt-override"
        | "--system-prompt"
        | "--tools"
        | "-w"
        | "--worktree"
        | "--worktree-ref"
        | "--ref" => Some(1),
        "--always-approve"
        | "--yolo"
        | "-c"
        | "--continue"
        | "--debug"
        | "--disable-web-search"
        | "--fork-session"
        | "--fullscreen"
        | "-h"
        | "--help"
        | "--include-partial-messages"
        | "--leader"
        | "--no-leader"
        | "--minimal"
        | "--no-alt-screen"
        | "--no-plan"
        | "--no-subagents"
        | "--oauth"
        | "--restore-code"
        | "-v"
        | "--version"
        | "--verbatim"
        | "--no-auto-update" => Some(0),
        _ => None,
    }
}

pub(crate) fn is_session_selector(arg: &str) -> bool {
    matches!(
        name(arg),
        "--resume" | "-r" | "--continue" | "-c" | "--session-id" | "-s" | "--fork-session"
    )
}

/// Split only documented native short options. A value-taking short consumes
/// the remaining bytes; unknown syntax stays opaque, including its entire tail.
fn short_options(arg: &str) -> Option<(Vec<String>, Option<&'static str>)> {
    if !arg.starts_with('-') || arg.starts_with("--") || arg.len() < 2 {
        return None;
    }
    let mut result = Vec::new();
    for (offset, option) in arg[1..].char_indices() {
        let (long, value) = match option {
            'c' => ("--continue", false),
            'h' => ("--help", false),
            'v' => ("--version", false),
            'm' => ("--model", true),
            'p' => ("--single", true),
            'r' => ("--resume", true),
            's' => ("--session-id", true),
            'w' => ("--worktree", true),
            _ => return None,
        };
        result.push(long.into());
        if value {
            let tail = &arg[offset + 2..];
            if tail.is_empty() {
                return Some((result, Some(long)));
            }
            result.push(tail.strip_prefix('=').unwrap_or(tail).into());
            return Some((result, None));
        }
    }
    Some((result, None))
}

fn has_value(option: &str, next: Option<&String>) -> bool {
    next.is_some_and(|value| {
        !matches!(option, "--resume" | "-r" | "--worktree" | "-w") || !value.starts_with('-')
    })
}

fn expanded_args(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            result.extend_from_slice(&args[index..]);
            break;
        }
        if let Some((options, value)) = short_options(arg) {
            result.extend(options);
            index += 1;
            if value.is_some_and(|option| has_value(option, args.get(index))) {
                result.push(args[index].clone());
                index += 1;
            }
            continue;
        }
        result.push(arg.clone());
        index += 1;
        if !arg.starts_with('-') || arg.contains('=') {
            continue;
        }
        let Some(arity) = option_arity(arg) else {
            result.extend_from_slice(&args[index..]);
            break;
        };
        if arity == 1 && has_value(arg, args.get(index)) {
            result.push(args[index].clone());
            index += 1;
        }
    }
    result
}

/// Like the shared scanner, stop at unknown arity and the literal tail. Grok's
/// optional resume/worktree values do not swallow a following flag.
fn positions(args: &[String]) -> Vec<usize> {
    let mut result = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            result.push(index);
            index += 1;
            continue;
        }
        result.push(index);
        if arg.contains('=') || name(arg) != arg {
            index += 1;
            continue;
        }
        let Some(arity) = option_arity(arg) else {
            break;
        };
        index += 1;
        if arity == 1
            && args.get(index).is_some_and(|v| {
                !matches!(arg.as_str(), "--resume" | "-r" | "--worktree" | "-w")
                    || !v.starts_with('-')
            })
        {
            index += 1;
        }
    }
    result
}

pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let expanded = expanded_args(args);
    if positions(&expanded)
        .into_iter()
        .filter(|&i| is_session_selector(&expanded[i]))
        .count()
        != 1
    {
        return Ok(None);
    }
    let selectors: Vec<_> = positions(args)
        .into_iter()
        .filter(|&i| is_session_selector(&args[i]))
        .collect();
    let [index] = selectors.as_slice() else {
        return Ok(None);
    };
    let index = *index;
    let arg = &args[index];
    if !matches!(name(arg), "--resume" | "-r") {
        return Ok(None);
    }
    let (value, count) = if let Some((_, value)) = arg.split_once('=') {
        (value.to_owned(), 1)
    } else if arg.starts_with("-r") && arg.len() > 2 {
        (arg[2..].to_owned(), 1)
    } else if let Some(value) = args.get(index + 1) {
        (value.clone(), 2)
    } else {
        return Ok(None);
    };
    if !is_session_uuid(&value) {
        return Ok(None);
    }
    args.drain(index..index + count);
    Ok(Some(value))
}

pub(crate) fn validate_args(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("Launch arguments cannot contain NUL".into());
    }
    let expanded = expanded_args(args);
    for index in positions(&expanded) {
        let arg = &expanded[index];
        let option = name(arg);
        if matches!(option, "--cwd" | "--worktree" | "-w") {
            return Err(format!("Grok launch argument {arg} overrides Bus's selected PWD and project hooks; choose the PWD in Bus"));
        }
        if matches!(
            option,
            "-p" | "--single"
                | "--prompt-file"
                | "--prompt-json"
                | "-h"
                | "--help"
                | "-v"
                | "--version"
                | "--leader"
                | "--minimal"
                | "--no-alt-screen"
        ) || (!arg.starts_with('-')
            && matches!(
                arg.as_str(),
                "agent"
                    | "completions"
                    | "dashboard"
                    | "doctor"
                    | "du"
                    | "disk-usage"
                    | "export"
                    | "help"
                    | "inspect"
                    | "leader"
                    | "login"
                    | "logout"
                    | "mcp"
                    | "memory"
                    | "models"
                    | "plugin"
                    | "sessions"
                    | "setup"
                    | "trace"
                    | "update"
                    | "version"
                    | "v"
                    | "worktree"
                    | "wrap"
            ))
        {
            return Err(format!("Grok launch argument {arg} disables the private fullscreen interactive terminal Bus needs"));
        }
    }
    Ok(())
}

pub(crate) fn runtime_additions(args: &[String]) -> Vec<String> {
    let args = expanded_args(args);
    let positions = positions(&args);
    super::runtime_args()
        .into_iter()
        .filter(|flag| !positions.iter().any(|&i| args[i] == *flag))
        .collect()
}

/// The shared resume owner uses this to omit startup-only selectors and prompts.
/// Derive argv from the verbatim saved options; never replay startup prompts,
/// literal tails or session selectors. Preserve unknown spans without guessing.
pub(crate) fn resumable_options(args: &[String]) -> Vec<String> {
    let args = expanded_args(args);
    let mut result = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            index += 1;
            continue;
        }
        let option = name(arg);
        if is_session_selector(option) {
            index += 1;
            if !arg.contains('=')
                && matches!(option, "--resume" | "--session-id")
                && args.get(index).is_some_and(|v| !v.starts_with('-'))
            {
                index += 1;
            }
            continue;
        }
        let Some(arity) = option_arity(arg) else {
            result.extend_from_slice(&args[index..]);
            break;
        };
        let end = (index + 1 + arity).min(args.len());
        result.extend_from_slice(&args[index..end]);
        index = end;
    }
    result
}
