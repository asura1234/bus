// Included only in messaging::provider_glue::launch::tests to retain case IDs.
#[path = "yolo_options_test.rs"]
mod yolo_options;

#[test]
fn fake_provider_is_available_only_to_an_isolated_dev_driver() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let previous_flag = std::env::var_os("BUS_TUI_FAKE_AGENT");
    let previous_owner = std::env::var_os("BUS_TUI_OWNER");
    std::env::remove_var("BUS_TUI_FAKE_AGENT");
    std::env::remove_var("BUS_TUI_OWNER");
    assert!(crate::agents::providers::launch::native_provider("fake").is_none());
    std::env::set_var("BUS_TUI_FAKE_AGENT", "1");
    std::env::set_var("BUS_TUI_OWNER", "isolated-test");
    assert_eq!(
        crate::agents::providers::launch::native_provider("fake").map(|p| p.label()),
        if cfg!(feature = "dev-tools") {
            Some("fake")
        } else {
            None
        }
    );
    match previous_flag {
        Some(value) => std::env::set_var("BUS_TUI_FAKE_AGENT", value),
        None => std::env::remove_var("BUS_TUI_FAKE_AGENT"),
    }
    match previous_owner {
        Some(value) => std::env::set_var("BUS_TUI_OWNER", value),
        None => std::env::remove_var("BUS_TUI_OWNER"),
    }
}
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
        (
            owned(&["--resume", id, "--disable-auto-update"]),
            Some(id.into())
        )
    );
    assert_eq!(
        launch_args(Provider::Codex, &format!("resume {id} -m gpt-test")).unwrap(),
        (
            owned(&[
                "resume",
                id,
                "-m",
                "gpt-test",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "-c",
                "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
                "-c",
                "tui.keymap.editor.delete_backward=[\"backspace\"]"
            ]),
            Some(id.into())
        )
    );
    // A later "resume" is an option value, not the subcommand.
    assert_eq!(
        launch_args(Provider::Codex, "--model resume").unwrap(),
        (
            owned(&[
                "--model",
                "resume",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "-c",
                "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
                "-c",
                "tui.keymap.editor.delete_backward=[\"backspace\"]"
            ]),
            None
        )
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
        vec![
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
            "-c",
            "tui.keymap.editor.delete_backward=[\"backspace\"]"
        ]
    );
    assert!(runtime_args(Provider::ClaudeCode).is_empty());
    assert_eq!(
        runtime_args(Provider::Cursor),
        vec!["--disable-auto-update"]
    );
    assert!(validate_args(Provider::Codex, &["--no-daemon".into()]).is_ok());
}

#[test]
fn codex_launch_does_not_duplicate_an_explicit_private_runtime_option() {
    assert_eq!(
        launch_args(Provider::Codex, "--yolo --no-daemon")
            .unwrap()
            .0,
        vec![
            "--yolo",
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
            "-c",
            "tui.keymap.editor.delete_backward=[\"backspace\"]"
        ]
    );
    assert_eq!(
        launch_args(Provider::Codex, "'first prompt' --no-daemon")
            .unwrap()
            .0,
        vec![
            "first prompt",
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
            "-c",
            "tui.keymap.editor.delete_backward=[\"backspace\"]"
        ]
    );
}

#[test]
fn codex_capture_runtime_precedes_a_literal_argument_tail() {
    let (args, adopted) = launch_args(Provider::Codex, "-- '--yolo literal prompt'").unwrap();
    assert_eq!(adopted, None);
    assert_eq!(
        args,
        [
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
            "-c",
            "tui.keymap.editor.delete_backward=[\"backspace\"]",
            "--",
            "--yolo literal prompt"
        ]
    );
}

#[test]
fn generated_args_do_not_split_an_option_from_a_value_named_separator() {
    let (args, _) = launch_args(Provider::Codex, "--model '--'").unwrap();
    assert_eq!(
        args,
        [
            "--model",
            "--",
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
            "-c",
            "tui.keymap.editor.delete_backward=[\"backspace\"]"
        ]
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
            vec![
                "resume",
                id,
                "--yolo",
                "-c",
                "key=value",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "-c",
                "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
                "-c",
                "tui.keymap.editor.delete_backward=[\"backspace\"]"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            Some(id.into())
        )
    );
}

