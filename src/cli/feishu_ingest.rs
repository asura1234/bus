//! Bridge sink: select the recorded private instance without caller inference.
use crate::messaging::{
    control::{self, Request, Response},
    feishu::ingest::Message,
    model::{feishu::IncomingReply, BusState},
    storage::state_store::JsonStore,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_INPUT: u64 = 128 * 1024;

pub(super) fn run(args: &[String]) -> io::Result<()> {
    let (instance, app) = arguments(args).map_err(io::Error::other)?;
    let result = ingest_with(&instance, &app, io::stdin().lock(), control::request);
    match result {
        Ok(response) => {
            super::help::write_stdout_line(format_args!(
                "{}",
                serde_json::to_string(&response).map_err(io::Error::other)?
            ));
            Ok(())
        }
        Err(error) => {
            let _log_only = crate::utils::error_pipe::suppress(true);
            tracing::warn!(event = "bus.feishu.ingest_failed", %error, "Feishu reply remains in the bridge spool");
            Err(io::Error::other(error))
        }
    }
}

pub(super) fn arguments(args: &[String]) -> Result<(PathBuf, String), String> {
    let matches = super::control::command_tree(false)
        .try_get_matches_from(args)
        .map_err(|error| error.to_string())?;
    let args = matches
        .subcommand_matches("feishu")
        .and_then(|group| group.subcommand_matches("ingest"))
        .ok_or("Choose bus feishu ingest --help")?;
    let instance = PathBuf::from(
        args.get_one::<String>("instance")
            .ok_or("Missing --instance")?,
    );
    if !instance.is_absolute() {
        return Err("Feishu sink --instance must be an absolute root or registry path".into());
    }
    Ok((
        instance,
        args.get_one::<String>("app-id")
            .ok_or("Missing --app-id")?
            .clone(),
    ))
}

fn read_input(input: impl Read, app: &str) -> Result<(Value, IncomingReply), String> {
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Feishu sink input")?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err("Feishu sink input exceeds 128 KiB".into());
    }
    let message: Message =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid Feishu v1 sink JSON")?;
    let normalized = message.normalize(app)?;
    let wire = serde_json::from_slice(&bytes).map_err(|_| "Invalid Feishu v1 sink JSON")?;
    Ok((wire, normalized))
}

fn ingest_with(
    instance: &Path,
    app: &str,
    input: impl Read,
    mut send: impl FnMut(&Path, &Request) -> Result<Response, String>,
) -> Result<Response, String> {
    let (event, normalized) = read_input(input, app)?;
    let target = select_instance(instance, &normalized)?;
    // Every sink invocation can recover after a cached refusal. Durable app/message
    // receipts own replay protection; transport retries within this call keep this ID.
    let request = Request {
        id: super::control::next_request_id(),
        method: "feishu.ingest".into(),
        params: json!({"app_id":app, "event":event}),
    };
    let response = send(&target, &request)?;
    if !response.ok {
        return Err(response.error.as_ref().map_or_else(
            || "Bus did not acknowledge Feishu ingestion".into(),
            |error| format!("Bus refused Feishu ingestion: {}", error.message),
        ));
    }
    Ok(response)
}

fn select_instance(instance: &Path, input: &IncomingReply) -> Result<PathBuf, String> {
    if !instance.is_absolute() {
        return Err("Feishu sink instance must be absolute".into());
    }
    let instance =
        std::fs::canonicalize(instance).map_err(|_| "Feishu sink instance does not exist")?;
    if instance.join("state.json").is_file() {
        return Ok(instance);
    }
    let entries = std::fs::read_dir(instance.join("sessions"))
        .map_err(|_| "Feishu sink instance is neither a session nor a registry")?;
    let mut receipts = BTreeSet::new();
    let mut references = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|_| "Cannot inspect Bus session registry")?;
        if !entry
            .file_type()
            .map_err(|_| "Cannot inspect Bus registry entry")?
            .is_dir()
        {
            continue;
        }
        let root =
            std::fs::canonicalize(entry.path()).map_err(|_| "Cannot resolve Bus session root")?;
        let Some(state) = JsonStore::new(root.join("state.json"))
            .load()
            .map_err(|error| format!("Cannot inspect Bus routing state: {error}"))?
        else {
            continue;
        };
        if state.feishu.inbox.contains_key(&input.key()) {
            receipts.insert(root.clone());
        }
        if state.feishu.resolve(input).is_ok() || references_session(&state, input) {
            references.insert(root);
        }
    }
    let candidates = if receipts.is_empty() {
        references
    } else {
        receipts
    };
    if candidates.len() != 1 {
        return Err(if candidates.is_empty() {
            "Unknown Feishu reply; no mapped Bus session"
        } else {
            "Ambiguous Feishu reply; multiple Bus sessions match"
        }
        .into());
    }
    candidates
        .into_iter()
        .next()
        .ok_or_else(|| "Unknown Feishu reply".into())
}

