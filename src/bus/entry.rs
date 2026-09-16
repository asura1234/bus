//! Opt-in process edge; provider HOME/XDG settings are never changed.
use std::{io, path::PathBuf};

use super::local_sessions::{LocalSessionRegistry, ResumeTarget};

const USAGE: &str = "Usage: bus [--dev] [--paths | --help]\n       bus sessions\n       bus [--dev] resume <session-id>\n       bus [--dev] resume --last\n       bus --dev --orchestrator-control [resume <session-id> | resume --last]\n       bus assignment verify --frame FRAME";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Run,
    Sessions,
    Resume(ResumeTarget),
    Paths,
    Help,
    Control(Vec<String>),
    AssignmentVerify(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Invocation {
    dev: bool,
    orchestrator_control: bool,
    action: Action,
}

pub(crate) fn data_dir() -> Option<PathBuf> {
    std::env::var_os("BUS_DATA_DIR").map(PathBuf::from)
}

fn parse_invocation(args: &[String]) -> Result<Invocation, String> {
    let mut dev = false;
    let mut orchestrator_control = false;
    let mut action = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dev" if !dev => {
                dev = true;
                index += 1;
            }
            "--orchestrator-control" if !orchestrator_control => {
                orchestrator_control = true;
                index += 1;
            }
            "--paths" if action.is_none() => {
                action = Some(Action::Paths);
                index += 1;
            }
            "--help" | "-h" if action.is_none() => {
                action = Some(Action::Help);
                index += 1;
            }
            "sessions" if action.is_none() => {
                action = Some(Action::Sessions);
                index += 1;
            }
            "resume" if action.is_none() => {
                index += 1;
                let mut target = None;
                while index < args.len() {
                    match args[index].as_str() {
                        "--dev" if !dev => dev = true,
                        "--orchestrator-control" if !orchestrator_control => {
                            orchestrator_control = true;
                        }
                        "--last" if target.is_none() => target = Some(ResumeTarget::Last),
                        value if target.is_none() && !value.starts_with('-') => {
                            target = Some(ResumeTarget::Id(value.to_owned()));
                        }
                        _ => return Err(USAGE.to_owned()),
                    }
                    index += 1;
                }
                action = Some(Action::Resume(target.ok_or_else(|| USAGE.to_owned())?));
            }
            "assignment"
                if action.is_none()
                    && args.get(index + 1).map(String::as_str) == Some("verify")
                    && args.get(index + 2).map(String::as_str) == Some("--frame")
                    && args.get(index + 3).is_some_and(|frame| !frame.is_empty())
                    && index + 4 == args.len() =>
            {
                action = Some(Action::AssignmentVerify(args[index + 3].clone()));
                index = args.len();
            }
            value if action.is_none() && !value.starts_with('-') => {
                action = Some(Action::Control(args[index..].to_vec()));
                break;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let action = action.unwrap_or(Action::Run);
    // The control boundary is a developer session edge: it never applies to a client
    // command, a listing, or a path query.
    if orchestrator_control && (!dev || !matches!(action, Action::Run | Action::Resume(_))) {
        return Err(USAGE.to_owned());
    }
    Ok(Invocation {
        dev,
        orchestrator_control,
        action,
    })
}

/// Consumes the launch token exactly once, before anything is started. A protected
/// launch fails closed; an ordinary launch never reads or disturbs the variable,
/// which an external control client still needs.
fn arm_orchestrator_control(
    enabled: bool,
) -> Result<Option<super::orchestrator_control::TokenDigest>, String> {
    if !enabled {
        return Ok(None);
    }
    super::orchestrator_control::arm_from_environment().map(Some)
}

pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let invocation = parse_invocation(args).map_err(io::Error::other)?;
    let dev = invocation.dev;
    if dev {
        std::env::set_var("BUS_DEV", "1");
        std::env::set_var("HERDR_LOG", super::diagnostics::DEV_FILTER);
    } else {
        std::env::remove_var("BUS_DEV");
    }
    std::env::remove_var("BUS_DEV_EXISTING_SERVER");
    let _capability =
        arm_orchestrator_control(invocation.orchestrator_control).map_err(io::Error::other)?;
    if invocation.action == Action::Help {
        print_help();
        return Ok(());
    }
    if let Action::AssignmentVerify(frame) = &invocation.action {
        let result = match super::trusted_assignment::verify_from_environment(frame) {
            super::trusted_assignment::Verification::Verified(record) => {
                serde_json::json!({"status":"verified","assignment":record})
            }
            super::trusted_assignment::Verification::Absent => {
                serde_json::json!({"status":"absent"})
            }
            super::trusted_assignment::Verification::Invalid { reason } => {
                serde_json::json!({"status":"invalid","reason":reason})
            }
        };
        println!("{result}");
        return Ok(());
    }

    let explicit_root = data_dir();
    let base = super::local_sessions::default_base_dir().map_err(io::Error::other)?;
    let registry = LocalSessionRegistry::new(base.clone());
    if invocation.action == Action::Sessions {
        let sessions = registry.list().map_err(io::Error::other)?;
        let output = super::local_sessions::format_session_list(&sessions, super::io::now_ms());
        if !output.is_empty() {
            println!("{output}");
        }
        return Ok(());
    }
    let (root, local_session_id) = match &invocation.action {
        Action::Run => match explicit_root {
            Some(root) => (root, None),
            None => {
                let session = registry.create().map_err(io::Error::other)?;
                (session.root, Some(session.id))
            }
        },
        Action::Resume(target) => {
            if explicit_root.is_some() {
                return Err(io::Error::other(
                    "BUS_DATA_DIR cannot be combined with bus resume",
                ));
            }
            let session = registry.resume(target.clone()).map_err(io::Error::other)?;
            (session.root, Some(session.id))
        }
        Action::Control(_) => match explicit_root {
            Some(root) => (root, None),
            None => {
                let session = registry
                    .resume(ResumeTarget::Last)
                    .map_err(io::Error::other)?;
                (session.root, Some(session.id))
            }
        },
        Action::Paths => (explicit_root.unwrap_or_else(|| base.clone()), None),
        Action::Sessions | Action::Help | Action::AssignmentVerify(_) => unreachable!(),
    };
    if !root.is_absolute() {
        return Err(io::Error::other(
            "BUS_DATA_DIR must be an absolute directory",
        ));
    }
    std::env::set_var("BUS_DATA_DIR", &root);
    match &local_session_id {
        Some(id) => std::env::set_var("BUS_SESSION_ID", id),
        None => std::env::remove_var("BUS_SESSION_ID"),
    }
    for key in [
        "HERDR_SOCKET_PATH",
        "HERDR_CLIENT_SOCKET_PATH",
        "HERDR_CONFIG_PATH",
    ] {
        std::env::remove_var(key);
    }
    let session = vec![
        "bus".to_owned(),
        "--session".to_owned(),
        super::runtime::DEFAULT_SESSION.to_owned(),
    ];
    crate::session::configure_from_args(&session).map_err(io::Error::other)?;
    match invocation.action {
        Action::Control(control_args) => super::control_cli::run(&root, &control_args),
        Action::Run | Action::Resume(_) => {
            if let Some(id) = &local_session_id {
                eprintln!("Bus session: {id}");
            }
            // Only a read-only liveness check: never change an attached server's
            // lifecycle or release queued requests to enable diagnostics.
            if dev && crate::server::autodetect::is_server_listening() {
                std::env::set_var("BUS_DEV_EXISTING_SERVER", "1");
                eprintln!("{}", super::diagnostics::EXISTING_SERVER_NOTICE);
            }
            let result = crate::server::autodetect::auto_detect_launch(false);
            if let Some(id) = local_session_id {
                let cleanup = (|| -> Result<(), String> {
                    if !registry.is_empty(&id)? {
                        return Ok(());
                    }
                    if crate::server::autodetect::is_server_listening() {
                        crate::session::stop_active_server()?;
                    }
                    registry.discard_if_empty(&id)?;
                    Ok(())
                })();
                if result.is_ok() {
                    cleanup.map_err(io::Error::other)?;
                }
            }
            result
        }
        Action::Paths => {
            println!(
                "{}",
                serde_json::json!({"data":root,"session_base":base,"dev":dev,"logs":crate::session::data_dir(),"callback_logs":root.join("callbacks/<launch-id>/hook.log"),"config":crate::config::config_dir(),"state":crate::config::state_dir(),"xdg_config":std::env::var("XDG_CONFIG_HOME").ok(),"xdg_state":std::env::var("XDG_STATE_HOME").ok()})
            );
            Ok(())
        }
        Action::Sessions | Action::Help | Action::AssignmentVerify(_) => unreachable!(),
    }
}

