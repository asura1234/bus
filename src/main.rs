use std::io;

pub(crate) const HERDR_ENV_VAR: &str = "HERDR_ENV";
pub(crate) const HERDR_ENV_VALUE: &str = "1";
const NESTED_HERDR_MESSAGES: [&str; 6] = [
    "inception detected. we need to go deeper... said no one ever.",
    "recursion is a pathway to many abilities some consider to be... unnatural.",
    "you were so preoccupied with whether you could, you didn't stop to think if you should. — dr. malcolm",
    "recursive herdring is disabled. somewhere, a call stack breathes a sigh of relief.",
    "recursive descent denied. there is, in fact, such a thing as too much herdr.",
    "recursion detected. base case not found. aborting.",
];

mod agent_resume;
mod api;
mod app;
mod build_info;
mod bus;
mod checksum;
mod client;
mod config;
mod copy_mode;
mod detect;
mod events;
mod ghostty;
mod handoff_runtime;
mod input;
mod integration;
mod ipc;
mod kitty_graphics;
mod layout;
mod logging;
mod metadata_tokens;
mod noninteractive_process;
mod pane;
mod pane_graphics_files;
mod persist;
mod platform;
mod popup_size;
mod product_announcements;
mod protocol;
mod pty;
mod raw_input;
mod release_notes;
mod render_prof;
mod render_signal;
mod selection;
mod server;
mod session;
mod sound;
mod terminal;
mod terminal_effects;
mod terminal_modes;
mod terminal_notify;
mod terminal_theme;
mod ui;
mod update;
mod workspace;
mod worktree;

fn should_block_nested(config: &config::Config) -> bool {
    should_block_nested_for_env(config, std::env::var(HERDR_ENV_VAR).ok().as_deref())
}

fn should_block_nested_for_env(config: &config::Config, herdr_env: Option<&str>) -> bool {
    !config.experimental.allow_nested && herdr_env == Some(HERDR_ENV_VALUE)
}

fn random_nested_message() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as usize)
        .unwrap_or(0);
    let index = (nanos ^ (std::process::id() as usize)) % NESTED_HERDR_MESSAGES.len();
    NESTED_HERDR_MESSAGES[index]
}

fn exit_if_nested_disabled(config: &config::Config) {
    if should_block_nested(config) {
        eprintln!("\x1b[1merror:\x1b[0m nested herdr is disabled by default.");
        eprintln!("see configuration if you want to enable it.");
        eprintln!();
        eprintln!("\x1b[2m\"{}\"\x1b[0m", random_nested_message());
        std::process::exit(1);
    }
}

fn args_as_utf8<I>(args: I) -> Result<Vec<String>, String>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    args.into_iter()
        .enumerate()
        .map(|(index, arg)| {
            arg.into_string()
                .map_err(|_| format!("argument {index} is not valid UTF-8"))
        })
        .collect()
}

fn main() -> io::Result<()> {
    let raw_args: Vec<String> = match args_as_utf8(std::env::args_os()) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };
    if let Some(result) = bus::callbacks::dispatch(&raw_args) {
        return result;
    }
    if raw_args.get(1).is_some_and(|arg| arg == "--bus") {
        return bus::entry::run(&raw_args[2..]);
    }
    let args = match session::configure_from_args(&raw_args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };
    match args.get(1).map(String::as_str) {
        None => {}
        // Hidden entry the client spawns for the persistent daemon.
        Some("server") if args.len() == 2 => return server::headless::run_server(),
        // Hidden entry that attaches a thin client to the running daemon.
        Some("client") if args.len() == 2 => {
            let loaded_config = config::Config::load();
            exit_if_nested_disabled(&loaded_config.config);
            return client::run_client();
        }
        Some("update") => {
            let options = match update::parse_self_update_args(&args[2..]) {
                Ok(options) => options,
                Err(err) if err.starts_with("usage:") => {
                    eprintln!("{err}");
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("{err}");
                    eprintln!("usage: herdr update [--handoff]");
                    std::process::exit(2);
                }
            };
            match update::self_update(options) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    if e.starts_with("self-update is disabled") {
                        eprintln!("{e}");
                    } else {
                        eprintln!("update failed: {e}");
                    }
                    std::process::exit(1);
                }
            }
        }
        Some("--version" | "-V") if args.len() == 2 => {
            platform::begin_cli_output();
            println!("herdr {}", crate::build_info::version());
            return Ok(());
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            std::process::exit(2);
        }
    }

    let loaded_config = config::Config::load();
    exit_if_nested_disabled(&loaded_config.config);

    if let Err(err) = server::autodetect::auto_detect_launch(false) {
        eprintln!("herdr: {err}");
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_herdr_blocks_when_env_is_set() {
        let config = config::Config::default();
        assert!(should_block_nested_for_env(&config, Some(HERDR_ENV_VALUE)));
    }

    #[test]
    fn nested_herdr_does_not_block_when_allowed() {
        let config: config::Config =
            toml::from_str("[experimental]\nallow_nested = true\n").unwrap();
        assert!(!should_block_nested_for_env(&config, Some(HERDR_ENV_VALUE)));
    }

    #[test]
    fn nested_herdr_does_not_block_without_env() {
        let config = config::Config::default();
        assert!(!should_block_nested_for_env(&config, None));
    }

    #[test]
    fn random_nested_message_comes_from_known_set() {
        let message = random_nested_message();
        assert!(NESTED_HERDR_MESSAGES.contains(&message));
    }

    #[test]
    fn nested_message_strings_no_longer_repeat_herdr_prefix() {
        assert!(NESTED_HERDR_MESSAGES
            .iter()
            .all(|message| !message.starts_with("herdr:")));
    }

    #[cfg(unix)]
    fn invalid_utf8_arg() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    }

    #[cfg(windows)]
    fn invalid_utf8_arg() -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800])
    }

    #[test]
    fn args_as_utf8_passes_through_valid_arguments() {
        let args = ["herdr", "pane", "get", "pane-1"].map(std::ffi::OsString::from);
        assert_eq!(
            args_as_utf8(args).unwrap(),
            ["herdr", "pane", "get", "pane-1"]
        );
    }

    #[test]
    fn args_as_utf8_reports_the_offending_argument_instead_of_panicking() {
        let args = vec![
            std::ffi::OsString::from("herdr"),
            std::ffi::OsString::from("pane"),
            invalid_utf8_arg(),
        ];
        assert_eq!(
            args_as_utf8(args).unwrap_err(),
            "argument 2 is not valid UTF-8"
        );
    }
}