/// Include evidence that a per-session resolver rejects locally, so references
/// split across sessions cannot become a false unique match elsewhere.
fn references_session(state: &BusState, input: &IncomingReply) -> bool {
    input
        .reply_to
        .iter()
        .chain(input.root_id.iter())
        .any(|message| {
            state
                .feishu
                .outbox
                .values()
                .any(|record| record.message_id.as_ref() == Some(message))
                || state
                    .feishu
                    .inbox
                    .contains_key(&format!("{}:{message}", input.app_id))
        })
        || input.text.split_whitespace().any(|word| {
            let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':');
            word.strip_prefix("bus:")
                .is_some_and(|code| state.feishu.outbox.contains_key(code))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::model::feishu_fixture;

    struct Fixture {
        base: PathBuf,
        first: PathBuf,
        second: PathBuf,
        code_second: String,
    }

    impl Fixture {
        fn new() -> Self {
            let base = crate::utils::test_temp::unique_temp_path("feishu-sink-registry");
            let first = base.join("sessions/first");
            let second = base.join("sessions/second");
            let mut codes = Vec::new();
            for (root, outbound) in [(&first, "om_first"), (&second, "om_second")] {
                std::fs::create_dir_all(root).unwrap();
                let (state, code) = feishu_fixture::mapped(root, outbound);
                JsonStore::new(root.join("state.json"))
                    .save(&state)
                    .unwrap();
                codes.push(code);
            }
            std::fs::write(base.join("last-session"), "second").unwrap();
            Self {
                base,
                first,
                second,
                code_second: codes.pop().unwrap(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn wire(parent: Option<&str>, text: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"app_id":"cli_test","message_id":"om_reply","chat_id":"oc_dm","chat_type":"p2p",
            "sender":{"open_id":"ou_developer","sender_type":"user"},"text":text,"reply_to":parent,"root_id":null,"thread_id":null,"create_time":"20"})).unwrap()
    }

    #[test]
    fn feishu_sink_selects_the_mapped_session_with_overlapping_ids_and_wrong_last_session() {
        let fixture = Fixture::new();
        let event = wire(Some("om_first"), "Proceed");
        let result = ingest_with(
            &fixture.base,
            "cli_test",
            event.as_slice(),
            |root, request| {
                assert_eq!(root, std::fs::canonicalize(&fixture.first).unwrap());
                assert_eq!(request.method, "feishu.ingest");
                assert_eq!(request.params["event"]["text"], "Proceed");
                assert!(request.params.get("caller").is_none());
                assert!(request.params.get("as").is_none());
                Ok(Response::success(&request.id, json!({"stage":"delivered"})))
            },
        )
        .unwrap();
        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(fixture.base.join("last-session")).unwrap(),
            "second"
        );
    }

    #[test]
    fn feishu_sink_refuses_cross_session_conflicts_and_unknown_replies_before_dispatch() {
        let fixture = Fixture::new();
        for event in [
            wire(None, "Unknown"),
            wire(
                Some("om_first"),
                &format!("bus:{} Proceed", fixture.code_second),
            ),
        ] {
            assert!(
                ingest_with(&fixture.base, "cli_test", event.as_slice(), |_, _| panic!(
                    "ambiguous or unknown input cannot reach a control socket"
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn feishu_sink_offline_mapped_instance_never_falls_back_to_another_session() {
        let fixture = Fixture::new();
        let event = wire(Some("om_first"), "Proceed");
        let mut paths = Vec::new();
        let result = ingest_with(&fixture.base, "cli_test", event.as_slice(), |root, _| {
            paths.push(root.to_path_buf());
            Err("control_unavailable: offline".into())
        });
        assert!(result.unwrap_err().contains("offline"));
        assert_eq!(paths, [std::fs::canonicalize(&fixture.first).unwrap()]);
        assert_ne!(paths[0], std::fs::canonicalize(&fixture.second).unwrap());
    }

    #[test]
    fn feishu_sink_new_invocation_can_recover_after_a_cached_refusal() {
        let fixture = Fixture::new();
        let event = wire(Some("om_first"), "Proceed");
        let mut receipts = std::collections::BTreeMap::<String, Response>::new();
        // The real coordinator caches mutation responses, including refusals.
        let mut transport = |_: &Path, request: &Request| {
            if let Some(receipt) = receipts.get(&request.id) {
                return Ok(receipt.clone());
            }
            let response = if receipts.is_empty() {
                Response::failure(
                    &request.id,
                    "storage_unavailable",
                    "Temporarily unavailable",
                )
            } else {
                Response::success(
                    &request.id,
                    json!({"stage":"delivered","message_id":10,"request_id":11}),
                )
            };
            receipts.insert(request.id.clone(), response.clone());
            Ok(response)
        };
        assert!(ingest_with(&fixture.base, "cli_test", event.as_slice(), &mut transport).is_err());
        assert!(ingest_with(&fixture.base, "cli_test", event.as_slice(), &mut transport).is_ok(),
            "a fresh sink invocation must recover after repair; durable Bus message-ID dedupe owns repeat protection");
    }

    #[test]
    fn feishu_sink_explicit_root_parks_unknown_input_through_only_its_own_coordinator() {
        let fixture = Fixture::new();
        let event = wire(None, "Unknown");
        let result = ingest_with(
            &fixture.first,
            "cli_test",
            event.as_slice(),
            |root, request| {
                assert_eq!(root, std::fs::canonicalize(&fixture.first).unwrap());
                Ok(Response::success(&request.id, json!({"stage":"parked"})))
            },
        )
        .unwrap();
        assert_eq!(result.result["stage"], "parked");
    }

    #[test]
    fn feishu_sink_requires_a_bus_acknowledgement_and_bounds_input_before_dispatch() {
        let fixture = Fixture::new();
        let event = wire(Some("om_first"), "Proceed");
        assert!(ingest_with(
            &fixture.first,
            "cli_test",
            event.as_slice(),
            |_, request| Ok(Response::failure(
                &request.id,
                "storage_unavailable",
                "not saved"
            ))
        )
        .is_err());
        let oversized = vec![b' '; MAX_INPUT as usize + 1];
        assert!(ingest_with(
            &fixture.first,
            "cli_test",
            oversized.as_slice(),
            |_, _| panic!("oversized input cannot dispatch")
        )
        .unwrap_err()
        .contains("128 KiB"));
        let args = [
            "feishu",
            "ingest",
            "--instance",
            "relative",
            "--app-id",
            "cli_test",
        ]
        .map(String::from);
        assert!(arguments(&args).unwrap_err().contains("absolute"));
    }
}
