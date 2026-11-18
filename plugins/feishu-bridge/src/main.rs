mod config;
mod delivery;
mod error;
mod history;
mod launchd;
mod listener;
mod model;
mod process;
mod store;

use clap::{Args, Parser, Subcommand};
use error::{invalid, Result};
use std::{path::PathBuf, sync::atomic::Ordering};

#[derive(Parser)]
#[command(version, about = "Durable Feishu bot messaging bridge for macOS")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Send(Send),
    Listen {
        #[arg(long,conflicts_with="input",value_parser=clap::value_parser!(u64).range(1..))]
        run_for: Option<u64>,
        #[arg(long)]
        input: Option<PathBuf>,
    },
    Status,
    Install,
    Uninstall,
}
#[derive(Args)]
struct Send {
    #[arg(long, group = "recipient", required_unless_present = "chat_id")]
    open_id: Option<String>,
    #[arg(long, group = "recipient", required_unless_present = "open_id")]
    chat_id: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    text: String,
    #[arg(long)]
    idempotency_key: Option<String>,
}
fn execute(cli: Cli) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(invalid("macOS is required"));
    }
    let path = cli.config.map(Ok).unwrap_or_else(config::config_path)?;
    let c = config::Config::load(&path)?;
    let seconds = match &cli.command {
        Action::Listen { run_for, .. } => *run_for,
        _ => None,
    };
    let ctl = process::Control::new(seconds);
    let signal = ctl.stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))
        .map_err(|_| error::Error(1, "cannot install signal handler".into()))?;
    let output = match cli.command {
        Action::Send(s) => {
            if s.open_id
                .as_deref()
                .is_some_and(|s| !config::valid_id(s, "ou_"))
                || s.chat_id
                    .as_deref()
                    .is_some_and(|s| !config::valid_id(s, "oc_"))
                || s.idempotency_key
                    .as_ref()
                    .is_some_and(|s| s.is_empty() || s.len() > 50 || !s.is_ascii())
            {
                return Err(invalid("invalid recipient or idempotency key"));
            }
            Some(process::send(
                &c,
                s.open_id.as_deref(),
                s.chat_id.as_deref(),
                &s.text,
                s.idempotency_key.as_deref(),
                &ctl,
            )?)
        }
        Action::Listen { input: Some(p), .. } => {
            listener::import(&c, &p, &ctl)?;
            None
        }
        Action::Listen { .. } => {
            listener::live(&c, &ctl)?;
            None
        }
        Action::Status => Some(store::status(&c)?),
        Action::Install => Some(launchd::install(&c, &path)?),
        Action::Uninstall => Some(launchd::uninstall(&c)?),
    };
    if let Some(v) = output {
        println!("{v}");
    }
    Ok(())
}
fn main() {
    if let Err(e) = execute(Cli::parse()) {
        eprintln!("{e}");
        std::process::exit(e.0)
    }
}