include!("hook_json_test.rs");
include!("suggest_test.rs");

#[test]
fn yolo_preserves_explicit_bypass_after_opaque_options_and_ignores_literal_values() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    let mut failures = Vec::new();
    for (provider, flag) in [
        (Provider::ClaudeCode, "--dangerously-skip-permissions"),
        (Provider::Codex, "--yolo"),
        (Provider::Cursor, "--force"),
    ] {
        for (input, expected) in [
            (
                vec!["--future-option", "value", flag],
                vec!["--future-option", "value", flag],
            ),
            (vec!["--model", flag], vec![flag, "--model", flag]),
            (vec!["--", flag], vec![flag, "--", flag]),
            (vec!["--model", "--"], vec![flag, "--model", "--"]),
        ] {
            let mut args: Vec<_> = input.iter().map(|a| (*a).to_owned()).collect();
            crate::agents::providers::launch::apply_run_permissions(provider, &mut args);
            if args != expected {
                failures.push(format!(
                    "{provider:?}: {input:?} -> {args:?}, want {expected:?}"
                ));
            }
        }
    }
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn yolo_adds_bypass_when_prompt_value_follows_current_unmodeled_claude_option() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    // --brief is a current Claude flag. The later bypass spelling belongs to
    // --append-system-prompt and must not suppress the run's real option.
    let mut args = vec![
        "--brief".to_owned(),
        "--append-system-prompt".to_owned(),
        "--dangerously-skip-permissions".to_owned(),
    ];
    crate::agents::providers::launch::apply_run_permissions(Provider::ClaudeCode, &mut args);
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert_eq!(
        args,
        [
            "--dangerously-skip-permissions",
            "--brief",
            "--append-system-prompt",
            "--dangerously-skip-permissions",
        ]
    );
}

