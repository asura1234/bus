//! Bridge sink: select the recorded private instance without caller inference.
mod registry;
mod routing;
use crate::messaging::{
    control::{self, Request, Response},
    feishu::ingest::Message,
    model::feishu::IncomingReply,
};
use serde_json::{json, Value};
use std::{
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
    if !instance.is_absolute() {
        return Err("Feishu sink instance must be absolute".into());
    }
    let instance =
        std::fs::canonicalize(instance).map_err(|_| "Feishu sink instance does not exist")?;
    let registry =
        instance.join("sessions").is_dir() || instance.join("session-registry.lock").is_file();
    if !registry && instance.join("state.json").is_file() {
        return dispatch(&instance, app, event, &mut send);
    }
    if !registry {
        return Err("Feishu sink instance is neither a session nor a registry".into());
    }
    registry::ingest(&instance, app, event, normalized, &mut send, routing::read)
}

fn dispatch(
    target: &Path,
    app: &str,
    event: Value,
    send: &mut impl FnMut(&Path, &Request) -> Result<Response, String>,
) -> Result<Response, String> {
    // Every sink invocation can recover after a cached refusal. Durable app/message
    // receipts own replay protection; transport retries within this call keep this ID.
    let request = Request {
        id: super::control::next_request_id(),
        method: "feishu.ingest".into(),
        params: json!({"app_id":app, "event":event}),
    };
    let response = send(target, &request)?;
    if !response.ok {
        return Err(response.error.as_ref().map_or_else(
            || "Bus did not acknowledge Feishu ingestion".into(),
            |error| format!("Bus refused Feishu ingestion: {}", error.message),
        ));
    }
    Ok(response)
}

