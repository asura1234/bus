//! Opt-in process edge; provider HOME/XDG settings are never changed.
use std::{io, path::PathBuf};

use super::local_sessions::{LocalSessionRegistry, ResumeTarget};

const USAGE: &str = "Usage: bus [--dev] [--paths | --help]\n       bus sessions\n       bus [--dev] resume <session-id>\n       bus [--dev] resume --last\n       bus assignment verify --frame FRAME";

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
    action: Action,
}

pub(crate) fn data_dir() -> Option<PathBuf> {
    std::env::var_os("BUS_DATA_DIR").map(PathBuf::from)
}

fn parse_invocation(args: &[String]) -> Result<Invocation, String> {
    let mut dev = false;
    let mut action = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dev" if !dev => {
                dev = true;
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
    Ok(Invocation {
        dev,
        action: action.unwrap_or(Action::Run),
    })
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
                action: Action::Resume(ResumeTarget::Id("0123456789abcdef".into())),
            }
        );
        assert_eq!(
            parse_invocation(&args(&["resume", "--last", "--dev"])).unwrap(),
            Invocation {
                dev: true,
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
                action: Action::Control(args(&["agent", "read", "Codex1"])),
            }
        );
    }
}
