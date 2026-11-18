use crate::{
    config::{valid_id, Config},
    error::{upstream, Error, Result},
    model,
    process::{self, Control},
    store::Store,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn pages(c: &Config, mut query: Value, ctl: &Control) -> Result<Vec<Value>> {
    let mut tokens = HashSet::new();
    let mut all = vec![];
    loop {
        if ctl.done() {
            return Err(upstream("history interrupted"));
        }
        let data = process::api(c, "/open-apis/im/v1/messages", query.clone(), ctl)?;
        let items = data["items"]
            .as_array()
            .ok_or_else(|| upstream("invalid history items"))?;
        all.extend(items.iter().cloned());
        match data["has_more"].as_bool() {
            Some(false) => return Ok(all),
            Some(true) => {
                let token = data["page_token"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| upstream("missing history page token"))?;
                if !tokens.insert(token.to_string()) {
                    return Err(upstream("repeated history page token"));
                }
                query["page_token"] = json!(token);
            }
            _ => return Err(upstream("invalid history pagination state")),
        }
    }
}
pub fn reconcile(store: &mut Store, c: &Config, chat: &str, ctl: &Control) -> Result<()> {
    // Raw message history has no trustworthy conversation type. Authenticate
    // the container before assigning the p2p type to its recovered messages.
    let info = process::lark(
        c,
        &[
            "im".into(),
            "chats".into(),
            "get".into(),
            "--chat-id".into(),
            chat.into(),
            "--as".into(),
            "bot".into(),
            "--format".into(),
            "json".into(),
        ],
        vec![],
        ctl,
        true,
    )?;
    if info["data"]["chat_mode"] != "p2p" {
        return Err(upstream("history container is not a p2p chat"));
    }
    let end = now_ms();
    let start = store
        .watermark(&c.app_id, chat)?
        .map(|t| t.saturating_sub((c.overlap_seconds * 1000) as i64).max(0))
        .unwrap_or_else(|| c.initial_history_start.parse().unwrap());
    let raw = pages(
        c,
        json!({"container_id_type":"chat","container_id":chat,"start_time":(start/1000).to_string(),"end_time":(end/1000).to_string(),"page_size":50,"sort_type":"ByCreateTimeAsc","user_id_type":"open_id"}),
        ctl,
    )?;
    let mut threads = HashSet::new();
    persist(store, c, chat, &raw, &[], &mut threads)?;
    let mut root_threads = Vec::new();
    let mut complete = true;
    for (id, kind) in store.contexts(&c.app_id, chat)? {
        if ctl.done() {
            return Err(upstream("history interrupted"));
        }
        if kind == "thread" {
            threads.insert(id);
            continue;
        }
        let lookup = process::api(
            c,
            &format!("/open-apis/im/v1/messages/{id}"),
            json!({"user_id_type":"open_id"}),
            ctl,
        );
        let data = match lookup {
            Ok(data) => data,
            Err(e) if e.0 == 4 => return Err(e),
            Err(_) => {
                complete = false;
                eprintln!("remembered root unavailable; retained for retry");
                continue;
            }
        };
        let Some(items) = data["items"].as_array().filter(|items| !items.is_empty()) else {
            complete = false;
            eprintln!("remembered root unavailable; retained for retry");
            continue;
        };
        for item in items {
            if item["deleted"] != true {
                if let Some(thread) = item["thread_id"].as_str().filter(|s| !s.is_empty()) {
                    root_threads.push((id.clone(), thread.to_owned()));
                }
            }
        }
        // Explicit tombstones are irrecoverable; an empty/error response is
        // kept retryable because it does not prove permanent deletion.
        let retired = if items.iter().all(|v| v["deleted"] == true) {
            vec![id]
        } else {
            vec![]
        };
        persist(store, c, chat, items, &retired, &mut threads)?;
    }
    let mut scanned = HashSet::new();
    while let Some(id) = threads
        .iter()
        .filter(|id| !scanned.contains(*id))
        .min()
        .cloned()
    {
        if ctl.done() {
            return Err(upstream("history interrupted"));
        }
        let result = pages(
            c,
            json!({"container_id_type":"thread","container_id":id,"page_size":50,"sort_type":"ByCreateTimeAsc","user_id_type":"open_id"}),
            ctl,
        );
        scanned.insert(id.clone());
        let items = match result {
            Ok(items) => items,
            Err(e) if e.0 == 4 => return Err(e),
            Err(_) => {
                complete = false;
                eprintln!("thread history incomplete; retained for retry");
                continue;
            }
        };
        let retired = root_threads
            .iter()
            .filter(|(_, thread)| thread == &id)
            .map(|(root, _)| root.clone())
            .collect::<Vec<_>>();
        persist(store, c, chat, &items, &retired, &mut threads)?;
    }
    if ctl.done() {
        return Err(upstream("history interrupted"));
    }
    if complete {
        store.recovery(&c.app_id, chat, Some(end), vec![], &[], &[])?;
    }
    Ok(())
}

fn persist(
    store: &mut Store,
    c: &Config,
    chat: &str,
    raw: &[Value],
    retired: &[String],
    threads: &mut HashSet<String>,
) -> Result<()> {
    let mut messages = vec![];
    let mut ids = HashSet::new();
    let mut discovered = HashSet::new();
    for item in raw.iter().filter(|v| v["deleted"] != true) {
        if let Some(id) = item["thread_id"].as_str().filter(|s| !s.is_empty()) {
            if !valid_id(id, "omt_") && !valid_id(id, "om_") {
                return Err(upstream("invalid history thread identifier"));
            }
            discovered.insert(id.to_owned());
        }
        let event = model::history_event(item, chat).map_err(|s| Error(1, s))?;
        if let Some(v) = model::normalize_event(
            &c.app_id,
            &c.allowed_sender_open_ids,
            &c.allowed_chat_ids,
            &event,
        )
        .map_err(|s| Error(1, s))?
        {
            if ids.insert(v["message_id"].as_str().unwrap().to_owned()) {
                messages.push(v);
            }
        }
    }
    let known = discovered.iter().cloned().collect::<Vec<_>>();
    store.recovery(&c.app_id, chat, None, messages, &known, retired)?;
    threads.extend(discovered);
    Ok(())
}
