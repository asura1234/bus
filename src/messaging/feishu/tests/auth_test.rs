use super::*;
use serde_json::json;

fn status() -> Value {
    json!({"verified":true,"identity":"user","identities":{
        "bot":{"available":true,"verified":true,"openId":"ou_bot"},
        "user":{"status":"ready","available":true,"verified":true,
            "openId":"ou_authenticated_self","scope":"im:message.send_as_user im:message"}
    }})
}

#[test]
fn feishu_auth_resolves_only_verified_user_self_in_real_status_shape() {
    let value = status();
    assert_eq!(parse(&value).unwrap().open_id(), "ou_authenticated_self");
    let mut refreshed = value;
    refreshed["identities"]["user"]["status"] = json!("needs_refresh");
    assert_eq!(
        parse(&refreshed).unwrap().open_id(),
        "ou_authenticated_self"
    );
}

#[test]
fn feishu_auth_never_uses_bot_top_level_or_configured_recipient_as_self() {
    let mut value = status();
    value["identities"].as_object_mut().unwrap().remove("user");
    value["openId"] = json!("ou_other");
    value["recipient"] = json!("ou_other");
    assert!(parse(&value).is_err());
    for field in ["available", "verified"] {
        let mut value = status();
        value["identities"]["user"][field] = json!(false);
        assert!(parse(&value).is_err());
    }
    for id in [
        "",
        "oc_a_group",
        "--user-id ou_other",
        "ou_self\n--chat-id other",
    ] {
        let mut value = status();
        value["identities"]["user"]["openId"] = json!(id);
        assert!(parse(&value).is_err());
    }
}

#[test]
fn feishu_auth_requires_both_documented_user_send_scopes() {
    for scope in [
        "",
        "im:message",
        "im:message.send_as_user",
        "im:message:readonly",
    ] {
        let mut value = status();
        value["identities"]["user"]["scope"] = json!(scope);
        assert!(parse(&value).is_err(), "{scope}");
    }
    let mut value = status();
    value["identities"]["user"]["scope"] = json!("im:chat:read im:message im:message.send_as_user");
    assert_eq!(parse(&value).unwrap().open_id(), "ou_authenticated_self");
}