fn print_help() {
    println!("{}\n", super::control_cli::HELP);
    println!("Bus — coordinate selected agents in native terminal rooms\n\n{USAGE}\n\nA plain `bus` launch always creates a new local session.\n`bus sessions` lists resumable sessions, their rooms, and recent activity.\n`bus resume <session-id>` resumes that exact session.\n`bus resume --last` resumes the last opened session.\n--dev enables developer log files, excluding input/content dumps.\nExisting servers keep their original log level; they are never automatically restarted.\n--paths shows data and log directories without starting a session.\nBUS_DATA_DIR is an exact isolated-root override for development and tests; it cannot be combined with resume.\n\nCtrl+Shift+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+A/E line start/end · Ctrl+R history search · Ctrl+Shift+E composer size\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved.");
}

pub(crate) fn apply_config(config: &mut crate::config::Config) {
    config.onboarding = Some(false);
    config.update.version_check = false;
    config.update.manifest_check = false;
    config.ui.sound.enabled = false;
}

#[cfg(test)]
mod tests {
    use super::{parse_invocation, Action, Invocation, ResumeTarget, USAGE};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn empty_arguments_mean_a_fresh_session() {
        assert_eq!(
            parse_invocation(&[]).unwrap(),
            Invocation {
                dev: false,
                orchestrator_control: false,
                action: Action::Run,
            }
        );
    }

