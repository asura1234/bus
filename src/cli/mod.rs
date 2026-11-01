//! Opt-in process edge; provider HOME/XDG settings are never changed.
use std::io;

pub(crate) mod control;
pub(crate) mod help;
pub(crate) mod launch;
mod perf;
pub(crate) mod restart;
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
        LoggingOptions, Retention, DEFAULT_MAX_LOG_BYTES, DEFAULT_RETAINED_LOG_FILES, DEV_FILTER,
        DEV_LOG_ARCHIVE, DEV_MAX_LOG_BYTES,
    };
    use tracing_subscriber::EnvFilter;

    let dev = std::env::var_os("BUS_DEV").is_some_and(|value| value == "1");
    LoggingOptions {
        filter: if dev {
            EnvFilter::new(DEV_FILTER)
        } else {
            EnvFilter::try_from_env("BUS_LOG").unwrap_or_else(|_| EnvFilter::new("bus=info"))
        },
        max_bytes: if dev {
            DEV_MAX_LOG_BYTES
        } else {
            DEFAULT_MAX_LOG_BYTES
        },
        retention: if dev {
            Retention::Archive(DEV_LOG_ARCHIVE)
        } else {
            Retention::Generations(DEFAULT_RETAINED_LOG_FILES)
        },
        dev,
    }
}

#[cfg(test)]
#[path = "tests/logging_options_test.rs"]
mod logging_options_tests;

pub(crate) fn perf_options() -> crate::utils::perf::PerfOptions {
    crate::utils::perf::PerfOptions::from_lookup(|key| std::env::var(key).ok())
}

pub(crate) fn config_override() -> Option<fn(&mut crate::utils::config::Config)> {
    bus_data_dir()
        .is_some()
        .then_some(apply_config as fn(&mut crate::utils::config::Config))
}

fn print_paths(root: &std::path::Path, base: &std::path::Path, dev: bool) -> io::Result<()> {
    help::write_stdout_line(format_args!(
        "{}",
        serde_json::json!({"data":root,"session_base":base,"dev":dev,"logs":crate::utils::paths::data_dir(),"callback_logs":root.join("callbacks/<launch-id>/hook.log"),"config":crate::utils::config::config_dir(),"state":crate::utils::config::state_dir(),"xdg_config":std::env::var("XDG_CONFIG_HOME").ok(),"xdg_state":std::env::var("XDG_STATE_HOME").ok()})
    ));
    Ok(())
}

/// Run flags reach the daemon and coordinator through the environment.
fn apply_run_environment(dev: bool, yolo: bool) {
    if dev {
        std::env::set_var("BUS_DEV", "1");
        std::env::set_var("BUS_LOG", crate::utils::logging::DEV_FILTER);
    } else {
        std::env::remove_var("BUS_DEV");
    }
    std::env::remove_var("BUS_DEV_EXISTING_SERVER");
    // Public invocations must opt in even when launched by a yolo agent.
    // The private daemon inherits this handoff without parsing the public CLI.
    if yolo {
        std::env::set_var(crate::utils::env::YOLO_ENV_VAR, "1");
    } else {
        std::env::remove_var(crate::utils::env::YOLO_ENV_VAR);
    }
}

pub(crate) fn run(args: &[String]) -> io::Result<()> {
    if let Some(text) = help::subcommand_help(args) {
        help::write_stdout_line(format_args!("{text}"));
        return Ok(());
    }
    let invocation = parse_invocation(args).map_err(io::Error::other)?;
    if matches!(invocation.action, Action::Run | Action::Resume(_)) {
        crate::utils::launch_guard::guard_launch()?;
    }
    let control_args = match &invocation.action {
        Action::Control(args) => Some(args.clone()),
        _ => None,
    };
    match run_invocation(invocation) {
        Err(error) if control_args.is_some() => {
            control::report_setup_failure(control_args.as_deref().unwrap_or_default(), error)
        }
        result => result,
    }
}

