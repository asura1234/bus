#[cfg(windows)]
#[path = "identify/windows.rs"]
mod windows;

use super::catalog::{normalized_agent_lookup_name, path_basename};
use super::{agent_label, parse_agent_label, Agent};

/// Identify which agent is running from the process name.
/// Returns `None` for plain shells or unrecognized programs.
pub fn identify_agent(process_name: &str) -> Option<Agent> {
    parse_agent_label(process_name)
}

pub fn identify_agent_in_job(job: &crate::platform::ForegroundJob) -> Option<(Agent, String)> {
    if let Some(process) = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)
    {
        let candidate = normalized_process_name(process);
        if let Some(agent) = identify_agent(&candidate) {
            return Some((agent, candidate));
        }
    }

    let mut best: Option<(u8, Agent, String)> = None;

    for process in &job.processes {
        let candidate = normalized_process_name(process);
        let Some(agent) = identify_agent(&candidate) else {
            continue;
        };
        let score = process_priority(process, &candidate);

        match &best {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => best = Some((score, agent, candidate)),
        }
    }

    best.map(|(_, agent, name)| (agent, name))
}

// ---------------------------------------------------------------------------

/// Get the foreground job for a given child PID.
/// Delegates to platform-specific implementation.
pub fn foreground_job(child_pid: u32) -> Option<crate::platform::ForegroundJob> {
    #[cfg(windows)]
    {
        windows::foreground_job(child_pid)
    }
    #[cfg(not(windows))]
    {
        crate::platform::foreground_job(child_pid)
    }
}

/// Get the foreground process group leader as a one-process job.
/// This is cheaper than collecting every process in the foreground job.
pub fn foreground_group_leader_job(
    process_group_id: u32,
) -> Option<crate::platform::ForegroundJob> {
    crate::platform::foreground_group_leader_job(process_group_id)
}

/// Get the foreground process group for a pane shell PID.
/// This is cheaper than collecting every process in the foreground job.
pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    #[cfg(windows)]
    {
        foreground_job(child_pid).map(|job| job.process_group_id)
    }
    #[cfg(not(windows))]
    {
        crate::platform::foreground_process_group_id(child_pid)
    }
}

/// Interpret the managed-agent marker after platform has collected raw OS facts.
pub(crate) fn process_agent_hint(pid: u32) -> Option<Agent> {
    let environ = crate::platform::process_environment(pid)?;
    parse_agent_env_hint(&environ)
}

pub(super) fn parse_agent_env_hint(environ: &[u8]) -> Option<Agent> {
    for record in environ.split(|&byte| byte == 0) {
        let Some(value) = record.strip_prefix(b"HERDR_AGENT=") else {
            continue;
        };
        return parse_agent_label(std::str::from_utf8(value).ok()?);
    }
    None
}

fn normalized_process_name(process: &crate::platform::ForegroundProcess) -> String {
    let effective = process.argv0.as_deref().unwrap_or(&process.name);
    let lower_effective = effective.to_lowercase();

    if is_generic_runtime_or_shell(&lower_effective) {
        if let Some(wrapped_agent) =
            wrapped_agent_name_from_runtime_argv(&lower_effective, process.argv.as_deref())
        {
            return wrapped_agent;
        }
    }

    if identify_agent(effective).is_some() {
        return effective.to_string();
    }

    if let Some(runtime) = process.argv.as_deref().and_then(|argv| argv.first()) {
        let runtime_name = normalized_agent_lookup_name(path_basename(runtime));
        if matches!(runtime_name.as_str(), "node" | "bun") {
            if let Some(wrapped_agent) =
                wrapped_agent_name_from_runtime_argv(runtime, process.argv.as_deref())
            {
                if identify_agent(&wrapped_agent) == Some(Agent::Qwen) {
                    return wrapped_agent;
                }
            }
        }
    }

    if let Some(wrapped_agent) = argv0_agent_name(process.argv.as_deref())
        .or_else(|| cmdline_argv0_agent_name(process.cmdline.as_deref().unwrap_or_default()))
    {
        return wrapped_agent;
    }

    effective.to_string()
}

