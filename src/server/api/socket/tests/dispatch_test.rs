#[cfg(test)]
#[test]
fn caller_timeout_dispatch_uses_timeout_error() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let response = dispatch_to_app_with_caller_timeout(
        Request {
            id: "prompt-timeout".into(),
            method: Method::AgentPrompt(crate::protocol::api::schema::AgentPromptParams {
                target: "reviewer".into(),
                text: "review this".into(),
                wait: None,
            }),
        },
        &tx,
        Some(Duration::ZERO),
    );
    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "timeout");
}
