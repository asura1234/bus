use super::queue_resumed_agent_launch;
use crate::terminal::{runtime::PaneLaunchEnv, TerminalRuntime};

fn native_environment() -> PaneLaunchEnv {
    PaneLaunchEnv::from_extra(vec![
        ("BUS_CODEX_NATIVE".into(), "1".into()),
        (
            "BUS_CALLBACK_DIR".into(),
            std::env::temp_dir()
                .join("bus-cold-resume-capture")
                .display()
                .to_string(),
        ),
    ])
}

#[tokio::test]
async fn cold_native_resume_launch_wraps_the_saved_conversation_argv() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(120, 40, 4);
    let argv = [
        "codex",
        "resume",
        "160d1f8b-9023-44b8-9bc7-24333effb185",
        "--cd",
        "../target project/nested",
    ]
    .map(str::to_owned);
    queue_resumed_agent_launch(None, &runtime, &native_environment(), &argv).unwrap();
    let command = String::from_utf8(writes.try_recv().unwrap().to_vec()).unwrap();
    assert!(
        command.contains("--bus-codex-runtime"),
        "cold resume bypassed the native helper: {command}"
    );
    assert!(
        runtime.shared_codex_session_open(),
        "the restored pane did not retain native lifetime ownership"
    );
    assert!(command.contains("160d1f8b-9023-44b8-9bc7-24333effb185"));
    assert!(command.contains("../target project/nested"));
    runtime.shutdown();
}

#[tokio::test]
async fn cold_resume_preserves_profile_permission_and_foreign_provider_launches() {
    for argv in [
        vec![
            "codex",
            "resume",
            "saved-thread",
            "--profile",
            "team profile",
        ],
        vec!["codex", "resume", "saved-thread", "--sandbox", "read-only"],
        vec!["claude", "--resume", "saved-thread"],
    ] {
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(120, 40, 4);
        let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
        queue_resumed_agent_launch(None, &runtime, &native_environment(), &argv).unwrap();
        let command = String::from_utf8(writes.try_recv().unwrap().to_vec()).unwrap();
        assert!(
            !command.contains("--bus-codex-runtime"),
            "unsupported launch was rewritten: {command}"
        );
        assert!(!runtime.shared_codex_session_open());
        for arg in argv {
            assert!(command.contains(&arg), "resume argument lost: {command}");
        }
        runtime.shutdown();
    }
}
