//! Provider argv and explicitly consented observation-hook configuration.
use super::{
    callbacks::Manifest,
    model::{AgentId, Provider},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(crate) struct AddAgent {
    pub(crate) room: super::model::RoomId,
    pub(crate) name: String,
    pub(crate) provider: Provider,
    pub(crate) cwd: String,
    pub(crate) extra_args: String,
    pub(crate) consent_project_hooks: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SetupNotice {
    pub(crate) path: PathBuf,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PathSuggestion {
    pub(crate) path: PathBuf,
    pub(crate) is_directory: bool,
}

pub(crate) struct PreparedLaunch {
    pub(crate) cwd: PathBuf,
    pub(crate) args: Vec<String>,
    /// The existing provider session this launch resumes instead of starting one.
    pub(crate) adopted_session: Option<String>,
    pub(crate) env: HashMap<String, String>,
    pub(crate) manifest: Manifest,
}

pub(crate) fn provider_kind(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex",
        Provider::ClaudeCode => "claude",
        Provider::Cursor => "cursor",
    }
}

pub(crate) fn executable(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "codex",
        Provider::ClaudeCode => "claude",
        Provider::Cursor => "cursor-agent",
    }
}

/// Bus-owned provider arguments shared by fresh launches and native resumes.
pub(crate) fn runtime_args(provider: Provider) -> Vec<String> {
    match provider {
        // Codex otherwise attaches to a shared app-server daemon that runs hooks
        // with the env of whichever pane first started it, so BUS_LAUNCH_ID and
        // BUS_CALLBACK_DIR would route every agent's callbacks to that launch.
        Provider::Codex => vec!["--no-daemon".into()],
        Provider::ClaudeCode | Provider::Cursor => Vec::new(),
    }
}

pub(crate) fn canonical_directory(input: &str) -> Result<PathBuf, String> {
    let path = if input == "~" || input.starts_with("~/") {
        let home = std::env::home_dir().ok_or("Home directory unavailable")?;
        if input == "~" {
            home
        } else {
            home.join(&input[2..])
        }
    } else {
        PathBuf::from(input)
    };
    if !path.is_absolute() {
        return Err("Agent PWD must be an absolute path or ~/ path".into());
    }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.is_dir() {
        return Err("Agent PWD must be an existing directory".into());
    }
    Ok(path)
}

/// Takes the reviewed resume form out of the user's launch args so the agent
/// adopts an existing provider session: `--resume ID` for Claude Code and
/// Cursor, a leading `resume ID` subcommand for Codex. The ID must be a session
/// UUID. Pickers and `--continue`/`--last` stay outside the allowlist, so the
/// remaining args still go through [`validate_args`].
pub(crate) fn take_adopted_session(
    provider: Provider,
    args: &mut Vec<String>,
) -> Result<Option<String>, String> {
    let mut found = None;
    let mut index = 0;
    while index < args.len() {
        let attached = match provider {
            // A subcommand only counts first; later it may be an option value.
            Provider::Codex if index == 0 && args[0] == "resume" => None,
            Provider::Codex => break,
            Provider::ClaudeCode | Provider::Cursor if args[index] == "--resume" => None,
            Provider::ClaudeCode | Provider::Cursor => {
                match args[index].strip_prefix("--resume=") {
                    Some(value) => Some(value.to_owned()),
                    None => {
                        index += 1;
                        continue;
                    }
                }
            }
        };
        if found.is_some() {
            return Err("Give at most one session to resume".into());
        }
        args.remove(index);
        let value = match attached {
            Some(value) => value,
            None if index < args.len() => args.remove(index),
            None => String::new(),
        };
        if !is_session_uuid(&value) {
            return Err(format!(
                "Resume needs a session ID (a UUID), not {value:?}; pickers, --continue and --last are not supported"
            ));
        }
        found = Some(value);
    }
    Ok(found)
}

/// The adopted session in an agent's launch args, without the other checks.
pub(crate) fn adopted_session(
    provider: Provider,
    extra_args: &str,
) -> Result<Option<String>, String> {
    let mut args = super::files::parse_path_tokens(extra_args).map_err(|e| e.to_string())?;
    take_adopted_session(provider, &mut args)
}

fn is_session_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(index, c)| match index {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The user's validated launch args with Bus's runtime args, led by the resume
/// form when the agent adopts an existing session.
fn launch_args(
    provider: Provider,
    extra_args: &str,
) -> Result<(Vec<String>, Option<String>), String> {
    let mut args = super::files::parse_path_tokens(extra_args).map_err(|e| e.to_string())?;
    let adopted = take_adopted_session(provider, &mut args)?;
    validate_args(provider, &args)?;
    if let Some(session) = &adopted {
        args.splice(0..0, resume_args(provider, session));
    }
    args.extend(runtime_args(provider));
    Ok((args, adopted))
}

/// Holds a launch's adopted session in its callback folder from launch on, so
/// no second agent adopts it before the provider's SessionStart binds it.
const ADOPTED_SESSION_FILE: &str = "adopted-session";

/// The session a launch adopted, from its callback folder.
pub(crate) fn reserved_session(spool: &Path) -> Option<String> {
    std::fs::read_to_string(spool.join(ADOPTED_SESSION_FILE))
        .ok()
        .map(|session| session.trim().to_owned())
        .filter(|session| !session.is_empty())
}

/// The provider argv that resumes `session`; Codex's is a leading subcommand.
fn resume_args(provider: Provider, session: &str) -> Vec<String> {
    match provider {
        Provider::Codex => vec!["resume".into(), session.into()],
        Provider::ClaudeCode | Provider::Cursor => vec!["--resume".into(), session.into()],
    }
}

enum LaunchOption {
    Flag,
    Value,
    Choice(&'static [&'static str]),
}

fn supported_option(provider: Provider, name: &str) -> Option<LaunchOption> {
    use LaunchOption::{Choice, Flag, Value};
    // These are explicit interactive options verified against the installed
    // provider help. New flags/subcommands fail closed until their semantics
    // are reviewed; a blacklist cannot cover equivalent aliases or new modes.
    match (provider, name) {
        (_, "--model") | (Provider::Codex, "-m") => Some(Value),
        (Provider::Codex, "--no-alt-screen" | "--strict-config" | "--oss" | "--search") => {
            Some(Flag)
        }
        (Provider::Codex, "--local-provider") => Some(Choice(&["ollama", "lmstudio"])),
        (Provider::ClaudeCode, "--verbose" | "--ax-screen-reader") => Some(Flag),
        (Provider::ClaudeCode, "--system-prompt" | "--append-system-prompt") => Some(Value),
        (Provider::ClaudeCode, "--effort") => {
            Some(Choice(&["low", "medium", "high", "xhigh", "max"]))
        }
        (Provider::Cursor, "--plan") => Some(Flag),
        (Provider::Cursor, "--mode") => Some(Choice(&["plan", "ask"])),
        _ => None,
    }
}

fn validate_args(provider: Provider, args: &[String]) -> Result<(), String> {
    let mut tokens = args.iter();
    while let Some(arg) = tokens.next() {
        if arg.contains('\0') {
            return Err("Launch arguments cannot contain NUL".into());
        }
        // Only Codex advertises -m. Support its separated and attached value
        // forms without accepting other short options or option clusters.
        let (name, attached) = if provider == Provider::Codex && arg.starts_with("-m") {
            let rest = &arg[2..];
            (
                "-m",
                (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest)),
            )
        } else {
            arg.split_once('=')
                .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)))
        };
        let kind = supported_option(provider, name).ok_or_else(|| {
            format!("Unsupported {} launch argument {name}. Bus accepts only supported interactive model/display options, not subcommands or permission, hook, configuration, or workspace overrides.", provider_kind(provider))
        })?;
        if matches!(kind, LaunchOption::Flag) {
            if attached.is_some() {
                return Err(format!("Launch option {name} does not take a value"));
            }
            continue;
        }
        let value = attached
            .or_else(|| tokens.next().map(String::as_str))
            .filter(|value| !value.is_empty() && !value.starts_with('-') && !value.contains('\0'))
            .ok_or_else(|| {
                format!("Launch option {name} requires a nonempty value, not another option")
            })?;
        if let LaunchOption::Choice(choices) = kind {
            if !choices.contains(&value) {
                return Err(format!(
                    "Launch option {name} must be one of: {}",
                    choices.join(", ")
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn setup_notice(provider: Provider, cwd: &Path) -> Option<SetupNotice> {
    let root = project_root(cwd);
    let cwd = root.as_path();
    match provider {
        Provider::ClaudeCode => None,
        Provider::Codex => Some(SetupNotice { path: cwd.join(".codex/hooks.json"), message: "Add Bus-owned SessionStart, UserPromptSubmit and Stop observation hooks. Trust this project and all three definitions in Codex. Bus enables room delivery automatically when the terminal is ready. Codex emits SessionStart with the first room prompt; no priming message or restart is needed. Existing hooks/notify remain. Cleanup: remove only entries whose command is this Bus executable plus --bus-callback codex-hook.".into() }),
        Provider::Cursor => Some(SetupNotice { path: cwd.join(".cursor/hooks.json"), message: "Add Bus-owned sessionStart, beforeSubmitPrompt, afterAgentResponse and stop observation hooks. Complete normal Cursor trust/setup. Bus enables room delivery automatically when the terminal is ready; queued prompts wait until then. Cleanup: remove only entries whose command is this Bus executable plus --bus-callback cursor-hook.".into() }),
    }
}

fn project_root(cwd: &Path) -> PathBuf {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        }
        _ => cwd.to_path_buf(),
    }
}

fn hook_command(binary: &Path, provider: Provider) -> String {
    // Hook definitions are shell commands; provider launch arguments never are.
    let quoted = crate::platform::remote_reattach_program(&binary.to_string_lossy());
    let adapter = match provider {
        Provider::Codex => "codex-hook",
        Provider::ClaudeCode => "claude-hook",
        Provider::Cursor => "cursor-hook",
    };
    format!("{quoted} --bus-callback {adapter}")
}

fn hook_entries(provider: Provider, binary: &Path) -> Vec<(&'static str, Value)> {
    let command = hook_command(binary, provider);
    let events: &[&str] = match provider {
        Provider::Codex => &["SessionStart", "UserPromptSubmit", "Stop"],
        Provider::ClaudeCode => &["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"],
        Provider::Cursor => &[
            "sessionStart",
            "beforeSubmitPrompt",
            "afterAgentResponse",
            "stop",
        ],
    };
    events
        .iter()
        .map(|event| {
            (
                *event,
                if provider == Provider::Cursor {
                    json!({"command":command})
                } else {
                    json!({"hooks":[{"type":"command","command":command,"timeout":5}]})
                },
            )
        })
        .collect()
}

fn merge_hooks(mut document: Value, provider: Provider, binary: &Path) -> Result<Value, String> {
    let root = document
        .as_object_mut()
        .ok_or("Hook config must be a JSON object")?;
    if provider == Provider::Cursor {
        root.entry("version").or_insert(json!(1));
    }
    let hooks = root
        .entry("hooks")
        .or_insert(json!({}))
        .as_object_mut()
        .ok_or("hooks must be a JSON object")?;
    for (event, owned) in hook_entries(provider, binary) {
        let group = hooks
            .entry(event)
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or("Hook event must contain an array")?;
        if group.contains(&owned) {
            continue;
        }
        if group
            .iter()
            .any(|entry| entry.to_string().contains("--bus-callback"))
        {
            return Err(format!(
                "Conflicting Bus hook in {event}; inspect/remove that owned entry before setup"
            ));
        }
        group.push(owned);
    }
    Ok(document)
}

pub(crate) fn install_hooks(path: &Path, provider: Provider, binary: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing hook config parent")?;
    super::io::private_dir(parent).map_err(|e| e.to_string())?;
    let _lease = super::io::lock(&parent.join(".bus-hooks.lock")).map_err(|e| e.to_string())?;
    let previous = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let document = match previous.as_ref() {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|e| e.to_string())?,
        None => json!({}),
    };
    let merged = merge_hooks(document, provider, binary)?;
    let current = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if current != previous {
        return Err("Hook config changed during setup; retry confirmation".into());
    }
    super::io::atomic_write(
        path,
        &serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn prepare(
    input: &AddAgent,
    agent_id: AgentId,
    data_dir: &Path,
    binary: &Path,
) -> Result<PreparedLaunch, String> {
    let cwd = canonical_directory(&input.cwd)?;
    if input.name.trim().is_empty() {
        return Err("Agent name is required".into());
    }
    let (mut args, adopted_session) = launch_args(input.provider, &input.extra_args)?;
    let available = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join(executable(input.provider)).is_file())
    });
    if !available {
        return Err(format!(
            "{} is not installed on PATH",
            executable(input.provider)
        ));
    }
    if let Some(notice) = setup_notice(input.provider, &cwd) {
        if !input.consent_project_hooks {
            return Err(format!(
                "Setup confirmation required for {}. {}",
                notice.path.display(),
                notice.message
            ));
        }
        install_hooks(&notice.path, input.provider, binary)?;
    }
    let launch_id = super::io::digest(
        format!(
            "{}:{}:{}",
            agent_id.0,
            std::process::id(),
            super::io::now_ns()
        )
        .as_bytes(),
    );
    let manifest = Manifest {
        agent_id,
        provider: input.provider,
        launch_id: launch_id.clone(),
    };
    let spool = data_dir.join("callbacks").join(&launch_id);
    super::callbacks::initialize(&spool, &manifest).map_err(|e| e.to_string())?;
    if let Some(session) = &adopted_session {
        super::io::atomic_write(&spool.join(ADOPTED_SESSION_FILE), session.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    if input.provider == Provider::ClaudeCode {
        let settings = spool.join("claude-settings.json");
        install_hooks(&settings, input.provider, binary)?;
        super::usage::claude_statusline::install(&spool, &project_root(&cwd), binary)
            .map_err(|e| e.to_string())?;
        args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
    }
    let env = HashMap::from([
        ("BUS_LAUNCH_ID".into(), launch_id),
        (
            "BUS_CALLBACK_DIR".into(),
            spool.to_string_lossy().into_owned(),
        ),
    ]);
    Ok(PreparedLaunch {
        cwd,
        args,
        adopted_session,
        env,
        manifest,
    })
}

/// Real filesystem suggestions; caller supplies a query ID and never renders IO.
pub(crate) fn suggestions(
    input: &str,
    directories_only: bool,
) -> Result<Vec<PathSuggestion>, String> {
    let expanded = if let Some(rest) = input.strip_prefix("~/") {
        std::env::home_dir()
            .ok_or("Home directory unavailable")?
            .join(rest)
    } else {
        PathBuf::from(input)
    };
    if !expanded.is_absolute() {
        return Ok(Vec::new());
    }
    let (parent, prefix) = if input.ends_with('/') {
        (expanded.as_path(), String::new())
    } else {
        (
            expanded.parent().ok_or("Missing directory")?,
            expanded
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    };
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        let is_directory = metadata.is_dir();
        if is_directory || (!directories_only && metadata.is_file()) {
            paths.push(PathSuggestion { path, is_directory });
        }
        if paths.len() == 200 {
            break;
        }
    }
    paths.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_args_cannot_replace_hooks_or_bypass_trust() {
        for (provider, arg) in [
            (
                Provider::Codex,
                "--dangerously-bypass-approvals-and-sandbox",
            ),
            (Provider::ClaudeCode, "--settings"),
            (Provider::Cursor, "--force"),
        ] {
            assert!(validate_args(provider, &[arg.into()]).is_err(), "{arg}");
        }
        assert!(validate_args(Provider::Codex, &["--model".into(), "gpt-test".into()]).is_ok());
    }

    #[test]
    fn provider_launch_options_reject_safeguard_aliases_and_noninteractive_commands() {
        for (provider, inputs) in [
            (
                Provider::Codex,
                vec![
                    "-a never -s danger-full-access",
                    "-anever",
                    "-a=never",
                    "-sdanger-full-access",
                    "--ask-for-approval=never",
                    "--sandbox=danger-full-access",
                    "e",
                    "review",
                    "--model test e",
                    "--model=test review",
                    "-p untrusted-profile",
                    "-cfeatures.hooks=false",
                    "--remote unix:///tmp/other.sock",
                    "--unknown-option=value",
                    "-- review",
                ],
            ),
            (
                Provider::ClaudeCode,
                vec![
                    "--bare",
                    "--safe-mode",
                    "--bg",
                    "--background",
                    "--dangerously-skip-permissions",
                    "--allowed-tools Bash",
                    "--settings=/tmp/hooks.json",
                    "-phello",
                    "--print=hello",
                    "review",
                    "--unknown-option=value",
                ],
            ),
            (
                Provider::Cursor,
                vec![
                    "-f",
                    "--force",
                    "--yolo",
                    "--auto-review",
                    "--approve-mcps",
                    "--sandbox=disabled",
                    "--trust",
                    "--plugin-dir=/tmp/hooks",
                    "-phello",
                    "worker",
                    "--unknown-option=value",
                ],
            ),
        ] {
            for input in inputs {
                let args = super::super::files::parse_path_tokens(input).unwrap();
                assert!(
                    validate_args(provider, &args).is_err(),
                    "accepted {provider:?}: {input}"
                );
            }
        }
    }

    #[test]
    fn provider_launch_options_preserve_supported_interactive_values_and_reject_ambiguous_syntax() {
        for (provider, input) in [
            (Provider::Codex, "-m gpt-test --no-alt-screen --strict-config"),
            (Provider::Codex, "-mgpt-test --oss --local-provider=ollama"),
            (Provider::Codex, "-m=gpt-test --search"),
            (Provider::ClaudeCode, "--model=sonnet --effort high --verbose --append-system-prompt 'Use literal @mentions and paths'"),
            (Provider::Cursor, "--model 'sonnet[effort=high]' --mode=plan"),
            (Provider::Cursor, "--plan"),
        ] {
            let args = super::super::files::parse_path_tokens(input).unwrap();
            assert!(validate_args(provider, &args).is_ok(), "rejected {provider:?}: {input}");
        }
        for (provider, input) in [
            (Provider::Codex, "--model --ask-for-approval=never"),
            (Provider::Codex, "--model="),
            (Provider::Codex, "--model"),
            (Provider::Codex, "--no-alt-screen=review"),
            (Provider::Codex, "--local-provider=unknown"),
            (Provider::ClaudeCode, "-m sonnet"),
            (Provider::ClaudeCode, "--effort=unknown"),
            (Provider::Cursor, "--mode=agent"),
            (Provider::Cursor, "--model sonnet review"),
            (Provider::Cursor, "--model=sonnet\0--force"),
        ] {
            let args = super::super::files::parse_path_tokens(input).unwrap();
            assert!(
                validate_args(provider, &args).is_err(),
                "accepted {provider:?}: {input}"
            );
        }
    }

    #[test]
    fn launch_args_adopt_one_session_by_uuid_in_each_providers_resume_form() {
        let id = "160d1f8b-9023-44b8-9bc7-24333effb185";
        let owned = |args: &[&str]| args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            launch_args(
                Provider::ClaudeCode,
                &format!("--model sonnet --resume {id}")
            )
            .unwrap(),
            (
                owned(&["--resume", id, "--model", "sonnet"]),
                Some(id.into())
            )
        );
        assert_eq!(
            launch_args(Provider::Cursor, &format!("--resume={id}")).unwrap(),
            (owned(&["--resume", id]), Some(id.into()))
        );
        assert_eq!(
            launch_args(Provider::Codex, &format!("resume {id} -m gpt-test")).unwrap(),
            (
                owned(&["resume", id, "-m", "gpt-test", "--no-daemon"]),
                Some(id.into())
            )
        );
        // A later "resume" is an option value, not the subcommand.
        assert_eq!(
            launch_args(Provider::Codex, "--model resume").unwrap(),
            (owned(&["--model", "resume", "--no-daemon"]), None)
        );
        for (provider, input) in [
            (Provider::ClaudeCode, "--resume"),
            (Provider::ClaudeCode, "--resume not-a-uuid"),
            (
                Provider::ClaudeCode,
                "--resume 160d1f8b-9023-44b8-9bc7-24333effb18",
            ),
            (
                Provider::ClaudeCode,
                "--resume 160d1f8b_9023-44b8-9bc7-24333effb185",
            ),
            (
                Provider::ClaudeCode,
                &format!("--resume {id} --resume {id}"),
            ),
            (Provider::ClaudeCode, &format!("--resume {id} --continue")),
            (
                Provider::ClaudeCode,
                &format!("--resume {id} --fork-session"),
            ),
            (
                Provider::ClaudeCode,
                "-r 160d1f8b-9023-44b8-9bc7-24333effb185",
            ),
            (Provider::ClaudeCode, "--continue"),
            (Provider::ClaudeCode, &format!("--session-id {id}")),
            (Provider::Codex, "resume --last"),
            (Provider::Codex, "resume"),
            (Provider::Codex, &format!("resume {id} resume {id}")),
            (Provider::Codex, &format!("-m gpt resume {id}")),
            (Provider::Codex, &format!("resume {id} --all")),
            (Provider::Cursor, "--resume --continue"),
            (Provider::Cursor, "--continue"),
        ] {
            assert!(
                launch_args(provider, input).is_err(),
                "accepted {provider:?}: {input}"
            );
        }
    }

    #[test]
    fn codex_launches_host_their_own_runtime_so_hooks_inherit_launch_env() {
        assert_eq!(
            runtime_args(Provider::Codex),
            vec!["--no-daemon".to_owned()]
        );
        assert!(runtime_args(Provider::ClaudeCode).is_empty());
        assert!(runtime_args(Provider::Cursor).is_empty());
        // Bus owns this flag; a user-supplied copy stays outside the allowlist.
        assert!(validate_args(Provider::Codex, &["--no-daemon".into()]).is_err());
    }

    #[test]
    fn hook_merge_preserves_existing_json_is_idempotent_and_conflicts_fail_closed() {
        let binary = Path::new("/tmp/a path/it's bus");
        let original = json!({"other":true,"hooks":{"Stop":[{"hooks":[{"type":"command","command":"existing"}]}]}});
        let merged = merge_hooks(original.clone(), Provider::Codex, binary).unwrap();
        assert_eq!(merged["other"], true);
        assert_eq!(merged["hooks"]["Stop"][0], original["hooks"]["Stop"][0]);
        assert_eq!(
            merge_hooks(merged.clone(), Provider::Codex, binary).unwrap(),
            merged
        );
        assert!(merge_hooks(merged, Provider::Codex, Path::new("/tmp/other-bus")).is_err());
        assert!(hook_command(binary, Provider::Codex).starts_with("'/tmp/a path/it'\\''s bus' "));
    }

    #[test]
    fn project_hook_install_preserves_unrelated_entries_and_refuses_corrupt_files() {
        let dir =
            std::env::temp_dir().join(format!("bus-hook-install-{}", super::super::io::now_ns()));
        super::super::io::private_dir(&dir).unwrap();
        let path = dir.join("hooks.json");
        let original = json!({"custom":42,"hooks":{"stop":[{"command":"existing"}]}});
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let binary = Path::new("/tmp/bus fixture");
        install_hooks(&path, Provider::Cursor, binary).unwrap();
        let installed = std::fs::read(&path).unwrap();
        let document: Value = serde_json::from_slice(&installed).unwrap();
        assert_eq!(document["custom"], 42);
        assert_eq!(document["hooks"]["stop"][0], original["hooks"]["stop"][0]);
        assert_eq!(document["hooks"]["stop"].as_array().unwrap().len(), 2);
        assert_eq!(
            document["hooks"]["sessionStart"].as_array().unwrap().len(),
            1
        );
        install_hooks(&path, Provider::Cursor, binary).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), installed);
        assert!(install_hooks(&path, Provider::Cursor, Path::new("/tmp/different bus")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), installed);
        std::fs::write(&path, b"corrupt original").unwrap();
        assert!(install_hooks(&path, Provider::Cursor, binary).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt original");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn suggestions_report_directory_kind_without_ui_filesystem_reads() {
        let dir =
            std::env::temp_dir().join(format!("bus-suggestions-{}", super::super::io::now_ns()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(dir.join("folder")).unwrap();
        std::fs::write(dir.join("file.md"), b"fixture").unwrap();
        let input = format!("{}/", dir.display());
        let entries = suggestions(&input, false).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            !entries
                .iter()
                .find(|entry| entry.path == dir.join("file.md"))
                .unwrap()
                .is_directory
        );
        assert!(
            entries
                .iter()
                .find(|entry| entry.path == dir.join("folder"))
                .unwrap()
                .is_directory
        );
        let directories = suggestions(&input, true).unwrap();
        assert_eq!(directories.len(), 1);
        assert_eq!(directories[0].path, dir.join("folder"));
        assert!(directories[0].is_directory);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
