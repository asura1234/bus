use serde_json::Value;

#[derive(Clone)]
pub(super) struct SelfIdentity {
    open_id: String,
}

impl SelfIdentity {
    pub(super) fn open_id(&self) -> &str {
        &self.open_id
    }
}

pub(super) fn parse(value: &Value) -> Result<SelfIdentity, String> {
    let user = &value["identities"]["user"];
    if user["available"] != true {
        return Err("lark-cli has no authenticated user; run lark-cli auth login".into());
    }
    if value["verified"] != true || user["verified"] != true {
        return Err("lark-cli could not verify the authenticated user".into());
    }
    if !matches!(user["status"].as_str(), Some("ready" | "needs_refresh")) {
        return Err("lark-cli user authentication is not ready".into());
    }
    let id = user["openId"]
        .as_str()
        .filter(|id| {
            id.strip_prefix("ou_").is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        })
        .ok_or("lark-cli did not report a valid authenticated self openId")?;
    let scopes: std::collections::BTreeSet<_> = user["scope"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if !["im:message.send_as_user", "im:message"]
        .iter()
        .all(|scope| scopes.contains(scope))
    {
        return Err("Feishu notifications require user scopes im:message.send_as_user and im:message; run lark-cli auth login --scope 'im:message.send_as_user im:message'".into());
    }
    Ok(SelfIdentity {
        open_id: id.to_owned(),
    })
}

#[cfg(test)]
#[path = "tests/auth_test.rs"]
mod tests;