enum Target {
    Session(PathBuf),
    Parked(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::{model::feishu_fixture, storage::state_store::JsonStore};

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
            crate::messaging::prefs::settings::save(
                &base.join("settings.json"),
                &crate::messaging::prefs::settings::BusSettings {
                    feishu_recipient: "ou_developer".into(),
                    feishu_bridge_path: Some("/usr/bin/true".into()),
                    ..Default::default()
                },
            )
            .unwrap();
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
    fn feishu_sink_durably_parks_cross_session_conflicts_and_unknown_replies_before_dispatch() {
        let fixture = Fixture::new();
        for (index, event) in [
            wire(None, "Unknown"),
            wire(
                Some("om_first"),
                &format!("bus:{} Proceed", fixture.code_second),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut event: Value = serde_json::from_slice(&event).unwrap();
            event["message_id"] = json!(format!("om_parked_{index}"));
            let bytes = serde_json::to_vec(&event).unwrap();
            let response = ingest_with(&fixture.base, "cli_test", bytes.as_slice(), |_, _| {
                panic!("ambiguous or unknown input cannot reach a control socket")
            })
            .unwrap();
            assert_eq!(response.result["stage"], "parked");
        }
        let journal: Value =
            serde_json::from_slice(&std::fs::read(fixture.base.join("feishu-inbox.json")).unwrap())
                .unwrap();
        assert_eq!(journal.as_object().unwrap().len(), 2);
    }

    #[test]
    fn feishu_registry_parked_first_version_survives_reload_without_blocking_a_mapped_reply() {
        let f = Fixture::new();
        let unknown = wire(None, "Unmapped original");
        assert_eq!(
            ingest_with(&f.base, "cli_test", unknown.as_slice(), |_, _| panic!(
                "unknown"
            ))
            .unwrap()
            .result["stage"],
            "parked"
        );
        let edited = wire(Some("om_first"), "Edited replay must not run");
        assert_eq!(
            ingest_with(&f.base, "cli_test", edited.as_slice(), |_, _| panic!(
                "edited replay"
            ))
            .unwrap()
            .result["stage"],
            "parked"
        );
        let mut valid: Value = serde_json::from_slice(&edited).unwrap();
        valid["message_id"] = json!("om_valid");
        let mut routed = Vec::new();
        let response = ingest_with(
            &f.base,
            "cli_test",
            serde_json::to_vec(&valid).unwrap().as_slice(),
            |target, request| {
                routed.push(target.to_owned());
                assert_eq!(request.params["event"]["message_id"], "om_valid");
                Ok(Response::success(&request.id, json!({"stage":"delivered"})))
            },
        )
        .unwrap();
        assert_eq!(response.result["stage"], "delivered");
        assert_eq!(routed, [std::fs::canonicalize(&f.first).unwrap()]);
        let journal: Value =
            serde_json::from_slice(&std::fs::read(f.base.join("feishu-inbox.json")).unwrap())
                .unwrap();
        assert_eq!(
            journal["cli_test:om_reply"]["input"]["text"],
            "Unmapped original"
        );
    }

    #[test]
    fn feishu_registry_parked_child_follows_its_backfilled_parent_exactly_once() {
        let f = Fixture::new();
        let child = wire(Some("om_parent"), "Original child instruction");
        assert_eq!(
            ingest_with(&f.base, "cli_test", child.as_slice(), |_, _| panic!(
                "unknown child"
            ))
            .unwrap()
            .result["stage"],
            "parked"
        );
        let mut parent: Value =
            serde_json::from_slice(&wire(None, &format!("bus:{} Parent", f.code_second))).unwrap();
        parent["message_id"] = json!("om_parent");
        let mut control = |root: &Path, request: &Request| {
            assert_eq!(root, std::fs::canonicalize(&f.second).unwrap());
            let store = JsonStore::new(root.join("state.json"));
            let mut state = store.load().unwrap().unwrap();
            let input = serde_json::from_value::<Message>(request.params["event"].clone())
                .unwrap()
                .normalize("cli_test")
                .unwrap();
            let outcome = state
                .ingest_feishu_reply(input, "ou_developer", 30)
                .unwrap();
            store.save(&state).unwrap();
            Ok(Response::success(
                &request.id,
                serde_json::to_value(outcome).unwrap(),
            ))
        };
        assert_eq!(
            ingest_with(
                &f.base,
                "cli_test",
                serde_json::to_vec(&parent).unwrap().as_slice(),
                &mut control
            )
            .unwrap()
            .result["stage"],
            "delivered"
        );
        let replay = ingest_with(&f.base, "cli_test", child.as_slice(), &mut control).unwrap();
        assert_eq!(replay.result["stage"], "delivered");
        assert_eq!(
            ingest_with(&f.base, "cli_test", child.as_slice(), &mut control)
                .unwrap()
                .result,
            replay.result
        );
        let mut edited_parent: Value =
            serde_json::from_slice(&wire(None, "Edited parent without a code")).unwrap();
        edited_parent["message_id"] = json!("om_parent");
        assert_eq!(
            ingest_with(
                &f.base,
                "cli_test",
                serde_json::to_vec(&edited_parent).unwrap().as_slice(),
                &mut control
            )
            .unwrap()
            .result["stage"],
            "delivered",
            "an edited replay still finds its canonical session receipt"
        );
        let state = JsonStore::new(f.second.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(state.requests().count(), 2);
        assert!(JsonStore::new(f.first.join("state.json"))
            .load()
            .unwrap()
            .unwrap()
            .feishu
            .inbox
            .is_empty());
    }

    #[test]
    fn feishu_registry_does_not_acknowledge_failed_storage_or_unauthenticated_input() {
        let f = Fixture::new();
        let mut unknown: Value = serde_json::from_slice(&wire(None, "Unknown")).unwrap();
        unknown["sender"]["open_id"] = json!("ou_impostor");
        assert!(ingest_with(
            &f.base,
            "cli_test",
            serde_json::to_vec(&unknown).unwrap().as_slice(),
            |_, _| panic!("unauthenticated")
        )
        .is_err());
        assert!(!f.base.join("feishu-inbox.json").exists());
        std::fs::create_dir(f.base.join("feishu-inbox.json")).unwrap();
        assert!(ingest_with(
            &f.base,
            "cli_test",
            wire(None, "Unknown").as_slice(),
            |_, _| panic!("uncommitted")
        )
        .is_err());
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

    #[test]
    fn feishu_registry_legacy_base_state_does_not_hide_registered_sessions() {
        let f = Fixture::new();
        std::fs::copy(f.first.join("state.json"), f.base.join("state.json")).unwrap();
        let unknown = wire(None, "Unknown");
        assert_eq!(
            ingest_with(&f.base, "cli_test", unknown.as_slice(), |_, _| panic!(
                "registry input must not target its legacy base"
            ))
            .unwrap()
            .result["stage"],
            "parked"
        );
        let mut mapped: Value =
            serde_json::from_slice(&wire(Some("om_second"), "Continue")).unwrap();
        mapped["message_id"] = json!("om_legacy_valid");
        let mapped = serde_json::to_vec(&mapped).unwrap();
        let response = ingest_with(&f.base, "cli_test", mapped.as_slice(), |root, request| {
            assert_eq!(root, std::fs::canonicalize(&f.second).unwrap());
            Ok(Response::success(&request.id, json!({"stage":"delivered"})))
        })
        .unwrap();
        assert_eq!(response.result["stage"], "delivered");
    }

    #[test]
    fn feishu_registry_reconciles_parked_siblings_in_acceptance_order() {
        let f = Fixture::new();
        // Message IDs are opaque. Their lexical order is unrelated to the serial
        // bridge lane: the first accepted sibling can sort after the second.
        for (id, text, created) in [
            ("om_z_first", "Use option A", "20"),
            ("om_a_second", "Use option B instead", "30"),
        ] {
            let mut event: Value = serde_json::from_slice(&wire(Some("om_parent"), text)).unwrap();
            event["message_id"] = json!(id);
            event["root_id"] = json!("om_parent");
            event["create_time"] = json!(created);
            let response = ingest_with(
                &f.base,
                "cli_test",
                serde_json::to_vec(&event).unwrap().as_slice(),
                |_, _| panic!("unknown parent"),
            )
            .unwrap();
            assert_eq!(response.result["stage"], "parked");
        }
        let mut parent: Value =
            serde_json::from_slice(&wire(None, &format!("bus:{} Parent", f.code_second))).unwrap();
        parent["message_id"] = json!("om_parent");
        parent["create_time"] = json!("10");
        let mut control = |root: &Path, request: &Request| {
            assert_eq!(root, std::fs::canonicalize(&f.second).unwrap());
            let store = JsonStore::new(root.join("state.json"));
            let mut state = store.load().unwrap().unwrap();
            let input = serde_json::from_value::<Message>(request.params["event"].clone())
                .unwrap()
                .normalize("cli_test")
                .unwrap();
            let outcome = state
                .ingest_feishu_reply(input, "ou_developer", 50)
                .unwrap();
            store.save(&state).unwrap();
            Ok(Response::success(
                &request.id,
                serde_json::to_value(outcome).unwrap(),
            ))
        };
        let response = ingest_with(
            &f.base,
            "cli_test",
            serde_json::to_vec(&parent).unwrap().as_slice(),
            &mut control,
        )
        .unwrap();
        assert_eq!(response.result["stage"], "delivered");
        let state = JsonStore::new(f.second.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        let mut queued: Vec<_> = state
            .requests()
            .map(|r| {
                (
                    r.prompt.id,
                    r.prompt
                        .in_reply_to
                        .as_ref()
                        .unwrap()
                        .feishu_message_id
                        .clone(),
                )
            })
            .collect();
        queued.sort_by_key(|(id, _)| *id);
        let ids: Vec<_> = queued.into_iter().map(|(_, external)| external).collect();
        assert_eq!(
            ids,
            ["om_parent", "om_z_first", "om_a_second"],
            "late ancestry must preserve the original bridge acceptance order among siblings"
        );
    }

    #[test]
    fn feishu_registry_parked_receipts_do_not_multiply_history_loads() {
        for referenced in [false, true] {
            let f = Fixture::new();
            for index in 0..40 {
                let mut event: Value = serde_json::from_slice(&wire(
                    referenced.then_some("om_not_a_bus_parent"),
                    "Unmapped",
                ))
                .unwrap();
                event["message_id"] = json!(format!("om_pending_{index}"));
                assert_eq!(
                    ingest_with(
                        &f.base,
                        "cli_test",
                        serde_json::to_vec(&event).unwrap().as_slice(),
                        |_, _| panic!("unknown reference cannot dispatch")
                    )
                    .unwrap()
                    .result["stage"],
                    "parked"
                );
            }
            let bytes = wire(
                referenced.then_some("om_still_unknown"),
                "New unmatched input",
            );
            let (event, input) = read_input(bytes.as_slice(), "cli_test").unwrap();
            let mut loads = Vec::new();
            let response = registry::ingest(
                &f.base,
                "cli_test",
                event,
                input,
                &mut |_, _| panic!("unknown reference cannot dispatch"),
                |root| {
                    loads.push(root.to_owned());
                    routing::read(root)
                },
            )
            .unwrap();
            assert_eq!(response.result["stage"], "parked");
            loads.sort();
            let mut expected = vec![
                f.first.canonicalize().unwrap(),
                f.second.canonicalize().unwrap(),
            ];
            expected.sort();
            assert_eq!(
                loads, expected,
                "one invocation loads each history once, independent of forty parked receipts"
            );
            let journal: Value =
                serde_json::from_slice(&std::fs::read(f.base.join("feishu-inbox.json")).unwrap())
                    .unwrap();
            assert_eq!(journal.as_object().unwrap().len(), 41);
        }
    }

    #[test]
    fn feishu_sink_uninitialized_directory_never_creates_a_registry_receipt() {
        let f = Fixture::new();
        std::fs::rename(f.base.join("sessions"), f.base.join("saved-sessions")).unwrap();
        assert!(ingest_with(
            &f.base,
            "cli_test",
            wire(None, "Unknown").as_slice(),
            |_, _| panic!("not an instance")
        )
        .unwrap_err()
        .contains("neither a session nor a registry"));
        assert!(!f.base.join("feishu-inbox.json").exists());
    }
}
