//! Opt-in process edge; provider HOME/XDG settings are never changed.
use std::{io, path::PathBuf};

pub(crate) fn data_dir() -> Option<PathBuf> {
    std::env::var_os("BUS_DATA_DIR").map(PathBuf::from)
}

pub(crate) fn run(args: &[String]) -> io::Result<()> {
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
    match args {
        [] => crate::server::autodetect::auto_detect_launch(false),
        [arg] if arg == "--paths" => {
            println!(
                "{}",
                serde_json::json!({"data":root,"config":crate::config::config_dir(),"state":crate::config::state_dir(),"xdg_config":std::env::var("XDG_CONFIG_HOME").ok(),"xdg_state":std::env::var("XDG_STATE_HOME").ok()})
            );
            Ok(())
        }
        [arg] if arg == "--help" || arg == "-h" => {
            println!("Bus — coordinate selected agents in native terminal rooms\n\nUsage: ./bus [--paths]\nBUS_DATA_DIR overrides Bus's isolated data root.\n\nCtrl+R room · Ctrl+N agent · Ctrl+F files · F2 rename · F3 notes\n@ choose agents · + choose files (type the shifted symbols)\nEnter send · Shift+Enter (supported hosts) / Ctrl+J newline\nCtrl+E full/compact composer · Page Up/Down scroll draft\nF6 room · Ctrl+C save and quit (Ctrl+Q also works)\n\nBuilt on Herdr; upstream license and attribution are preserved.");
            Ok(())
        }
        _ => Err(io::Error::other("Usage: ./bus [--paths | --help]")),
    }
}

pub(crate) fn apply_config(config: &mut crate::config::Config) {
    config.onboarding = Some(false);
    config.update.version_check = false;
    config.update.manifest_check = false;
    config.ui.sound.enabled = false;
}
