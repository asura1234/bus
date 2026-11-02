use super::*;

#[test]
fn grok_idle_notification_requests_reconciliation_without_minting_a_turn() {
    let value = serde_json::json!({"hookEventName":"notification", "sessionId":"known", "notificationType":"idle_prompt"});
    let observed = parse(ProviderKind::Grok, &value).unwrap();
    assert_eq!(
        format!("{observed:?}"),
        "ReconcileSession { session: \"known\" }"
    );
}

#[test]
fn grok_shared_resume_delegates_clustered_selectors() {
    assert_eq!(
        launch::resumable_options(ProviderKind::Grok, &["-cmgrok-build".into()]),
        ["--model", "grok-build"]
    );
}

#[test]
fn grok_registration_has_stable_identity_and_observation_contract() {
    let provider: ProviderKind =
        serde_json::from_str("\"grok\"").expect("Grok provider registration");
    assert_eq!(provider.label(), "grok");
    assert_eq!(provider.executable(), "grok");
    let contract = hook_json::HookContract::for_provider(provider);
    assert_eq!(contract.adapter, "grok-hook");
    assert_eq!(
        contract.entry_shape,
        hook_json::HookEntryShape::NestedCommands
    );
    assert_eq!(
        contract.events,
        &[
            "SessionStart",
            "UserPromptSubmit",
            "Stop",
            "StopFailure",
            "StopCancelled",
            "Notification"
        ]
    );
}

#[test]
fn grok_registration_invalid_directory_never_initializes_capture() {
    let root = crate::utils::test_temp::unique_temp_path("bus-grok-g0");
    let initialized = std::cell::Cell::new(false);
    let spec = launch::LaunchSpec {
        provider: serde_json::from_str("\"grok\"").unwrap(),
        cwd: root.to_string_lossy().into_owned(),
        extra_args: String::new(),
        consent_project_hooks: true,
        hooks: hook_json::HookContext {
            binary: root.join("bus"),
            spool: root.join("capture"),
            launch_id: "g0-test".into(),
        },
    };
    let error = launch::prepare(&spec, |_| {
        initialized.set(true);
        Ok(())
    })
    .unwrap_err();
    assert!(!error.is_empty());
    assert!(!initialized.get());
    assert!(!root.exists());
}
