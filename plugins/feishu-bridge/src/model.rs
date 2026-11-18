use serde_json::{json, Value};

pub fn normalize_event(
    app: &str,
    senders: &[String],
    chats: &[String],
    event: &Value,
) -> Result<Option<Value>, String> {
    let s = |key: &str| event[key].as_str().unwrap_or("");
    if s("type") != "im.message.receive_v1"
        || s("sender_type") != "user"
        || s("chat_type") != "p2p"
        || s("message_type") != "text"
        || !senders.iter().any(|v| v == s("sender_id"))
        || !chats.iter().any(|v| v == s("chat_id"))
    {
        return Ok(None);
    }
    if !crate::config::valid_id(s("message_id"), "om_")
        || s("create_time")
            .parse::<i64>()
            .ok()
            .filter(|v| *v >= 0)
            .is_none()
        || !event["content"].is_string()
    {
        return Err("invalid authorized text event".into());
    }
    let ancestry = |k: &str| -> Result<Value, String> {
        match event.get(k) {
            None | Some(Value::Null) => Ok(Value::Null),
            Some(Value::String(s)) if s.is_empty() => Ok(Value::Null),
            Some(Value::String(s))
                if crate::config::valid_id(s, if k == "thread_id" { "omt_" } else { "om_" })
                    || (k == "thread_id" && crate::config::valid_id(s, "om_")) =>
            {
                Ok(json!(s))
            }
            _ => Err("invalid authorized ancestry".into()),
        }
    };
    Ok(Some(
        json!({"app_id":app,"message_id":s("message_id"),"chat_id":s("chat_id"),"chat_type":"p2p",
        "sender":{"open_id":s("sender_id"),"sender_type":"user"},"text":s("content"),
        "reply_to":ancestry("reply_to")?,"root_id":ancestry("root_id")?,"thread_id":ancestry("thread_id")?,"create_time":s("create_time")}),
    ))
}

pub fn history_event(v: &Value, chat: &str) -> Result<Value, String> {
    // The API's sender.id is usable only with an explicit open_id identity tag.
    let sender = if v["sender"]["id_type"] == "open_id" {
        v["sender"]["id"].clone()
    } else {
        Value::Null
    };
    let content = if v["msg_type"] == "text" {
        let body: Value = serde_json::from_str(v["body"]["content"].as_str().unwrap_or(""))
            .unwrap_or(Value::Null);
        body["text"].clone()
    } else {
        Value::Null
    };
    if v["chat_id"].as_str().is_some_and(|id| id != chat) {
        return Err("history chat mismatch".into());
    }
    Ok(
        json!({"type":"im.message.receive_v1","message_id":v["message_id"],"chat_id":chat,"chat_type":"p2p",
        "sender_id":sender,"sender_type":v["sender"]["sender_type"],"message_type":v["msg_type"],
        "content":content,"create_time":v["create_time"],"reply_to":v["parent_id"],"root_id":v["root_id"],"thread_id":v["thread_id"]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattened_fixture_preserves_decoded_content_and_all_ancestry() {
        let event: Value =
            serde_json::from_str(include_str!("../tests/fixtures/text-event.json")).unwrap();
        let result = normalize_event(
            "cli_test",
            &["ou_allowed".into()],
            &["oc_allowed".into()],
            &event,
        )
        .unwrap();
        assert_eq!(
            result,
            Some(
                json!({"app_id":"cli_test","message_id":"om_fixture","chat_id":"oc_allowed",
            "chat_type":"p2p","sender":{"open_id":"ou_allowed","sender_type":"user"},
            "text":"{\"text\":\"already decoded 雪\"}","reply_to":"om_direct","root_id":"om_root",
            "thread_id":"omt_thread","create_time":"1794470000000"})
            )
        );
    }

    #[test]
    fn forged_body_identity_cannot_authorize_unknown_sender() {
        let mut event: Value =
            serde_json::from_str(include_str!("../tests/fixtures/text-event.json")).unwrap();
        event["sender_id"] = json!("ou_stranger");
        event["content"] = json!("ou_allowed");
        assert_eq!(
            normalize_event(
                "cli_test",
                &["ou_allowed".into()],
                &["oc_allowed".into()],
                &event
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn raw_history_decodes_once_and_requires_explicit_open_id_type() {
        let mut raw = json!({"message_id":"om_raw","chat_id":"oc_allowed","msg_type":"text","create_time":"10",
            "sender":{"id":"ou_allowed","id_type":"user_id","sender_type":"user"},
            "body":{"content":"{\"text\":\"literal 雪\"}"},"parent_id":"om_parent"});
        let norm = |raw: &Value| {
            normalize_event(
                "cli_test",
                &["ou_allowed".into()],
                &["oc_allowed".into()],
                &history_event(raw, "oc_allowed").unwrap(),
            )
        };
        assert_eq!(norm(&raw).unwrap(), None);
        raw["sender"]["id_type"] = json!("open_id");
        let v = norm(&raw).unwrap().unwrap();
        assert_eq!(v["text"], "literal 雪");
        assert_eq!(v["reply_to"], "om_parent");
        raw["body"]["content"] = json!("not JSON");
        assert!(norm(&raw).is_err());
    }
}
