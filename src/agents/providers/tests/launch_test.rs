// Included only in messaging::provider_glue::launch::tests to retain case IDs.
use crate::agents::providers::{
    hook_json::{self, HookContract},
    launch::{launch_args, runtime_args, validate_args},
    suggest::suggestions,
    ProviderKind as Provider,
};
use serde_json::{json, Value};
use std::path::Path;

mod files {
    pub(super) use crate::agents::providers::launch::parse_path_tokens;
}

mod bus_io {
    pub(super) use crate::agents::providers::hook_json::private_dir;
}

fn merge_hooks(document: Value, provider: Provider, binary: &Path) -> Result<Value, String> {
    hook_json::merge_hooks(document, HookContract::for_provider(provider), binary)
}

fn install_hooks(path: &Path, provider: Provider, binary: &Path) -> Result<(), String> {
    hook_json::install_hooks(path, HookContract::for_provider(provider), binary)
}

fn hook_command(binary: &Path, provider: Provider) -> String {
    HookContract::for_provider(provider).command(binary)
}

fn hook_entries(provider: Provider, binary: &Path) -> Vec<(&'static str, Value)> {
    let contract = HookContract::for_provider(provider);
    let command = contract.command(binary);
    contract
        .events
        .iter()
        .map(|event| (*event, contract.entry(&command)))
        .collect()
}

#[test]
fn launch_args_cannot_replace_hooks_or_bypass_trust() {
    for (provider, arg) in [
        (
            Provider::Codex,
            "--dangerously-bypass-approvals-and-sandbox",
        ),
        (Provider::ClaudeCode, "--settings"),
        (Provider::Cursor, "--force"),
    ] {
        assert!(validate_args(provider, &[arg.into()]).is_err(), "{arg}");
    }
    assert!(validate_args(Provider::Codex, &["--model".into(), "gpt-test".into()]).is_ok());
}

#[test]
fn provider_launch_options_reject_safeguard_aliases_and_noninteractive_commands() {
    for (provider, inputs) in [
        (
            Provider::Codex,
            vec![
                "-a never -s danger-full-access",
                "-anever",
                "-a=never",
                "-sdanger-full-access",
                "--ask-for-approval=never",
                "--sandbox=danger-full-access",
                "e",
                "review",
                "--model test e",
                "--model=test review",
                "-p untrusted-profile",
                "-cfeatures.hooks=false",
                "--remote unix:///tmp/other.sock",
                "--unknown-option=value",
                "-- review",
            ],
        ),
        (
            Provider::ClaudeCode,
            vec![
                "--bare",
                "--safe-mode",
                "--bg",
                "--background",
                "--dangerously-skip-permissions",
                "--allowed-tools Bash",
                "--settings=/tmp/hooks.json",
                "-phello",
                "--print=hello",
                "review",
                "--unknown-option=value",
            ],
        ),
        (
            Provider::Cursor,
            vec![
                "-f",
                "--force",
                "--yolo",
                "--auto-review",
                "--approve-mcps",
                "--sandbox=disabled",
                "--trust",
                "--plugin-dir=/tmp/hooks",
                "-phello",
                "worker",
                "--unknown-option=value",
            ],
        ),
    ] {
        for input in inputs {
            let args = files::parse_path_tokens(input).unwrap();
            assert!(
                validate_args(provider, &args).is_err(),
                "accepted {provider:?}: {input}"
            );
        }
    }
}