pub(super) fn wrapped_agent_name_from_runtime_argv(
    runtime: &str,
    argv: Option<&[String]>,
) -> Option<String> {
    let argv = argv?;
    let runtime_name = normalized_agent_lookup_name(path_basename(runtime));

    match runtime_name.as_str() {
        "node" => cursor_agent_name_from_bundled_node_argv(argv)
            .or_else(|| script_arg_agent_name(argv, &["-e", "--eval", "-p", "--print"], &[])),
        "bun" => script_arg_agent_name(argv, &["-e", "--eval", "-p", "--print"], &[]),
        name if is_python_runtime(name) => script_arg_agent_name(argv, &["-c"], &["-m"]),
        "sh" | "bash" | "zsh" | "fish" => script_arg_agent_name(argv, &["-c"], &[]),
        "cmd" => windows_cmd_arg_agent_name(argv),
        "powershell" | "pwsh" => powershell_arg_agent_name(argv),
        "tmux" => None,
        _ => None,
    }
}

fn cursor_agent_name_from_bundled_node_argv(argv: &[String]) -> Option<String> {
    let (runtime_parent, runtime_name) = path_parent_and_basename(argv.first()?)?;
    let (script_parent, script_name) = path_parent_and_basename(argv.get(1)?)?;
    if !runtime_name.eq_ignore_ascii_case("node.exe")
        || !script_name.eq_ignore_ascii_case("index.js")
        || !runtime_parent.eq_ignore_ascii_case(script_parent)
    {
        return None;
    }

    let mut tail = runtime_parent
        .rsplit(['/', '\\'])
        .filter(|component| !component.is_empty());
    let (Some(version), Some(versions), Some(package)) = (tail.next(), tail.next(), tail.next())
    else {
        return None;
    };
    (package.eq_ignore_ascii_case("cursor-agent")
        && versions.eq_ignore_ascii_case("versions")
        && !version.trim().is_empty())
    .then(|| agent_label(Agent::Cursor).to_string())
}

fn path_parent_and_basename(path: &str) -> Option<(&str, &str)> {
    let split = path.rfind(['/', '\\'])?;
    let parent = path[..split].trim_end_matches(['/', '\\']);
    let basename = &path[split + 1..];
    (!parent.is_empty() && !basename.is_empty()).then_some((parent, basename))
}

fn windows_cmd_arg_agent_name(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.trim_matches('"').to_lowercase();
        match flag.as_str() {
            "/c" | "/k" => {
                return args
                    .next()
                    .and_then(|command| command_text_agent_name(command))
            }
            "/d" | "/s" | "/q" | "/a" | "/u" | "/e:on" | "/e:off" | "/f:on" | "/f:off"
            | "/v:on" | "/v:off" => continue,
            _ => {}
        }
    }
    None
}

fn powershell_arg_agent_name(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.trim_matches('"').to_lowercase();
        match flag.as_str() {
            "-file" | "-f" | "/file" => {
                return args
                    .next()
                    .and_then(|path| agent_name_from_path_token(path));
            }
            "-command" | "-c" | "/command" | "/c" => {
                return args
                    .next()
                    .and_then(|command| command_text_agent_name(command));
            }
            "-encodedcommand" | "-enc" | "/encodedcommand" | "/enc" => return None,
            "-configurationname" | "-executionpolicy" | "-outputformat" | "-psconsolefile"
            | "-version" | "-windowstyle" | "-workingdirectory" => {
                let _ = args.next();
            }
            _ if flag.starts_with('-') || flag.starts_with('/') => {}
            _ => return agent_name_from_path_token(arg),
        }
    }
    None
}

fn command_text_agent_name(command: &str) -> Option<String> {
    let mut rest = command;
    while let Some((token, next)) = command_text_token(rest) {
        let token = token.trim();
        if token.eq_ignore_ascii_case("&")
            || token.eq_ignore_ascii_case(".")
            || token.eq_ignore_ascii_case("call")
        {
            rest = next;
            continue;
        }
        return agent_name_from_path_token(token);
    }
    None
}

fn command_text_token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    let first = input.chars().next()?;
    if first == '"' || first == '\'' {
        let start = first.len_utf8();
        if let Some(end) = input[start..].find(first) {
            let end = start + end;
            return Some((&input[start..end], &input[end + first.len_utf8()..]));
        }
        return Some((&input[start..], ""));
    }

    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    Some((&input[..end], &input[end..]))
}

