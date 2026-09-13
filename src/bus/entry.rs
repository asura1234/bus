//! Opt-in process edge; provider HOME/XDG settings are never changed.
use std::{io, path::PathBuf};

pub(crate) fn data_dir() -> Option<PathBuf> {
    std::env::var_os("BUS_DATA_DIR").map(PathBuf::from)
}

pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let (mut dev, mut action) = (false, "run");
    let mut control_args = None;
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "--dev" if !dev => dev = true,
            "--paths" if action == "run" => action = "paths",
            "--help" | "-h" if action == "run" => action = "help",
            _ if action == "run" && !arg.starts_with('-') => {
                action = "control";
                control_args = Some(&args[index..]);
                break;
            }
            _ => return Err(io::Error::other("Usage: bus [--dev] [--paths | --help]")),
        }
    }
    if dev {
        std::env::set_var("BUS_DEV", "1");
        std::env::set_var("HERDR_LOG", super::diagnostics::DEV_FILTER);
    } else {
        std::env::remove_var("BUS_DEV");
    }
    std::env::remove_var("BUS_DEV_EXISTING_SERVER");
    let root = data_dir()
        .map(Ok)
        .unwrap_or_else(super::runtime::default_data_dir)
        .map_err(io::Error::other)?;
    if !root.is_absolute() {
        return Err(io::Error::other(
            "BUS_DATA_DIR must be an absolute directory",
        ));
    }
    std::env::set_var("BUS_DATA_DIR", &root);
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
    match action {
        "control" => super::control_cli::run(&root, control_args.unwrap_or_default()),
        "run" => {
            // Only a read-only liveness check: never change an attached server's
            // lifecycle or release queued requests to enable diagnostics.
            if dev && crate::server::autodetect::is_server_listening() {
                std::env::set_var("BUS_DEV_EXISTING_SERVER", "1");
                eprintln!("{}", super::diagnostics::EXISTING_SERVER_NOTICE);
            }
            crate::server::autodetect::auto_detect_launch(false)
        }
        "paths" => {
            println!(
                "{}",
                serde_json::json!({"data":root,"dev":dev,"logs":crate::session::data_dir(),"callback_logs":root.join("callbacks/<launch-id>/hook.log"),"config":crate::config::config_dir(),"state":crate::config::state_dir(),"xdg_config":std::env::var("XDG_CONFIG_HOME").ok(),"xdg_state":std::env::var("XDG_STATE_HOME").ok()})
            );
            Ok(())
        }
        "help" => {
            println!("{}\n", super::control_cli::HELP);
            println!("Bus — coordinate selected agents in native terminal rooms\n\nUsage: bus [--dev] [--paths | --help]\n--dev enables developer log files, excluding input/content dumps.\nExisting servers keep their original log level; they are never automatically restarted.\n--paths shows data and log directories without starting a session.\nBUS_DATA_DIR overrides Bus's isolated data root.\n\nCtrl+Shift+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+A/E line start/end · Ctrl+R history search · Ctrl+Shift+E composer size\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved.");
            Ok(())
        }
        _ => Err(io::Error::other("Usage: bus [--dev] [--paths | --help]")),
    }
}

pub(crate) fn apply_config(config: &mut crate::config::Config) {
    config.onboarding = Some(false);
    config.update.version_check = false;
    config.update.manifest_check = false;
    config.ui.sound.enabled = false;
}
