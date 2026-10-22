//! Synthetic argv cases; no Grok model is invoked.
use super::*;
use crate::agents::providers::launch as common;
fn words(input: &str) -> Vec<String> {
    common::parse_path_tokens(input).unwrap()
}
const ID: &str = "160d1f8b-9023-44b8-9bc7-24333effb185";
#[test]
fn grok_ad_adopts_only_one_unambiguous_resume_uuid() {
    for input in [
        format!("--model grok-build --resume {ID}"),
        format!("--resume={ID}"),
        format!("-r {ID}"),
    ] {
        let mut args = words(&input);
        assert_eq!(take_adopted_session(&mut args).unwrap(), Some(ID.into()));
        assert!(!args.iter().any(|s| s.contains(ID)));
    }
    for input in [
        format!("--session-id {ID}"),
        format!("-s {ID}"),
        format!("--resume {ID} --fork-session"),
        format!("--resume {ID} --continue"),
        format!("--resume {ID} -s {ID}"),
        format!("--resume {ID} --resume={ID}"),
        "--resume title".into(),
        "--resume".into(),
        format!("-- --resume {ID}"),
        format!("--rules '--resume' {ID}"),
        format!("--future-option value --resume {ID}"),
    ] {
        let mut args = words(&input);
        let original = args.clone();
        assert_eq!(take_adopted_session(&mut args).unwrap(), None, "{input}");
        assert_eq!(args, original);
    }
}
#[test]
fn grok_ad_rejects_only_demonstrated_modes_at_argument_positions() {
    for input in [
        "-p hello",
        "-phello",
        "--resume --single hi",
        "--cwd /tmp",
        "--cwd=/tmp",
        "--worktree=synthetic",
        "-w synthetic",
        "-wsynthetic",
        "--worktree",
        "--single=hello",
        "--prompt-file prompt.txt",
        "--prompt-json=[]",
        "--model grok-build agent stdio",
        "inspect --json",
        "export",
        "sessions",
        "--help",
        "--version",
        "--leader",
        "--minimal",
        "--no-alt-screen",
    ] {
        assert!(validate_args(&words(input)).is_err(), "accepted {input}");
    }
    for input in [
        "--permission-mode default --allow 'Bash(ls*)' --deny 'Bash(rm*)'",
        "--rules agent --model --single",
        "-- --single",
        "-- --cwd=/tmp",
        "--rules --cwd=/tmp",
        "--rules --worktree=synthetic",
        "--future-option value --single",
        "--model=unrecognized --effort high",
        "'an interactive prompt' --deny export",
    ] {
        assert!(validate_args(&words(input)).is_ok(), "rejected {input}");
    }
    assert!(validate_args(&["bad\0arg".into()]).is_err());
}
#[test]
fn grok_ad_shared_resume_retains_permissions_and_opaque_options() {
    let input=words(&format!("--session-id {ID} --permission-mode default --allow 'Bash(ls*)' --deny 'Bash(rm*)' 'initial prompt' --model grok-build --future-option opaque --resume literal"));
    assert_eq!(common::resumable_options(crate::agents::providers::ProviderKind::Grok,&input),words("--permission-mode default --allow 'Bash(ls*)' --deny 'Bash(rm*)' --model grok-build --future-option opaque --resume literal"));
}

#[test]
fn grok_ad_rejects_native_commands_after_an_initial_prompt() {
    for input in [
        "'initial synthetic prompt' agent stdio",
        "'initial synthetic prompt' export",
        "'initial synthetic prompt' --model grok-build inspect",
    ] {
        assert!(
            validate_args(&words(input)).is_err(),
            "accepted native command: {input}"
        );
    }
    for input in [
        "--rules agent 'initial synthetic prompt'",
        "-- 'literal prompt' agent stdio",
    ] {
        assert!(
            validate_args(&words(input)).is_ok(),
            "reinterpreted literal: {input}"
        );
    }
}

#[test]
fn grok_ad_rejects_short_clusters_that_disable_or_relocate_the_managed_terminal() {
    for input in ["-cpSYNTHETIC_UNUSED", "-cwSYNTHETIC_UNUSED", "-cv", "-ch"] {
        assert!(
            validate_args(&words(input)).is_err(),
            "accepted incompatible cluster: {input}"
        );
    }
    for input in [
        "-mcpSYNTHETIC_UNUSED",
        "--rules -cpSYNTHETIC_UNUSED",
        "-- -cwSYNTHETIC_UNUSED",
    ] {
        assert!(
            validate_args(&words(input)).is_ok(),
            "reinterpreted value: {input}"
        );
    }
}

#[test]
fn grok_ad_resume_splits_native_continue_model_cluster_without_replaying_selectors() {
    for input in [
        "-cmgrok-build",
        "-c -m grok-build",
        "-cm grok-build",
        "--continue --model grok-build",
    ] {
        assert_eq!(
            resumable_options(&words(input)),
            ["--model", "grok-build"],
            "{input}"
        );
    }
    assert_eq!(
        resumable_options(&words("--rules -cmgrok-build -c -- --single")),
        ["--rules", "-cmgrok-build"]
    );
    assert_eq!(
        resumable_options(&words("--future-option opaque -cmgrok-build -- --single")),
        [
            "--future-option",
            "opaque",
            "-cmgrok-build",
            "--",
            "--single"
        ]
    );
    assert!(resumable_options(&words("-- -cmgrok-build")).is_empty());
}
#[test]
fn grok_ad_continue_cluster_does_not_hide_modes_or_allow_uuid_adoption() {
    assert!(validate_args(&words("-cmgrok-build --single SYNTHETIC_UNUSED")).is_err());
    let mut args = words(&format!("--resume {ID} -cmgrok-build"));
    let original = args.clone();
    assert_eq!(take_adopted_session(&mut args).unwrap(), None);
    assert_eq!(args, original);
}
