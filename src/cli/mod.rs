//! Opt-in process edge; provider HOME/XDG settings are never changed.
use std::io;

pub(crate) mod control;
pub(crate) mod help;
pub(crate) mod launch;
mod session_pick;
pub(crate) mod stop;
use help::print_help;
use session_pick::{parse_invocation, Action};
#[cfg(test)]
use session_pick::{Invocation, USAGE};

use crate::messaging::{
    coordinator, diagnostics,
    storage::{
        io as bus_io,
        sessions::{self, LocalSessionRegistry, ResumeTarget},
    },
};

use crate::utils::env::bus_data_dir;

pub(crate) fn logging_options() -> crate::utils::logging::LoggingOptions {
    use crate::utils::logging::{
        LoggingOptions, DEFAULT_MAX_LOG_BYTES, DEFAULT_RETAINED_LOG_FILES, DEV_FILTER,
    };
    use tracing_subscriber::EnvFilter;

    let dev = std::env::var_os("BUS_DEV").is_some_and(|value| value == "1");
    LoggingOptions {
        filter: if dev {
            EnvFilter::new(DEV_FILTER)
        } else {
            EnvFilter::try_from_env("BUS_LOG").unwrap_or_else(|_| EnvFilter::new("bus=info"))
        },
        max_bytes: DEFAULT_MAX_LOG_BYTES,
        retained_files: if dev { 3 } else { DEFAULT_RETAINED_LOG_FILES },
        dev,
    }
}

pub(crate) fn config_override() -> Option<fn(&mut crate::utils::config::Config)> {
    bus_data_dir()
        .is_some()
        .then_some(apply_config as fn(&mut crate::utils::config::Config))
}

pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let invocation = parse_invocation(args).map_err(io::Error::other)?;
    let dev = invocation.dev;
    if dev {
        std::env::set_var("BUS_DEV", "1");
        std::env::set_var("BUS_LOG", crate::utils::logging::DEV_FILTER);
    } else {
        std::env::remove_var("BUS_DEV");
    }
    std::env::remove_var("BUS_DEV_EXISTING_SERVER");
    if invocation.action == Action::Help {
        print_help();
        return Ok(());
    }

    let explicit_root = bus_data_dir();
    let base = sessions::default_base_dir().map_err(io::Error::other)?;
    let registry = LocalSessionRegistry::new(base.clone());
    if invocation.action == Action::Sessions {
        let sessions = registry.list().map_err(io::Error::other)?;
        let output = sessions::format_session_list(&sessions, bus_io::now_ms());
        if !output.is_empty() {
            help::write_stdout_line(format_args!("{output}"));
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
        // Stop targets the same session as control commands.
        Action::Control(_) | Action::Stop => match explicit_root {
            Some(root) => (root, None),
            None => {
                let session = registry
                    .resume(ResumeTarget::Last)
                    .map_err(io::Error::other)?;
                (session.root, Some(session.id))
            }
        },
        Action::Paths => (explicit_root.unwrap_or_else(|| base.clone()), None),
        Action::Sessions | Action::Help => unreachable!(),
    };
    if !root.is_absolute() {
        return Err(io::Error::other(
            "BUS_DATA_DIR must be an absolute directory",
        ));
    }
    if matches!(invocation.action, Action::Run) {
        // Bus's session setup would otherwise create a missing explicit root
        // with default permissions, which Bus's private control socket rejects.
        bus_io::private_dir(&root)?;
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
        coordinator::DEFAULT_SESSION.to_owned(),
    ];
    crate::utils::paths::configure_from_args(&session).map_err(io::Error::other)?;
    match invocation.action {
        Action::Control(control_args) => control::run(&root, &control_args),
        Action::Stop => {
            // Stopping the server closes every agent pane; an attached UI exits
            // without saving drafts, so quit it first when one is open.
            stop::run()
        }
        Action::Run | Action::Resume(_) => run_session(&registry, local_session_id, dev),
        Action::Paths => {
            help::write_stdout_line(format_args!(
                "{}",
                serde_json::json!({"data":root,"session_base":base,"dev":dev,"logs":crate::utils::paths::data_dir(),"callback_logs":root.join("callbacks/<launch-id>/hook.log"),"config":crate::utils::config::config_dir(),"state":crate::utils::config::state_dir(),"xdg_config":std::env::var("XDG_CONFIG_HOME").ok(),"xdg_state":std::env::var("XDG_STATE_HOME").ok()})
            ));
            Ok(())
        }
        Action::Sessions | Action::Help => unreachable!(),
    }
}

pub(crate) fn apply_config(config: &mut crate::utils::config::Config) {
    config.onboarding = Some(false);
    config.ui.sound.enabled = false;
}

fn run_session(
    registry: &LocalSessionRegistry,
    local_session_id: Option<String>,
    dev: bool,
) -> io::Result<()> {
    if let Some(id) = &local_session_id {
        eprintln!("Bus session: {id}");
    }
    // Only a read-only liveness check: never change an attached server's
    // lifecycle or release queued requests to enable diagnostics.
    if dev && launch::is_server_listening() {
        std::env::set_var("BUS_DEV_EXISTING_SERVER", "1");
        eprintln!("{}", diagnostics::EXISTING_SERVER_NOTICE);
    }
    let result = launch::auto_detect_launch(false);
    if let Some(id) = local_session_id {
        let cleanup = (|| -> Result<(), String> {
            if !registry.is_empty(&id)? {
                return Ok(());
            }
            if launch::is_server_listening() {
                stop::stop_active_server()?;
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
    fn stop_takes_no_arguments_and_accepts_dev() {
        assert_eq!(
            parse_invocation(&args(&["stop", "--dev"])).unwrap(),
            Invocation {
                dev: true,
                action: Action::Stop,
            }
        );
        assert_eq!(
            parse_invocation(&args(&["stop", "bus"])).unwrap_err(),
            USAGE
        );
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