/// The session control commands target. The perf reader only reads files, so it
/// looks the session up without recording it, which would wait for the registry lock.
fn last_session(
    registry: &LocalSessionRegistry,
    action: &Action,
) -> io::Result<sessions::LocalSession> {
    let reads_perf =
        matches!(action, Action::Control(args) if args.first().is_some_and(|arg| arg == "perf"));
    if reads_perf {
        registry.last()
    } else {
        registry.resume(ResumeTarget::Last)
    }
    .map_err(io::Error::other)
}

/// A restart treats `BUS_DATA_DIR` as an explicit data-dir override only when
/// this command set it. An inherited value equal to the running server's root
/// still carries that server's session id, and that keeps the recorded session.
/// A different root, or the same root with the session id removed, is the
/// override and clears the recorded session.
fn restart_command_overrides_data_dir(
    data_dir: Option<&std::path::Path>,
    session_id: Option<&str>,
    server_root: &std::path::Path,
) -> bool {
    let Some(data_dir) = data_dir else {
        return false;
    };
    if data_dir == server_root {
        return !matches!(session_id, Some(id) if !id.is_empty());
    }
    true
}

fn restart_session(
    explicit_root: Option<&std::path::Path>,
    fallback: impl FnOnce() -> io::Result<(std::path::PathBuf, String)>,
) -> io::Result<(std::path::PathBuf, Option<String>, bool)> {
    let env_session = std::env::var("BUS_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty());
    // No data dir on this command: the registry session is the server.
    let Some(data_dir) = explicit_root else {
        let (root, id) = fallback()?;
        return Ok((root, env_session.or(Some(id)), false));
    };
    // Only a missing recorded session is an override. Invalid metadata, a
    // missing session directory, an unreadable last-session file, and a
    // registry lock error fail the restart, as they do with no data dir.
    // A successful lookup can be the global last-opened session. When this
    // command still carries BUS_SESSION_ID, that id is the server being
    // restarted. session_root drops it only when the data dir is not that
    // session's own directory.
    match fallback() {
        Ok(_) => {}
        Err(error)
            if error
                .to_string()
                .contains("No local Bus session has been recorded yet") =>
        {
            return Ok((data_dir.to_path_buf(), None, true));
        }
        Err(error) => return Err(error),
    }
    if env_session.is_some() {
        return Ok((data_dir.to_path_buf(), env_session, false));
    }
    Ok((data_dir.to_path_buf(), None, true))
}

/// A set data dir overrides only when it is not the directory of the session
/// this command inherited. Last-opened is not that session.
fn clear_restart_session_if_data_dir_is_not_its_root(
    registry: &LocalSessionRegistry,
    data_dir: Option<&std::path::Path>,
    session: &mut Option<String>,
    explicit: &mut bool,
) -> io::Result<()> {
    if *explicit {
        return Ok(());
    }
    let (Some(dir), Some(id)) = (data_dir, session.as_deref().filter(|id| !id.is_empty())) else {
        return Ok(());
    };
    let Some(root) = registry
        .root_of(id)
        .map_err(|error| restart_registry_error(error, Some(dir)))?
    else {
        return Ok(());
    };
    if restart_command_overrides_data_dir(Some(dir), Some(id), &root) {
        *explicit = true;
        *session = None;
    }
    Ok(())
}

/// Server root for a restart. A set data dir that carries a registry session
/// id belongs to that session. A missing directory or invalid record for that
/// id fails the restart and names the id. Last-opened is used only when this
/// command has no session id, or the id is not a registry id.
fn restart_fallback_session(
    registry: &LocalSessionRegistry,
    action: &Action,
    explicit_root: Option<&std::path::Path>,
) -> io::Result<(std::path::PathBuf, String)> {
    if explicit_root.is_some() {
        if let Some(id) = std::env::var("BUS_SESSION_ID")
            .ok()
            .filter(|id| !id.is_empty())
        {
            match registry.root_of(&id) {
                Ok(Some(root)) => return Ok((root, id)),
                Ok(None) => {}
                Err(error) => return Err(restart_registry_error(error, explicit_root)),
            }
        }
    }
    let session = last_session(registry, action)?;
    Ok((session.root, session.id))
}

/// A registry failure while resolving the inherited session. A missing
/// directory names that session and the data dir on this command.
fn restart_registry_error(error: String, data_dir: Option<&std::path::Path>) -> io::Error {
    let message = match data_dir {
        Some(dir) if error.contains("was not found") => format!("{error} at {}", dir.display()),
        _ => error,
    };
    io::Error::other(message)
}

fn session_root(
    invocation: &session_pick::Invocation,
    explicit_root: Option<std::path::PathBuf>,
    base: &std::path::Path,
    registry: &LocalSessionRegistry,
    restart_explicit_data_dir: &mut bool,
) -> io::Result<Option<(std::path::PathBuf, Option<String>)>> {
    let located = match &invocation.action {
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
        // With no session ever recorded there is no server to stop.
        Action::Stop
            if explicit_root.is_none()
                && matches!(
                    std::fs::symlink_metadata(base.join("last-session")),
                    Err(error) if error.kind() == io::ErrorKind::NotFound
                ) =>
        {
            help::write_stdout_line(format_args!("{}", serde_json::json!({"stopped": false})));
            return Ok(None);
        }
        // Stop targets the same session as control commands.
        Action::Control(_) | Action::Stop => match explicit_root.clone() {
            Some(root) => (root, None),
            None => {
                let session = last_session(registry, &invocation.action)?;
                (session.root, Some(session.id))
            }
        },
        // An inherited BUS_DATA_DIR is not an override when it is that session's
        // own directory. Last-opened can name a session opened later.
        Action::Restart(_) => {
            let (root, mut session, mut explicit) =
                restart_session(explicit_root.as_deref(), || {
                    restart_fallback_session(registry, &invocation.action, explicit_root.as_deref())
                })?;
            clear_restart_session_if_data_dir_is_not_its_root(
                registry,
                explicit_root.as_deref(),
                &mut session,
                &mut explicit,
            )?;
            *restart_explicit_data_dir = explicit;
            (root, session)
        }
        Action::Paths => (explicit_root.unwrap_or_else(|| base.to_path_buf()), None),
        Action::Sessions | Action::Help | Action::Tui(_) => unreachable!(),
    };
    Ok(Some(located))
}

fn bind_session_env(root: &std::path::Path, session_id: Option<&str>) -> io::Result<()> {
    std::env::set_var("BUS_DATA_DIR", root);
    match session_id {
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
    let session = [
        "bus".to_owned(),
        "--session".to_owned(),
        coordinator::DEFAULT_SESSION.to_owned(),
    ];
    crate::utils::paths::configure_from_args(&session).map_err(io::Error::other)?;
    Ok(())
}

fn run_invocation(invocation: session_pick::Invocation) -> io::Result<()> {
    let dev = invocation.dev;
    apply_run_environment(dev, invocation.yolo);
    if invocation.action == Action::Help {
        print_help(dev);
        return Ok(());
    }
    if let Action::Tui(args) = &invocation.action {
        return run_tui(dev, args);
    }

    let explicit_root = bus_data_dir();
    let env_sets_data_dir = explicit_root.is_some();
    let mut restart_explicit_data_dir = false;
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
    let Some((root, local_session_id)) = session_root(
        &invocation,
        explicit_root,
        &base,
        &registry,
        &mut restart_explicit_data_dir,
    )?
    else {
        return Ok(());
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
        require_private_root(&root)?;
    }
    let explicit_data_dir = if matches!(invocation.action, Action::Restart(_)) {
        restart_explicit_data_dir
    } else {
        env_sets_data_dir
    };
    bind_session_env(&root, local_session_id.as_deref())?;
    match invocation.action {
        // Reads files only, so it also works while the session is hung.
        Action::Control(control_args) if control_args.first().is_some_and(|arg| arg == "perf") => {
            // Perf files sit beside the session's logs (`--paths` "logs").
            perf::run(&crate::utils::paths::data_dir(), &control_args[1..])
        }
        Action::Control(control_args) => control::run(&root, &control_args),
        Action::Stop => {
            // Stopping the server closes every agent pane; an attached UI exits
            // without saving drafts, so quit it first when one is open.
            stop::run()
        }
        Action::Restart(request) => restart::run(
            &root,
            local_session_id.as_deref(),
            explicit_data_dir,
            dev,
            invocation.yolo,
            &request,
        ),
        Action::Run | Action::Resume(_) => run_session(&registry, local_session_id, dev),
        Action::Paths => print_paths(&root, &base, dev),
        Action::Sessions | Action::Help | Action::Tui(_) => unreachable!(),
    }
}

/// The TUI driver is test tooling: only the dev variant (cargo feature
/// `dev-tools`, built by `./run dev`) compiles it in, and only a `--dev`
/// invocation reaches it. The release variant has no driver code at all.
fn run_tui(dev: bool, args: &[String]) -> io::Result<()> {
    #[cfg(not(feature = "dev-tools"))]
    {
        let _ = (dev, args);
        refuse_tui("the TUI driver is only in builds with the dev-tools feature (`./run dev`); this binary was built without it")
    }
    #[cfg(feature = "dev-tools")]
    {
        if !dev {
            refuse_tui("dev tool: run `bus --dev tui ...`");
        }
        tui_driver(args)
    }
}

/// Usage errors exit 2 with one line, as the driver's own errors do.
fn refuse_tui(message: &str) -> ! {
    eprintln!("error: DEV_TOOLS_UNAVAILABLE {message}");
    std::process::exit(2)
}

/// The driver prints its own JSON and exits with its own code contract.
#[cfg(feature = "dev-tools")]
fn tui_driver(args: &[String]) -> io::Result<()> {
    std::process::exit(crate::devtools::tui::run(args))
}

/// Every session runs its control socket in the data root, and the socket
/// refuses a root that others can reach. An existing BUS_DATA_DIR keeps its
/// mode, so refuse a shared one here, before anything starts, with the fix.
fn require_private_root(root: &std::path::Path) -> io::Result<()> {
    crate::platform::ipc::validate_private_socket_directory(root).map_err(|error| {
        io::Error::other(format!(
            "BUS_DATA_DIR {} must be private to you, because every session runs its control socket there ({error}); run `chmod 700 {}`",
            root.display(),
            root.display()
        ))
    })
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
    use super::{
        parse_invocation, require_private_root, restart_command_overrides_data_dir, Action,
        Invocation, ResumeTarget, USAGE,
    };

    /// A normal session used to run without a control socket, so a shared
    /// BUS_DATA_DIR still started. Now every session runs the socket, which
    /// refuses such a root, so launch refuses it first and names the fix.
    #[cfg(unix)]
    #[test]
    fn a_normal_session_in_a_shared_data_dir_is_refused_before_it_starts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::utils::test_temp::unique_temp_path("bshared");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = require_private_root(&dir).unwrap_err().to_string();
        assert!(error.contains("chmod 700"), "{error}");
        let (tx, _rx) = std::sync::mpsc::sync_channel(0);
        assert!(
            crate::messaging::control::start(&dir, tx).is_err(),
            "the control socket refuses the shared root the check refuses"
        );
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "launch never chmods the user's directory"
        );

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        require_private_root(&dir).unwrap();
        let (tx, _rx) = std::sync::mpsc::sync_channel(0);
        let control = crate::messaging::control::start(&dir, tx);
        assert!(control.is_ok(), "a private root runs the control socket");
        drop(control);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[path = "yolo_test.rs"]
    mod yolo_tests;

    #[test]
    fn empty_arguments_mean_a_fresh_session() {
        assert_eq!(
            parse_invocation(&[]).unwrap(),
            Invocation {
                dev: false,
                yolo: false,
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
                yolo: false,
                action: Action::Resume(ResumeTarget::Id("0123456789abcdef".into())),
            }
        );
        assert_eq!(
            parse_invocation(&args(&["resume", "--last", "--dev"])).unwrap(),
            Invocation {
                dev: true,
                yolo: false,
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
                yolo: false,
                action: Action::Stop,
            }
        );
        assert_eq!(
            parse_invocation(&args(&["stop", "bus"])).unwrap_err(),
            USAGE
        );
    }

    #[test]
    fn inherited_data_dir_equal_to_the_server_keeps_the_recorded_session() {
        let root = std::path::Path::new("/tmp/bus-session");
        let other = std::path::Path::new("/tmp/other-bus");
        assert!(
            !restart_command_overrides_data_dir(Some(root), Some("abc"), root),
            "an inherited BUS_DATA_DIR equal to the server root keeps the session"
        );
        assert!(
            restart_command_overrides_data_dir(Some(root), None, root),
            "BUS_DATA_DIR set on the command without the session is an override"
        );
        assert!(
            restart_command_overrides_data_dir(Some(root), Some(""), root),
            "an empty session id does not make BUS_DATA_DIR inherited"
        );
        assert!(
            restart_command_overrides_data_dir(Some(other), Some("abc"), root),
            "a data dir other than the running server's root is an override"
        );
        assert!(
            !restart_command_overrides_data_dir(None, Some("abc"), root),
            "a restart with no BUS_DATA_DIR uses the recorded session"
        );

        let recorded = crate::utils::restart_record::RecordedLaunch {
            dev: Some(true),
            yolo: Some(true),
            explicit_data_dir: Some(false),
            session_id: Some("abc".into()),
            session_known: true,
        };
        let kept = crate::utils::restart_record::resolve_restart_launch(
            &recorded,
            None,
            &crate::utils::restart_record::LaunchOverride {
                yolo: None,
                explicit_data_dir: restart_command_overrides_data_dir(
                    Some(root),
                    Some("abc"),
                    root,
                ),
                cli_dev: true,
                cli_session_id: Some("abc".into()),
            },
        )
        .unwrap();
        assert!(kept.dev && kept.yolo);
        assert!(!kept.explicit_data_dir);
        assert_eq!(kept.session_id.as_deref(), Some("abc"));
    }

    fn with_session_env(id: &str, body: impl FnOnce()) {
        struct Restore(Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("BUS_SESSION_ID", value),
                    None => std::env::remove_var("BUS_SESSION_ID"),
                }
            }
        }
        let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
        let _restore = Restore(std::env::var("BUS_SESSION_ID").ok());
        std::env::set_var("BUS_SESSION_ID", id);
        body();
    }

    #[test]
    fn restart_session_clears_the_session_when_the_data_dir_is_not_the_server_root() {
        let base = crate::utils::test_temp::unique_temp_path("bus-restart-other");
        let id = "0123456789abcdef";
        let server = base.join("sessions").join(id);
        let other = base.join("other-data");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            server.join("session.json"),
            format!(r#"{{"version":1,"id":"{id}","created_at_ms":1}}"#),
        )
        .unwrap();
        std::fs::write(base.join("last-session"), id).unwrap();
        let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(base.clone());
        let invocation = parse_invocation(&args(&["--dev", "restart", "--confirm"])).unwrap();
        let mut explicit = false;
        let mut result = None;
        with_session_env(id, || {
            result = Some(super::session_root(
                &invocation,
                Some(other.clone()),
                &base,
                &registry,
                &mut explicit,
            ));
        });
        let (root, session) = result.expect("called").expect("restart").expect("located");
        assert_eq!(root, other);
        assert!(
            explicit,
            "a data dir other than the running server's root is an override"
        );
        assert_eq!(session, None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn restart_session_keeps_the_session_when_the_data_dir_is_the_server_root() {
        let server = std::path::PathBuf::from("/tmp/bus-recorded-server");
        let mut result = None;
        with_session_env("abc", || {
            result = Some(super::restart_session(Some(&server), || {
                Ok((server.clone(), "from-registry".into()))
            }));
        });
        let (root, session, explicit) = result.expect("called").expect("restart session");
        assert_eq!(root, server);
        assert!(!explicit);
        assert_eq!(session.as_deref(), Some("abc"));
    }

    #[test]
    fn restart_session_overrides_a_data_dir_when_no_server_is_recorded() {
        let other = std::path::PathBuf::from("/tmp/bus-other-data");
        let mut result = None;
        with_session_env("abc", || {
            result = Some(super::restart_session(Some(&other), || {
                Err(std::io::Error::other(
                    "No local Bus session has been recorded yet",
                ))
            }));
        });
        let (root, session, explicit) = result.expect("called").expect("restart session");
        assert_eq!(root, other);
        assert!(explicit);
        assert_eq!(session, None);
    }

    #[test]
    fn restart_with_a_data_dir_reports_invalid_session_metadata() {
        let base = crate::utils::test_temp::unique_temp_path("bus-restart-meta");
        let id = "0123456789abcdef";
        let server = base.join("sessions").join(id);
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("session.json"), b"{}").unwrap();
        std::fs::write(base.join("last-session"), id).unwrap();
        let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(base.clone());
        let invocation = parse_invocation(&args(&["--dev", "restart", "--confirm"])).unwrap();
        let mut explicit = false;
        let mut result = None;
        with_session_env("abc", || {
            result = Some(super::session_root(
                &invocation,
                Some(server.clone()),
                &base,
                &registry,
                &mut explicit,
            ));
        });
        let error = result
            .expect("called")
            .expect_err("invalid session metadata is not an override");
        assert!(error.to_string().contains("metadata is invalid"), "{error}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn restart_with_a_data_dir_reports_missing_inherited_session() {
        let base = crate::utils::test_temp::unique_temp_path("bus-restart-missing");
        let inherited_id = "0123456789abcdef";
        let last_opened_id = "fedcba9876543210";
        let inherited = base.join("sessions").join(inherited_id);
        let last_opened = base.join("sessions").join(last_opened_id);
        std::fs::create_dir_all(&last_opened).unwrap();
        std::fs::write(
            last_opened.join("session.json"),
            format!(r#"{{"version":1,"id":"{last_opened_id}","created_at_ms":1}}"#),
        )
        .unwrap();
        std::fs::write(base.join("last-session"), last_opened_id).unwrap();
        let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(base.clone());
        let invocation = parse_invocation(&args(&["--dev", "restart", "--confirm"])).unwrap();
        let mut explicit = false;
        let mut result = None;
        with_session_env(inherited_id, || {
            result = Some(super::session_root(
                &invocation,
                Some(inherited.clone()),
                &base,
                &registry,
                &mut explicit,
            ));
        });
        let error = result
            .expect("called")
            .expect_err("a missing inherited session must fail the restart");
        assert!(error.to_string().contains("was not found"), "{error}");
        assert!(error.to_string().contains(inherited_id), "{error}");
        assert!(
            error.to_string().contains(&inherited.display().to_string()),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(base.join("last-session"))
                .unwrap()
                .trim(),
            last_opened_id
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn restart_with_a_missing_inherited_session_directory_is_not_an_override() {
        let base = crate::utils::test_temp::unique_temp_path("bus-restart-missing");
        let missing_id = "0123456789abcdef";
        let other_id = "fedcba9876543210";
        let missing = base.join("sessions").join(missing_id);
        let other = base.join("sessions").join(other_id);
        std::fs::create_dir_all(&other).unwrap();
        let metadata = format!(r#"{{"version":1,"id":"{other_id}","created_at_ms":1}}"#);
        std::fs::write(other.join("session.json"), metadata).unwrap();
        std::fs::write(base.join("last-session"), other_id).unwrap();
        let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(base.clone());
        let invocation = parse_invocation(&args(&["--dev", "restart", "--confirm"])).unwrap();
        let mut explicit = false;
        let mut result = None;
        with_session_env(missing_id, || {
            result = Some(super::session_root(
                &invocation,
                Some(missing.clone()),
                &base,
                &registry,
                &mut explicit,
            ));
        });
        let error = result
            .expect("called")
            .expect_err("a missing inherited session directory is not an override");
        assert!(error.to_string().contains("was not found"), "{error}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn restart_session_keeps_inherited_session_when_last_session_changes() {
        let inherited = std::path::PathBuf::from("/tmp/bus-inherited-a");
        let last_opened = std::path::PathBuf::from("/tmp/bus-last-b");
        let mut result = None;
        with_session_env("abc", || {
            result = Some(super::restart_session(Some(&inherited), || {
                Ok((last_opened.clone(), "other-session".into()))
            }));
        });
        let (root, session, explicit) = result.expect("called").expect("restart session");
        assert_eq!(root, inherited);
        assert!(
            !explicit,
            "the inherited target root must not be replaced by the global last-opened root"
        );
        assert_eq!(session.as_deref(), Some("abc"));
    }

    #[test]
    fn restart_session_keeps_its_directory_when_another_session_is_last_opened() {
        let base = crate::utils::test_temp::unique_temp_path("bus-restart-multi");
        let inherited_id = "0123456789abcdef";
        let last_opened_id = "fedcba9876543210";
        let inherited = base.join("sessions").join(inherited_id);
        let last_opened = base.join("sessions").join(last_opened_id);
        std::fs::create_dir_all(&inherited).unwrap();
        std::fs::create_dir_all(&last_opened).unwrap();
        let metadata = |id: &str| format!(r#"{{"version":1,"id":"{id}","created_at_ms":1}}"#);
        std::fs::write(inherited.join("session.json"), metadata(inherited_id)).unwrap();
        std::fs::write(last_opened.join("session.json"), metadata(last_opened_id)).unwrap();
        std::fs::write(base.join("last-session"), last_opened_id).unwrap();
        let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(base.clone());
        let invocation = parse_invocation(&args(&["--dev", "restart", "--confirm"])).unwrap();
        let mut explicit = false;
        let mut result = None;
        with_session_env(inherited_id, || {
            result = Some(super::session_root(
                &invocation,
                Some(inherited.clone()),
                &base,
                &registry,
                &mut explicit,
            ));
        });
        let (root, session) = result
            .expect("called")
            .expect("inherited restart")
            .expect("located");
        assert_eq!(root, inherited);
        assert!(
            !explicit,
            "the inherited target root must not be replaced by the global last-opened root"
        );
        assert_eq!(session.as_deref(), Some(inherited_id));
        assert_eq!(
            std::fs::read_to_string(base.join("last-session"))
                .unwrap()
                .trim(),
            last_opened_id
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn restart_parses_binary_confirm_and_who() {
        let invocation = parse_invocation(&args(&[
            "--dev",
            "restart",
            "--binary",
            "/tmp/bus",
            "--confirm",
            "--as",
            "569",
            "--yolo",
        ]))
        .unwrap();
        assert!(invocation.dev && invocation.yolo);
        assert_eq!(
            invocation.action,
            Action::Restart(crate::cli::session_pick::RestartRequest {
                binary: Some("/tmp/bus".into()),
                confirm: true,
                as_who: Some("569".into()),
            })
        );
        assert_eq!(
            parse_invocation(&args(&["--dev", "restart", "--confirm"]))
                .unwrap()
                .action,
            Action::Restart(crate::cli::session_pick::RestartRequest {
                binary: None,
                confirm: true,
                as_who: None,
            })
        );
        let refused =
            parse_invocation(&args(&["restart", "--binary", "/tmp/bus", "--confirm"])).unwrap_err();
        assert!(refused.contains("bus --dev restart"), "{refused}");
        assert_eq!(
            parse_invocation(&args(&["restart", "--help"])).unwrap_err(),
            refused
        );
    }

    #[test]
    fn tui_takes_every_later_argument_and_keeps_a_leading_dev() {
        assert_eq!(
            parse_invocation(&args(&[
                "--dev", "tui", "start", "--dev", "--size", "80x24"
            ]))
            .unwrap(),
            Invocation {
                dev: true,
                yolo: false,
                action: Action::Tui(args(&["start", "--dev", "--size", "80x24"])),
            }
        );
        assert_eq!(
            parse_invocation(&args(&["tui"])).unwrap(),
            Invocation {
                dev: false,
                yolo: false,
                action: Action::Tui(vec![]),
            }
        );
    }

    #[test]
    fn dev_control_arguments_are_preserved_after_the_command() {
        assert_eq!(
            parse_invocation(&args(&["--dev", "agent", "read", "Codex1"])).unwrap(),
            Invocation {
                dev: true,
                yolo: false,
                action: Action::Control(args(&["agent", "read", "Codex1"])),
            }
        );
    }
}
