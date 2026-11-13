use super::types::{FeishuState, IncomingReply, OutboxRecord};
use std::collections::BTreeSet;

impl FeishuState {
    /// Every known reference must agree; an unknown parent can fall back to its root.
    pub(crate) fn resolve(&self, input: &IncomingReply) -> Result<&OutboxRecord, String> {
        let mut codes = BTreeSet::new();
        for message in input.reply_to.iter().chain(input.root_id.iter()) {
            codes.extend(
                self.outbox
                    .values()
                    .filter(|record| record.message_id.as_ref() == Some(message))
                    .map(|record| record.code.as_str()),
            );
            if let Some(code) = self
                .inbox
                .get(&format!("{}:{message}", input.app_id))
                .and_then(|record| record.code.as_deref())
            {
                codes.insert(code);
            }
        }
        for word in input.text.split_whitespace() {
            let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':');
            if let Some(code) = word.strip_prefix("bus:") {
                if !self.outbox.contains_key(code) {
                    return Err("Unknown Feishu request code".into());
                }
                codes.insert(code);
            }
        }
        if codes.len() != 1 {
            return Err(if codes.is_empty() {
                "Unknown Feishu reply"
            } else {
                "Ambiguous Feishu reply references"
            }
            .into());
        }
        let code = codes.first().ok_or("Unknown Feishu reply")?;
        self.outbox
            .get(*code)
            .ok_or_else(|| "Unknown Feishu request code".into())
    }
}
