//! Machine preferences and direct commands; neither path starts another Bus.
use crate::messaging::control::Response;
use crate::messaging::machines::{self, MachineTarget};
use std::io::{self, Write};

pub(super) fn dispatch(args: &[String]) -> Option<io::Result<()>> {
    let offset = usize::from(args.first().is_some_and(|arg| arg == "--dev"));
    match args.get(offset).map(String::as_str) {
        Some("machine") => Some(manage(&args[offset + 1..])),
        Some("--machine") => Some(match args.get(offset + 1) {
            Some(label) if !label.is_empty() => remote(label, &args[offset + 2..]),
            _ => Err(io::Error::other(machines::HELP)),
        }),
        Some(value) if value.starts_with("--machine=") => {
            Some(remote(&value["--machine=".len()..], &args[offset + 1..]))
        }
        _ => None,
    }
}

fn emit(response: &Response) -> io::Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, response).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn manage(args: &[String]) -> io::Result<()> {
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return io::stdout().lock().write_all(machines::HELP.as_bytes());
    }
    let id = super::control::next_request_id();
    let result = manage_inner(args);
    match result {
        Ok(result) => emit(&Response::success(&id, result)),
        Err(error) => {
            emit(&Response::failure(&id, "machine_error", error))?;
            std::process::exit(1)
        }
    }
}

fn manage_inner(args: &[String]) -> Result<serde_json::Value, String> {
    match args {
        [command] if command == "list" => {
            Ok(serde_json::json!({"machines":machines::load_targets()?}))
        }
        [command, label, target] if command == "add" => {
            machines::update_targets(|targets| {
                if targets.iter().any(|machine| machine.label == *label) {
                    return Err(format!("Machine {label:?} is already saved; remove it before replacing its SSH target"));
                }
                targets.push(MachineTarget {
                    label: label.clone(),
                    target: target.clone(),
                    enabled: true,
                });
                Ok(())
            })?;
            Ok(serde_json::json!({"label":label,"target":target,"enabled":true}))
        }
        [command, label] if matches!(command.as_str(), "remove" | "enable" | "disable") => {
            machines::update_targets(|targets| {
                let index = targets
                    .iter()
                    .position(|machine| machine.label == *label)
                    .ok_or_else(|| format!("Unknown machine {label:?}"))?;
                match command.as_str() {
                    "remove" => {
                        targets.remove(index);
                    }
                    "enable" => targets[index].enabled = true,
                    _ => targets[index].enabled = false,
                }
                Ok(())
            })?;
            Ok(serde_json::json!({"label":label,"operation":command}))
        }
        _ => Err(machines::HELP.into()),
    }
}

fn remote(label: &str, args: &[String]) -> io::Result<()> {
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        return io::stdout().lock().write_all(machines::HELP.as_bytes());
    }
    let target = machines::load_targets().and_then(|targets| route(&targets, label));
    match target {
        Ok(Route::Direct(target)) => super::control::run_remote(&target, args),
        Ok(Route::Viewer) => super::control::run_remote_forwarded(label, args),
        Err(error) => {
            emit(&Response::failure(
                &super::control::next_request_id(),
                "machine_unavailable",
                error,
            ))?;
            std::process::exit(1)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Direct(String),
    Viewer,
}

fn route(targets: &[MachineTarget], label: &str) -> Result<Route, String> {
    match targets.iter().find(|target| target.label == label) {
        Some(target) if target.enabled => Ok(Route::Direct(target.target.clone())),
        Some(_) => Err(format!(
            "Machine {label:?} is disabled; use bus machine enable"
        )),
        None => Ok(Route::Viewer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_routing_only_reads_global_prefix_not_message_text() {
        let args = [
            "send",
            "--room",
            "1",
            "--to",
            "worker",
            "--text",
            "--machine",
        ]
        .map(str::to_owned);
        assert!(dispatch(&args).is_none());
    }

    #[test]
    fn machine_without_local_config_uses_a_live_viewer_route() {
        assert_eq!(route(&[], "C").unwrap(), Route::Viewer);
        assert_eq!(
            route(
                &[MachineTarget {
                    label: "C".into(),
                    target: "c-host".into(),
                    enabled: true
                }],
                "C"
            )
            .unwrap(),
            Route::Direct("c-host".into())
        );
    }
}
