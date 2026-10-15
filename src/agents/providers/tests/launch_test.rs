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
fn launch_args_preserve_permission_configuration_and_unknown_options() {
    for (provider, input) in [
        (Provider::Codex, "--yolo -c 'model_reasoning_effort=\"high\"' --sandbox danger-full-access --ask-for-approval never -a=never -sdanger-full-access --dangerously-bypass-approvals-and-sandbox"),
        (Provider::ClaudeCode, "--dangerously-skip-permissions --permission-mode bypassPermissions --settings=/tmp/user.json --allowed-tools Bash --add-dir /work/sibling"),
        (Provider::Cursor, "--force --yolo --sandbox=disabled --trust --approve-mcps --plugin-dir /tmp/plugin"),
        (Provider::Codex, "--future-option future-value --model="),
        (Provider::ClaudeCode, "--future-option=anything --effort=unknown"),
        (Provider::Cursor, "--future-option=anything --mode=agent"),
    ] {
        let mut expected = files::parse_path_tokens(input).unwrap();
        expected.extend(runtime_args(provider));
        assert_eq!(launch_args(provider, input).unwrap(), (expected, None), "{provider:?}: {input}");
    }
}

#[test]
fn provider_launch_options_reject_only_proven_noninteractive_or_hookless_modes() {
    for (provider, inputs) in [
        (
            Provider::Codex,
            vec![
                "exec",
                "e",
                "review",
                "--yolo exec",
                "-c key=value exec",
                "--model test e",
                "--model=test review",
            ],
        ),
        (
            Provider::ClaudeCode,
            vec![
                "--bare",
                "--safe-mode",
                "--bg",
                "--background",
                "-p",
                "--print",
                "--dangerously-skip-permissions --print",
            ],
        ),
        (Provider::Cursor, vec!["-p", "--print", "--yolo --print"]),
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
fn provider_launch_options_do_not_misread_values_or_positional_prompts_as_modes() {
    for (provider, input) in [
            (Provider::Codex, "-m gpt-test --no-alt-screen --strict-config"),
            (Provider::Codex, "-mgpt-test --oss --local-provider=ollama"),
            (Provider::Codex, "-m=gpt-test --search"),
            (Provider::Codex, "--add-dir /work/bus-sibling -m gpt-test --add-dir=/work/other"),
            (Provider::ClaudeCode, "--model=sonnet --effort high --verbose --append-system-prompt 'Use literal @mentions and paths'"),
            (Provider::Cursor, "--model 'sonnet[effort=high]' --mode=plan"),
            (Provider::Cursor, "--plan"),
            (Provider::Codex, "--model exec"),
            (Provider::Codex, "-c 'key=\"exec\"' --model review"),
            (Provider::Codex, "-- exec"),
            (Provider::ClaudeCode, "--system-prompt '--resume' --model '--print'"),
            (Provider::ClaudeCode, "--append-system-prompt '--bare'"),
            (Provider::ClaudeCode, "review"),
            (Provider::Cursor, "--model '--print'"),
            (Provider::Cursor, "worker"),
            (Provider::Cursor, "-- --print"),
        ] {
            let args = files::parse_path_tokens(input).unwrap();
            assert!(validate_args(provider, &args).is_ok(), "rejected {provider:?}: {input}");
        }
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        assert!(validate_args(provider, &["--unknown=value\0suffix".into()]).is_err());
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
            launch_args(provider, input).is_ok(),
            "rejected {provider:?}: {input}"
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
    assert!(validate_args(Provider::Codex, &["--no-daemon".into()]).is_ok());
}

#[test]
fn codex_launch_does_not_duplicate_an_explicit_private_runtime_option() {
    assert_eq!(
        launch_args(Provider::Codex, "--yolo --no-daemon")
            .unwrap()
            .0,
        vec!["--yolo", "--no-daemon"]
    );
}

#[test]
fn launch_args_adoption_distinguishes_selectors_from_literal_option_values() {
    let id = "160d1f8b-9023-44b8-9bc7-24333effb185";
    for (provider, input) in [
        (
            Provider::ClaudeCode,
            "--system-prompt '--resume'".to_owned(),
        ),
        (
            Provider::ClaudeCode,
            "--system-prompt '--resume=not-a-selector'".to_owned(),
        ),
        (Provider::Cursor, "--model '--resume'".to_owned()),
        (Provider::Codex, "-c 'key=\"resume\"'".to_owned()),
    ] {
        let mut expected = files::parse_path_tokens(&input).unwrap();
        expected.extend(runtime_args(provider));
        assert_eq!(launch_args(provider, &input).unwrap(), (expected, None));
    }
    assert_eq!(
        launch_args(Provider::Codex, &format!("--yolo -c key=value resume {id}")).unwrap(),
        (
            vec!["resume", id, "--yolo", "-c", "key=value", "--no-daemon"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Some(id.into())
        )
    );
}

include!("hook_json_test.rs");
include!("suggest_test.rs");
