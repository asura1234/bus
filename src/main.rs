use std::io;

pub(crate) const HERDR_ENV_VAR: &str = "HERDR_ENV";
pub(crate) const HERDR_ENV_VALUE: &str = "1";

mod agent_resume;
mod api;
mod app;
mod build_info;
mod bus;
mod client;
mod config;
mod copy_mode;
mod detect;
mod events;
mod ghostty;
mod home_path;
mod input;
mod ipc;
mod kitty_graphics;
mod layout;
mod logging;
mod metadata_tokens;
#[cfg(any(windows, test))]
mod noninteractive_process;
mod pane;
mod persist;
mod platform;
mod popup_size;
mod protocol;
mod pty;
mod raw_input;
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
mod workspace;

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
    match raw_args.get(1).map(String::as_str) {
        // Hidden entry the client spawns for the persistent daemon.
        Some("server") if raw_args.len() == 2 => {
            configure_session(&raw_args);
            server::headless::run_server()
        }
        // Hidden entry that attaches a thin client to the running daemon.
        Some("client") if raw_args.len() == 2 => {
            configure_session(&raw_args);
            client::run_client()
        }
        Some("--version" | "-V") if raw_args.len() == 2 => {
            platform::begin_cli_output();
            println!("bus {}", crate::build_info::version());
            Ok(())
        }
        _ => bus::entry::run(&raw_args[1..]),
    }
}

/// Applies the daemon session selected through the environment before a hidden entry runs.
fn configure_session(args: &[String]) {
    if let Err(err) = session::configure_from_args(args) {
        eprintln!("error: {err}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let args = ["bus", "resume", "--last"].map(std::ffi::OsString::from);
        assert_eq!(args_as_utf8(args).unwrap(), ["bus", "resume", "--last"]);
    }

    #[test]
    fn args_as_utf8_reports_the_offending_argument_instead_of_panicking() {
        let args = vec![
            std::ffi::OsString::from("bus"),
            std::ffi::OsString::from("resume"),
            invalid_utf8_arg(),
        ];
        assert_eq!(
            args_as_utf8(args).unwrap_err(),
            "argument 2 is not valid UTF-8"
        );
    }
}
