use super::{claude_code, codex, cursor, ProviderKind};
use crate::agents::providers::hook_json::{
    read_resume_json, resume_hooks, validate_resume_hooks, HookContext, HookContract,
    HookEntryShape,
};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    hooks: HookContext,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "bus-provider-resume-{}-{}",
            std::process::id(),
            crate::utils::time::now_ns(),
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self {
            hooks: HookContext {
                binary: root.join("bus with spaces"),
                spool: root.join("capture"),
                launch_id: "owned-launch".into(),
            },
            root,
        }
    }

    fn document(&self, provider: ProviderKind) -> Value {
        let contract = HookContract::for_provider(provider);
        let command = contract.command(&self.hooks.binary);
        let mut hooks = serde_json::Map::new();
        for event in contract.events {
            // Extra user fields/hooks are allowed by resume attestation even
            // though merge/rebind owns only exact Bus entry shapes.
            let entry = match contract.entry_shape {
                HookEntryShape::Command => json!({"command":command,"user":true}),
                HookEntryShape::NestedCommands => json!({"user":true,"hooks":[
                    {"type":"command","command":"user hook"},
                    {"type":"command","command":command,"timeout":19}
                ]}),
            };
            hooks.insert((*event).into(), json!([entry]));
        }
        json!({"hooks":hooks,"user_setting":"keep"})
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn resume_validation_accepts_matching_commands_inside_user_entries_without_rewriting() {
    let fixture = Fixture::new();
    for provider in [
        ProviderKind::ClaudeCode,
        ProviderKind::Codex,
        ProviderKind::Cursor,
    ] {
        let contract = HookContract::for_provider(provider);
        let path = fixture.root.join("hooks.json");
        let bytes = serde_json::to_vec(&fixture.document(provider)).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(validate_resume_hooks(&path, contract, &fixture.hooks.binary).is_ok());
        assert!(!resume_hooks(&path, contract, &fixture.hooks.binary).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn resume_validation_refuses_disabled_missing_and_oversize_configuration() {
    let fixture = Fixture::new();
    let path = fixture.root.join("hooks.json");
    let contract = HookContract::for_provider(ProviderKind::Codex);
    assert!(resume_hooks(&path, contract, &fixture.hooks.binary).is_err());
    assert!(!path.exists());
    let mut document = fixture.document(ProviderKind::Codex);
    document["disableAllHooks"] = json!(true);
    let bytes = serde_json::to_vec(&document).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(validate_resume_hooks(&path, contract, &fixture.hooks.binary).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::write(&path, vec![b' '; 2 * 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        read_resume_json(&path).unwrap_err(),
        "Bus resume capture configuration is too large"
    );
}

#[test]
fn resume_provider_capture_keeps_paths_and_argv_for_each_harness() {
    let fixture = Fixture::new();
    for provider in [
        ProviderKind::ClaudeCode,
        ProviderKind::Codex,
        ProviderKind::Cursor,
    ] {
        let path = match provider {
            ProviderKind::ClaudeCode => fixture.hooks.spool.join("claude-settings.json"),
            ProviderKind::Codex => fixture.root.join(".codex/hooks.json"),
            ProviderKind::Cursor => fixture.root.join(".cursor/hooks.json"),
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec(&fixture.document(provider)).unwrap(),
        )
        .unwrap();
        let (actual_path, args, rebound) = match provider {
            ProviderKind::ClaudeCode => {
                claude_code::resume::capture_args(&fixture.hooks, &fixture.root)
            }
            ProviderKind::Codex => codex::resume::capture_args(&fixture.hooks, &fixture.root),
            ProviderKind::Cursor => cursor::resume::capture_args(&fixture.hooks, &fixture.root),
        }
        .unwrap();
        assert_eq!(actual_path, path);
        assert!(!rebound);
        match provider {
            ProviderKind::ClaudeCode => assert_eq!(
                args,
                vec!["--settings".to_owned(), path.to_string_lossy().into_owned()]
            ),
            ProviderKind::Codex => assert_eq!(args, vec!["--no-daemon"]),
            ProviderKind::Cursor => assert!(args.is_empty()),
        }
    }
}