#[test]
fn yolo_codex_fresh_launch_saves_approval_options_before_applying_bypass() {
    use crate::agents::providers::launch::{prepare, LaunchSpec};

    struct Cleanup {
        root: std::path::PathBuf,
        env: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for (key, value) in &self.env {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let cleanup = Cleanup {
        root: crate::utils::test_temp::unique_temp_path("codex-fresh-yolo-options"),
        env: ["BUS_YOLO", "PATH"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    };
    let project = cleanup.root.join("project");
    let bin = cleanup.root.join("bin");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("codex"), "fixture executable").unwrap();
    std::env::set_var("PATH", &bin);
    let spec = LaunchSpec {
        provider: Provider::Codex,
        cwd: project.display().to_string(),
        extra_args: "--ask-for-approval on-request -c 'developer_instructions=\"--ask-for-approval\"' --sandbox workspace-write -- '--approve-for-me'".into(),
        consent_project_hooks: true,
        hooks: hook_json::HookContext {
            binary: std::env::current_exe().unwrap(),
            spool: cleanup.root.join("capture"),
            launch_id: "fresh-launch".into(),
        },
    };
    let original = [
        "--ask-for-approval",
        "on-request",
        "-c",
        "developer_instructions=\"--ask-for-approval\"",
        "--sandbox",
        "workspace-write",
        "--",
        "--approve-for-me",
    ];
    let saved = serde_json::to_vec(&original).unwrap();
    for enabled in [true, false] {
        if enabled {
            std::env::set_var("BUS_YOLO", "1");
        } else {
            std::env::remove_var("BUS_YOLO");
        }
        let prepared = prepare(&spec, |hooks| {
            std::fs::create_dir_all(&hooks.spool).map_err(|error| error.to_string())
        })
        .unwrap();
        let expected: Vec<&str> = if enabled {
            vec![
                "--yolo",
                "-c",
                "developer_instructions=\"--ask-for-approval\"",
                "--sandbox",
                "workspace-write",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "-c",
                "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
                "-c",
                "tui.keymap.editor.delete_backward=[\"backspace\"]",
                "--",
                "--approve-for-me",
            ]
        } else {
            vec![
                "--ask-for-approval",
                "on-request",
                "-c",
                "developer_instructions=\"--ask-for-approval\"",
                "--sandbox",
                "workspace-write",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "-c",
                "tui.keymap.global.open_external_editor=[\"ctrl-g\"]",
                "-c",
                "tui.keymap.editor.delete_backward=[\"backspace\"]",
                "--",
                "--approve-for-me",
            ]
        };
        assert_eq!(prepared.args, expected, "enabled={enabled}");
        assert_eq!(
            std::fs::read(spec.hooks.spool.join("launch-options.json")).unwrap(),
            saved,
            "effective permissions must not replace the saved user options"
        );
    }
}

#[test]
fn cursor_disables_updates_on_new_and_adopted_launches() {
    for input in ["", "--resume=160d1f8b-9023-44b8-9bc7-24333effb185"] {
        let (args, _) = launch_args(Provider::Cursor, input).unwrap();
        assert!(
            args.iter().any(|arg| arg == "--disable-auto-update"),
            "{args:?}"
        );
    }
}

#[test]
fn repeated_runtime_defaults_do_not_duplicate_update_arguments_or_consume_literal_text() {
    for last in [
        "-c",
        "--config",
        "--config=check_for_update_on_startup=true",
    ] {
        let mut args = vec![
            "-c".into(),
            "check_for_update_on_startup=false".into(),
            last.into(),
        ];
        if last != "--config=check_for_update_on_startup=true" {
            args.push("check_for_update_on_startup=true".into());
        }
        crate::agents::providers::launch::extend_generated_args(
            Provider::Codex,
            &mut args,
            runtime_args(Provider::Codex),
        );
        assert_eq!(
            args.windows(2)
                .filter(|pair| pair[0] == "-c" && pair[1] == "check_for_update_on_startup=false")
                .count(),
            2,
            "a later explicit enable requires one final disabling override: {args:?}"
        );
    }
    for provider in [Provider::Codex, Provider::Cursor] {
        let mut args = vec![
            "--".into(),
            "--disable-auto-update".into(),
            "check_for_update_on_startup=false".into(),
        ];
        let literal = args.clone();
        crate::agents::providers::launch::extend_generated_args(
            provider,
            &mut args,
            runtime_args(provider),
        );
        let once = args.clone();
        crate::agents::providers::launch::extend_generated_args(
            provider,
            &mut args,
            runtime_args(provider),
        );
        if provider == Provider::Cursor {
            assert_eq!(args, once, "{provider:?}");
        } else {
            assert_eq!(
                args.iter()
                    .filter(|arg| arg.as_str() == "--no-daemon")
                    .count(),
                1
            );
            assert_eq!(
                args.iter()
                    .filter(|arg| arg.as_str() == "check_for_update_on_startup=false")
                    .count(),
                2
            );
        }
        assert_eq!(&args[args.len() - literal.len()..], literal);
    }
}

#[test]
fn codex_update_suppression_overrides_explicit_enable_before_literal_prompt() {
    for prefix in ["", "--vendor-option value "] {
        for config in [
            "-c check_for_update_on_startup=true",
            "--config check_for_update_on_startup=true",
            "--config=check_for_update_on_startup=true",
        ] {
            let (mut args, _) = launch_args(
                Provider::Codex,
                &format!("{prefix}{config} -- 'initial prompt'"),
            )
            .unwrap();
            let separator = args.iter().position(|arg| arg == "--").unwrap();
            let update_values: Vec<_> = args[..separator]
                .iter()
                .filter_map(|arg| {
                    arg.strip_prefix("check_for_update_on_startup=")
                        .or_else(|| arg.strip_prefix("--config=check_for_update_on_startup="))
                })
                .collect();
            assert_eq!(&args[separator..], ["--", "initial prompt"]);
            assert_eq!(
                update_values.last().copied(),
                Some("false"),
                "the effective startup-update setting must stay disabled: {args:?}"
            );
            crate::agents::providers::launch::extend_generated_args(
                Provider::Codex,
                &mut args,
                runtime_args(Provider::Codex),
            );
            assert_eq!(
                args.iter()
                    .filter(|arg| arg.as_str() == "--no-daemon")
                    .count(),
                1
            );
            assert_eq!(
                args.iter()
                    .filter(|arg| arg.as_str() == "check_for_update_on_startup=false")
                    .count(),
                1
            );
            assert_eq!(&args[args.len() - 2..], ["--", "initial prompt"]);
        }
    }
}