fn script_arg_agent_name(
    argv: &[String],
    eval_flags: &[&str],
    module_flags: &[&str],
) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args
                .next()
                .and_then(|token| agent_name_from_path_token(token));
        }

        if flag_matches(arg, eval_flags) || flag_matches(arg, module_flags) {
            return None;
        }

        if arg.starts_with('-') {
            if option_takes_value(arg) {
                let _ = args.next();
            }
            continue;
        }

        return agent_name_from_path_token(arg);
    }

    None
}

fn flag_matches(arg: &str, flags: &[&str]) -> bool {
    flags
        .iter()
        .any(|flag| arg == *flag || short_flag_payload(arg, flag) || long_flag_value(arg, flag))
}

fn short_flag_payload(arg: &str, flag: &str) -> bool {
    flag.starts_with('-')
        && !flag.starts_with("--")
        && arg.starts_with(flag)
        && arg.len() > flag.len()
}

fn long_flag_value(arg: &str, flag: &str) -> bool {
    flag.starts_with("--")
        && arg
            .strip_prefix(flag)
            .is_some_and(|rest| rest.starts_with('='))
}

fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-r" | "--require"
            | "--loader"
            | "--import"
            | "--experimental-loader"
            | "--inspect-port"
            | "-W"
            | "-X"
            | "-S"
            | "-L"
            | "-o"
    )
}

fn argv0_agent_name(argv: Option<&[String]>) -> Option<String> {
    agent_name_from_path_token(argv?.first()?)
}

pub(super) fn cmdline_argv0_agent_name(cmdline: &str) -> Option<String> {
    agent_name_from_path_token(cmdline.split_whitespace().next()?)
}

fn agent_name_from_path_token(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|c| matches!(c, '"' | '\''));
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }

    agent_name_from_basename(path_basename(trimmed))
        .or_else(|| agent_name_from_known_package_path(trimmed))
        .or_else(|| resolved_agent_name_from_path_token(trimmed))
}

fn agent_name_from_known_package_path(path: &str) -> Option<String> {
    let raw_components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect();
    let ends_with = |suffix: &[&str]| {
        raw_components.len() >= suffix.len()
            && raw_components[raw_components.len() - suffix.len()..]
                .iter()
                .zip(suffix)
                .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
    };
    if ends_with(&[
        "node_modules",
        "@earendil-works",
        "pi-coding-agent",
        "dist",
        "cli.js",
    ]) || ends_with(&[
        "node_modules",
        "@earendil-works",
        "pi-coding-agent",
        "dist",
        "bundle",
        "cli.js",
    ]) {
        return Some(agent_label(Agent::Pi).to_string());
    }

    let components: Vec<String> = raw_components
        .into_iter()
        .map(normalized_agent_lookup_name)
        .collect();
    for window in components.windows(5) {
        if window == ["node_modules", "@qwen-code", "qwen-code", "dist", "index"] {
            return Some(agent_label(Agent::Qwen).to_string());
        }
    }
    for window in components.windows(4) {
        if window == ["node_modules", "mastracode", "dist", "cli"] {
            return Some(agent_label(Agent::Mastracode).to_string());
        }
    }
    None
}

fn resolved_agent_name_from_path_token(token: &str) -> Option<String> {
    let path = std::path::Path::new(token);
    if path.components().count() < 2 {
        return None;
    }

    let resolved = std::fs::canonicalize(path).ok()?;
    let basename = resolved.file_name()?.to_str()?;
    agent_name_from_basename(basename)
}

fn agent_name_from_basename(basename: &str) -> Option<String> {
    let agent = parse_agent_label(basename)?;
    Some(agent_label(agent).to_string())
}

fn process_priority(process: &crate::platform::ForegroundProcess, normalized_name: &str) -> u8 {
    let lower_name = normalized_name.to_lowercase();
    if lower_name != process.name.to_lowercase() {
        return 3;
    }
    if !is_generic_runtime_or_shell(&lower_name) {
        return 2;
    }
    1
}

fn is_generic_runtime_or_shell(name: &str) -> bool {
    let name = normalized_agent_lookup_name(path_basename(name));
    is_python_runtime(&name)
        || matches!(
            name.as_str(),
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "tmux"
                | "node"
                | "bun"
                | "cmd"
                | "powershell"
                | "pwsh"
        )
}

fn is_python_runtime(name: &str) -> bool {
    name == "python"
        || name.strip_prefix("python").is_some_and(|version| {
            !version.is_empty()
                && version
                    .split('.')
                    .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
