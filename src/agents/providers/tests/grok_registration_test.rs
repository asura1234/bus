use super::*;

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
    let root = std::env::temp_dir().join(format!(
        "bus-grok-g0-{}-{}",
        std::process::id(),
        crate::utils::time::now_ns()
    ));
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