#[test]
fn provider_launch_options_preserve_supported_interactive_values_and_reject_ambiguous_syntax() {
    for (provider, input) in [
            (Provider::Codex, "-m gpt-test --no-alt-screen --strict-config"),
            (Provider::Codex, "-mgpt-test --oss --local-provider=ollama"),
            (Provider::Codex, "-m=gpt-test --search"),
            (Provider::ClaudeCode, "--model=sonnet --effort high --verbose --append-system-prompt 'Use literal @mentions and paths'"),
            (Provider::Cursor, "--model 'sonnet[effort=high]' --mode=plan"),
            (Provider::Cursor, "--plan"),
        ] {
            let args = files::parse_path_tokens(input).unwrap();
            assert!(validate_args(provider, &args).is_ok(), "rejected {provider:?}: {input}");
        }
    for (provider, input) in [
        (Provider::Codex, "--model --ask-for-approval=never"),
        (Provider::Codex, "--model="),
        (Provider::Codex, "--model"),
        (Provider::Codex, "--no-alt-screen=review"),
        (Provider::Codex, "--local-provider=unknown"),
        (Provider::ClaudeCode, "-m sonnet"),
        (Provider::ClaudeCode, "--effort=unknown"),
        (Provider::Cursor, "--mode=agent"),
        (Provider::Cursor, "--model sonnet review"),
        (Provider::Cursor, "--model=sonnet\0--force"),
    ] {
        let args = files::parse_path_tokens(input).unwrap();
        assert!(
            validate_args(provider, &args).is_err(),
            "accepted {provider:?}: {input}"
        );
    }
}

#[test]
fn launch_args_adopt_one_session_by_uuid_in_each_providers_resume_form() {
    let id = "160d1f8b-9023-44b8-9bc7-24333effb185";
    let owned = |args: &[&str]| args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        launch_args(
            Provider::ClaudeCode,
            &format!("--model sonnet --resume {id}")
        )
        .unwrap(),
        (
            owned(&["--resume", id, "--model", "sonnet"]),
            Some(id.into())
        )
    );
    assert_eq!(
        launch_args(Provider::Cursor, &format!("--resume={id}")).unwrap(),
        (owned(&["--resume", id]), Some(id.into()))
    );
    assert_eq!(
        launch_args(Provider::Codex, &format!("resume {id} -m gpt-test")).unwrap(),
        (
            owned(&["resume", id, "-m", "gpt-test", "--no-daemon"]),
            Some(id.into())
        )
    );
    // A later "resume" is an option value, not the subcommand.
    assert_eq!(
        launch_args(Provider::Codex, "--model resume").unwrap(),
        (owned(&["--model", "resume", "--no-daemon"]), None)
    );
    for (provider, input) in [
        (Provider::ClaudeCode, "--resume"),
        (Provider::ClaudeCode, "--resume not-a-uuid"),
        (
            Provider::ClaudeCode,
            "--resume 160d1f8b-9023-44b8-9bc7-24333effb18",
        ),
        (
            Provider::ClaudeCode,
            "--resume 160d1f8b_9023-44b8-9bc7-24333effb185",
        ),
        (
            Provider::ClaudeCode,
            &format!("--resume {id} --resume {id}"),
        ),
        (Provider::ClaudeCode, &format!("--resume {id} --continue")),
        (
            Provider::ClaudeCode,
            &format!("--resume {id} --fork-session"),
        ),
        (
            Provider::ClaudeCode,
            "-r 160d1f8b-9023-44b8-9bc7-24333effb185",
        ),
        (Provider::ClaudeCode, "--continue"),
        (Provider::ClaudeCode, &format!("--session-id {id}")),
        (Provider::Codex, "resume --last"),
        (Provider::Codex, "resume"),
        (Provider::Codex, &format!("resume {id} resume {id}")),
        (Provider::Codex, &format!("-m gpt resume {id}")),
        (Provider::Codex, &format!("resume {id} --all")),
        (Provider::Cursor, "--resume --continue"),
        (Provider::Cursor, "--continue"),
    ] {
        assert!(
            launch_args(provider, input).is_err(),
            "accepted {provider:?}: {input}"
        );
    }
}

#[test]
fn codex_launches_host_their_own_runtime_so_hooks_inherit_launch_env() {
    assert_eq!(
        runtime_args(Provider::Codex),
        vec!["--no-daemon".to_owned()]
    );
    assert!(runtime_args(Provider::ClaudeCode).is_empty());
    assert!(runtime_args(Provider::Cursor).is_empty());
    // Bus owns this flag; a user-supplied copy stays outside the allowlist.
    assert!(validate_args(Provider::Codex, &["--no-daemon".into()]).is_err());
}

include!("hook_json_test.rs");
include!("suggest_test.rs");
