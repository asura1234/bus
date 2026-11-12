//! Unresolved registry input is accepted without inventing a destination session.
use super::{dispatch, select_instance, IncomingReply, Request, Response, Target, Value};
use crate::messaging::{feishu::Client, prefs::settings, storage::io};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
struct Receipt {
    accepted_order: u64,
    input: IncomingReply,
    event: Value,
    outcome: Value,
    target: Option<PathBuf>,
}

pub(super) fn ingest(
    root: &Path,
    app: &str,
    event: Value,
    input: IncomingReply,
    send: &mut impl FnMut(&Path, &Request) -> Result<Response, String>,
) -> Result<Response, String> {
    let settings = settings::load(&root.join("settings.json"))?;
    if input.sender_id != settings.feishu_recipient {
        return Err("Feishu sender does not match the configured recipient".into());
    }
    if Client::default()
        .program(settings.feishu_bridge_path.as_deref())
        .is_none()
    {
        return Err("Feishu bridge is not installed".into());
    }
    let _lock = io::append_lock(&root.join("feishu-inbox.lock")).map_err(|e| e.to_string())?;
    let path = root.join("feishu-inbox.json");
    let mut receipts: BTreeMap<String, Receipt> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid Feishu registry receipts: {e}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error.to_string()),
    };
    let key = input.key();
    let response = if let Some(receipt) = receipts.get(&key) {
        if receipt.input.sender_id != input.sender_id || receipt.input.chat_id != input.chat_id {
            return Err("Feishu replay identity does not match the accepted message".into());
        }
        // A copied/edit replay must never replace the first accepted version.
        // A successful handoff already owns a durable coordinator receipt.
        if let Some(target) = &receipt.target {
            match dispatch(target, app, receipt.event.clone(), send) {
                Ok(response) => response,
                Err(error) => {
                    log_pending(&error);
                    Response::success(
                        &super::super::control::next_request_id(),
                        receipt.outcome.clone(),
                    )
                }
            }
        } else {
            Response::success(
                &super::super::control::next_request_id(),
                receipt.outcome.clone(),
            )
        }
    } else {
        match select_instance(root, &input)? {
            Target::Session(target) => dispatch(&target, app, event, send)?,
            Target::Parked(reason) => {
                let outcome = json!({"stage":"parked", "reason":reason});
                let accepted_order = receipts
                    .values()
                    .map(|receipt| receipt.accepted_order)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or("Feishu registry acceptance order exhausted")?;
                receipts.insert(
                    key.clone(),
                    Receipt {
                        accepted_order,
                        input,
                        event,
                        outcome: outcome.clone(),
                        target: None,
                    },
                );
                save(&path, &receipts)?;
                log_pending(reason);
                Response::success(&super::super::control::next_request_id(), outcome)
            }
        }
    };
    if let Some(receipt) = receipts.get_mut(&key).filter(|r| r.target.is_some()) {
        receipt.outcome = response.result.clone();
        save(&path, &receipts)?;
    }
    // A later parent or outbound mapping can make a parked route exact. The
    // journal remains the durable acceptance owner until its coordinator acks.
    for _ in 0..receipts.len() {
        let mut progress = false;
        let mut pending: Vec<_> = receipts
            .values_mut()
            .filter(|r| r.target.is_none())
            .collect();
        pending.sort_by_key(|receipt| receipt.accepted_order);
        for receipt in pending {
            let target = match select_instance(root, &receipt.input) {
                Ok(Target::Session(target)) => target,
                Ok(Target::Parked(_)) => continue,
                Err(error) => {
                    log_pending(&error);
                    continue;
                }
            };
            match dispatch(&target, &receipt.input.app_id, receipt.event.clone(), send) {
                Ok(response) => {
                    receipt.outcome = response.result;
                    receipt.target = Some(target);
                    progress = true;
                }
                Err(error) => log_pending(&error),
            }
        }
        if !progress {
            break;
        }
        save(&path, &receipts)?;
    }
    if let Some(receipt) = receipts.get(&key) {
        if receipt.target.is_some() {
            return Ok(Response::success(&response.id, receipt.outcome.clone()));
        }
    }
    Ok(response)
}

fn save(path: &Path, receipts: &BTreeMap<String, Receipt>) -> Result<(), String> {
    let bytes = serde_json::to_vec(receipts).map_err(|e| e.to_string())?;
    io::atomic_write(path, &bytes).map_err(|e| e.to_string())
}

fn log_pending(reason: &str) {
    let _log_only = crate::utils::error_pipe::suppress(true);
    tracing::warn!(event = "bus.feishu.registry_parked", %reason, "Feishu reply remains in the registry journal");
}