    #[test]
    fn room_orchestrator_control_flag_is_opt_in_for_dev_run_and_resume() {
        assert_eq!(
            parse_invocation(&args(&["--dev", "--orchestrator-control"])).unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: true,
                action: Action::Run,
            }
        );
        assert_eq!(
            parse_invocation(&args(&[
                "--dev",
                "--orchestrator-control",
                "resume",
                "--last"
            ]))
            .unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: true,
                action: Action::Resume(ResumeTarget::Last),
            }
        );
        assert_eq!(
            parse_invocation(&args(&[
                "--dev",
                "resume",
                "--last",
                "--orchestrator-control"
            ]))
            .unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: true,
                action: Action::Resume(ResumeTarget::Last),
            }
        );
    }

    #[test]
    fn room_orchestrator_control_flag_requires_dev_and_a_session_action() {
        for invalid in [
            args(&["--orchestrator-control"]),
            args(&["--orchestrator-control", "resume", "--last"]),
            args(&["--dev", "--orchestrator-control", "--orchestrator-control"]),
            args(&["--dev", "--orchestrator-control", "sessions"]),
            args(&["--dev", "--orchestrator-control", "--paths"]),
            args(&["--dev", "--orchestrator-control", "agent", "read", "Codex1"]),
        ] {
            assert_eq!(
                parse_invocation(&invalid).unwrap_err(),
                USAGE,
                "accepted {invalid:?}"
            );
        }
    }

    const CONTROL_TOKEN: &str = "Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhKZGtDeQ";

    #[test]
    fn room_orchestrator_control_launch_hashes_the_token_and_scrubs_the_raw_secret() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let variable = super::super::orchestrator_control::TOKEN_ENV_VAR;
        std::env::set_var(variable, CONTROL_TOKEN);

        let digest = super::arm_orchestrator_control(true)
            .unwrap()
            .expect("launch capability");

        assert_eq!(
            digest.hex(),
            super::super::io::digest(CONTROL_TOKEN.as_bytes())
        );
        assert!(
            std::env::var_os(variable).is_none(),
            "raw token survived launch"
        );
    }

    #[test]
    fn room_orchestrator_control_launch_fails_closed_on_a_missing_or_weak_token() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let variable = super::super::orchestrator_control::TOKEN_ENV_VAR;
        std::env::remove_var(variable);
        assert!(super::arm_orchestrator_control(true).is_err());

        std::env::set_var(variable, "short-and-guessable");
        assert!(super::arm_orchestrator_control(true).is_err());
        assert!(
            std::env::var_os(variable).is_none(),
            "a rejected token must still be scrubbed"
        );
    }

    #[test]
    fn room_orchestrator_control_legacy_launch_never_reads_or_scrubs_the_token() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let variable = super::super::orchestrator_control::TOKEN_ENV_VAR;
        std::env::set_var(variable, CONTROL_TOKEN);

        assert!(super::arm_orchestrator_control(false).unwrap().is_none());

        assert_eq!(std::env::var(variable).unwrap(), CONTROL_TOKEN);
        std::env::remove_var(variable);
    }

    #[test]
    fn room_orchestrator_control_absent_flag_keeps_legacy_dev_behavior() {
        assert_eq!(
            parse_invocation(&args(&["--dev"])).unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: false,
                action: Action::Run,
            }
        );
    }

    #[test]
    fn resume_accepts_an_exact_id_or_last_with_dev_on_either_side() {
        assert_eq!(
            parse_invocation(&args(&["--dev", "resume", "0123456789abcdef"])).unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: false,
                action: Action::Resume(ResumeTarget::Id("0123456789abcdef".into())),
            }
        );
        assert_eq!(
            parse_invocation(&args(&["resume", "--last", "--dev"])).unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: false,
                action: Action::Resume(ResumeTarget::Last),
            }
        );
    }

    #[test]
    fn resume_requires_exactly_one_selector() {
        for invalid in [
            args(&["resume"]),
            args(&["resume", "--last", "0123456789abcdef"]),
            args(&["resume", "--unknown"]),
        ] {
            assert_eq!(parse_invocation(&invalid).unwrap_err(), USAGE);
        }
    }

    #[test]
    fn dev_control_arguments_are_preserved_after_the_command() {
        assert_eq!(
            parse_invocation(&args(&["--dev", "agent", "read", "Codex1"])).unwrap(),
            Invocation {
                dev: true,
                orchestrator_control: false,
                action: Action::Control(args(&["agent", "read", "Codex1"])),
            }
        );
    }
}
