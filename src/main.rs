use std::io;

mod agents;
mod cli;
mod client;
mod devtools;
mod messaging;
mod platform;
mod protocol;
mod server;
mod terminal;
mod utils;

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
    // A driver session's fake provider CLIs are this binary under a provider's name.
    if let Some(provider) = raw_args
        .first()
        .and_then(|arg0| devtools::tui::fake_provider(arg0))
    {
        std::process::exit(devtools::tui::run_fake_agent(provider, &raw_args[1..]));
    }
    if raw_args.get(1).map(String::as_str) == Some("--bus-callback") {
        let logging_options = cli::logging_options();
        let diagnostics_dir = std::env::var_os("BUS_CALLBACK_DIR").map(std::path::PathBuf::from);
        if let Some(result) = agents::providers::callback_entry::dispatch(
            &raw_args,
            diagnostics_dir,
            &logging_options,
        ) {
            return result;
        }
    }
    match raw_args.get(1).map(String::as_str) {
        // Hidden entry the client spawns for the persistent daemon.
        Some("server") if raw_args.len() == 2 => {
            configure_session(&raw_args);
            server::main_loop::run_server(cli::logging_options(), cli::config_override())
        }
        // Hidden entry that attaches a thin client to the running daemon.
        Some("client") if raw_args.len() == 2 => {
            configure_session(&raw_args);
            client::run_client(
                cli::logging_options(),
                cli::config_override(),
                cli::stop::stop_active_server,
            )
        }
        // Dev tools exist only behind --dev.
        Some("--dev") if raw_args.get(2).map(String::as_str) == Some("tui") => {
            std::process::exit(devtools::tui::run(&raw_args[3..]))
        }
        Some("tui") => {
            eprintln!("error: tui is a dev tool; run `bus --dev tui ...`");
            std::process::exit(2)
        }
        Some("--version" | "-V") if raw_args.len() == 2 => {
            platform::begin_cli_output();
            cli::help::write_stdout_line(format_args!("bus {}", utils::version::version()));
            Ok(())
        }
        _ => cli::run(&raw_args[1..]),
    }
}

/// Applies the daemon session selected through the environment before a hidden entry runs.
fn configure_session(args: &[String]) {
    if let Err(err) = utils::paths::configure_from_args(args) {
        eprintln!("error: {err}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::args_as_utf8;

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
