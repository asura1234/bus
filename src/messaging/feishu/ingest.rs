//! Version 1 sink input; transport payloads are authenticated before routing.
use crate::messaging::model::feishu::IncomingReply;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Message {
    app_id: String,
    message_id: String,
    chat_id: String,
    chat_type: String,
    sender: Sender,
    text: String,
    reply_to: NullableText,
    root_id: NullableText,
    thread_id: NullableText,
    create_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sender {
    open_id: String,
    sender_type: String,
}

/// The v1 keys are required even when their value is null.
#[derive(Debug)]
struct NullableText(Option<String>);

impl<'de> Deserialize<'de> for NullableText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Null => Ok(Self(None)),
            serde_json::Value::String(text) => Ok(Self(Some(text))),
            _ => Err(serde::de::Error::custom("expected a string or null")),
        }
    }
}

impl Message {
    pub(crate) fn normalize(self, expected_app: &str) -> Result<IncomingReply, String> {
        if !super::valid_id(expected_app, "cli_") || self.app_id != expected_app {
            return Err("Feishu app does not match the configured sink app".into());
        }
        if self.sender.sender_type != "user" || self.chat_type != "p2p" {
            return Err("Feishu sink accepts only user messages in a p2p chat".into());
        }
        if !super::valid_id(&self.message_id, "om_")
            || !super::valid_id(&self.chat_id, "oc_")
            || !super::valid_id(&self.sender.open_id, "ou_")
            || self
                .reply_to
                .0
                .as_deref()
                .is_some_and(|id| !super::valid_id(id, "om_"))
            || self
                .root_id
                .0
                .as_deref()
                .is_some_and(|id| !super::valid_id(id, "om_"))
            || self
                .thread_id
                .0
                .as_deref()
                .is_some_and(|id| id.is_empty() || id.chars().any(char::is_control))
        {
            return Err("Feishu sink message contains invalid identifiers".into());
        }
        if self.text.trim().is_empty() {
            return Err("Feishu sink message text is empty".into());
        }
        let create_time = self
            .create_time
            .parse::<u64>()
            .ok()
            .filter(|value| *value <= i64::MAX as u64)
            .filter(|_| {
                !self.create_time.is_empty() && self.create_time.bytes().all(|b| b.is_ascii_digit())
            })
            .ok_or("Feishu sink create_time must be nonnegative epoch milliseconds")?;
        Ok(IncomingReply {
            app_id: self.app_id,
            message_id: self.message_id,
            chat_id: self.chat_id,
            sender_id: self.sender.open_id,
            text: self.text,
            reply_to: self.reply_to.0,
            root_id: self.root_id.0,
            create_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wire() -> serde_json::Value {
        json!({"app_id":"cli_test","message_id":"om_reply","chat_id":"oc_dm","chat_type":"p2p",
            "sender":{"open_id":"ou_developer","sender_type":"user"},"text":"literal decoded text",
            "reply_to":"om_parent","root_id":null,"thread_id":null,"create_time":"1794470000000"})
    }

    #[test]
    fn feishu_wire_preserves_literal_text_and_normalizes_only_the_timestamp() {
        let mut wire = wire();
        wire["text"] = json!("  {\"text\":\"界 $HOME `command`\"}\nProceed");
        let input = serde_json::from_value::<Message>(wire.clone())
            .unwrap()
            .normalize("cli_test")
            .unwrap();
        assert_eq!(input.text, "  {\"text\":\"界 $HOME `command`\"}\nProceed");
        assert_eq!(input.create_time, 1_794_470_000_000);
        assert_eq!(input.reply_to.as_deref(), Some("om_parent"));
        assert_eq!(input.root_id, None);
    }

    #[test]
    fn feishu_wire_refuses_wrong_identity_context_and_invalid_ids_or_time() {
        for (field, value) in [
            ("app_id", json!("cli_other")),
            ("chat_type", json!("group")),
            ("message_id", json!("om_")),
            ("chat_id", json!("oc_ dm")),
            ("reply_to", json!("omt_thread")),
            ("root_id", json!("")),
            ("text", json!("  \n")),
            ("create_time", json!("-1")),
            ("create_time", json!("+1")),
            ("create_time", json!("9223372036854775808")),
        ] {
            let mut value_wire = wire();
            value_wire[field] = value;
            let message: Message = serde_json::from_value(value_wire).unwrap();
            assert!(message.normalize("cli_test").is_err(), "{field}");
        }
        for sender in [
            json!({"open_id":"ou_developer","sender_type":"app"}),
            json!({"open_id":"unknown","sender_type":"user"}),
        ] {
            let mut wire = wire();
            wire["sender"] = sender;
            assert!(serde_json::from_value::<Message>(wire)
                .unwrap()
                .normalize("cli_test")
                .is_err());
        }
    }

    #[test]
    fn feishu_wire_requires_v1_keys_and_their_exact_types() {
        for field in [
            "reply_to",
            "root_id",
            "thread_id",
            "app_id",
            "sender",
            "create_time",
        ] {
            let mut wire = wire();
            wire.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<Message>(wire).is_err(),
                "missing {field}"
            );
        }
        for (field, value) in [
            ("create_time", json!(20)),
            ("reply_to", json!(true)),
            ("unexpected", json!("field")),
        ] {
            let mut wire = wire();
            wire[field] = value;
            assert!(serde_json::from_value::<Message>(wire).is_err(), "{field}");
        }
    }
}
