use super::*;

const COMMAND: &str = include_str!("../../../../../tests/fixtures/kimi-steer/command.txt");
const THINKING: &str = include_str!("../../../../../tests/fixtures/kimi-steer/thinking.txt");
const IDLE: &str = include_str!("../../../../../tests/fixtures/kimi-steer/idle.txt");
const APPROVAL: &str = include_str!("../../../../../tests/fixtures/kimi-steer/approval.txt");

struct Scratch {
    _env: crate::utils::test_env::EnvLock,
    previous: Option<std::ffi::OsString>,
    root: std::path::PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let env = crate::utils::test_env::env_lock();
        let root = crate::utils::test_temp::unique_temp_path("kimi-delivery-data");
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let previous = std::env::var_os("BUS_DATA_DIR");
        std::env::set_var("BUS_DATA_DIR", &root);
        Self {
            _env: env,
            previous,
            root,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => std::env::set_var("BUS_DATA_DIR", previous),
            None => std::env::remove_var("BUS_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn paint(app: &App, frame: &str, draft: &str, repaint: usize) {
    let mut lines: Vec<String> = frame.lines().map(str::to_owned).collect();
    if let Some(top) = lines.iter().rposition(|line| line.trim().starts_with('╭')) {
        let bottom = lines
            .iter()
            .rposition(|line| line.trim().starts_with('╰'))
            .unwrap();
        let chars: Vec<_> = draft.chars().collect();
        let mut rows: Vec<_> = chars
            .chunks(45)
            .enumerate()
            .map(|(n, row)| {
                format!(
                    " │ {}{:<49}│",
                    if n == 0 { "> " } else { "  " },
                    row.iter().collect::<String>()
                )
            })
            .collect();
        if rows.is_empty() {
            rows.push(" │ >                                                  │".into());
        }
        lines.splice(top + 1..bottom, rows);
    }
    let frame = lines.join("\r\n");
    let frame = if repaint.is_multiple_of(2) {
        frame
    } else {
        frame.replace(['🌒', '🌑'], "🌕")
    };
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{frame}").as_bytes());
}

fn fixture(
    frame: &str,
    steer: bool,
) -> (
    App,
    Method,
    tokio::sync::mpsc::Receiver<Bytes>,
    std::path::PathBuf,
) {
    let (mut app, mut method, writes) = provider_fixture(AgentKind::Kimi, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .resize(140, 65, 0, 0);
    if steer {
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(AgentKind::Kimi), AgentState::Working);
    }
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.steer = steer;
    }
    paint(&app, frame, "", 0);
    let directory =
        crate::platform::draft_editor::directory(&app.agent_info(0, pane).unwrap().pane_id);
    assert!(!directory.exists(), "fixture must own its editor directory");
    (app, method, writes, directory)
}

async fn next_or_response(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &std::sync::mpsc::Receiver<String>,
) -> Bytes {
    let until = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(bytes) = writes.try_recv() {
            return bytes;
        }
        if let Ok(response) = response.try_recv() {
            panic!("submission ended before native input: {response}");
        }
        assert!(
            tokio::time::Instant::now() < until,
            "native editor key never arrived"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test]
async fn kimi_thinking_steer_uses_native_guidance_after_full_editor_verification() {
    delivery(THINKING, true, 0).await;
}

#[tokio::test]
async fn kimi_command_steer_uses_native_guidance_after_full_editor_verification() {
    delivery(COMMAND, true, 0).await;
}

#[tokio::test]
async fn kimi_idle_submission_still_uses_enter() {
    delivery(IDLE, false, 0).await;
}

#[tokio::test]
async fn kimi_busy_repaint_cannot_confirm_an_unconsumed_draft() {
    delivery(COMMAND, true, 1).await;
}

#[tokio::test]
async fn kimi_accepted_guidance_with_a_lost_write_ack_is_never_definitely_rejected() {
    delivery(COMMAND, true, 2).await;
}

async fn delivery(frame: &str, steer: bool, fault: u8) {
    delivery_with_idle_transition(frame, steer, fault, false).await;
}

#[tokio::test]
async fn kimi_working_to_idle_before_submit_uses_enter() {
    delivery_with_idle_transition(COMMAND, true, 0, true).await;
}

async fn delivery_with_idle_transition(
    frame: &str,
    steer: bool,
    fault: u8,
    idle_before_submit: bool,
) {
    let _scratch = Scratch::new();
    let (mut app, method, mut writes, directory) = fixture(frame, steer);
    let response = start(&mut app, method);
    let file = crate::utils::test_temp::unique_temp_path("kimi-native-editor");
    let mut draft = String::new();
    let mut release = None;
    for round in 0..2 {
        assert_eq!(
            next_or_response(&mut writes, &response).await.as_ref(),
            b"\x07",
            "no paste or submit before exact editor verification"
        );
        std::fs::write(&file, &draft).unwrap();
        crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
        draft = std::fs::read_to_string(&file).unwrap();
        let marker = draft.strip_prefix("from bus").unwrap().to_owned();
        assert!(!marker.is_empty());
        paint(&app, frame, &draft, round + 1);
        assert_eq!(
            next_or_response(&mut writes, &response).await.as_ref(),
            vec![0x7f; marker.len()]
        );
        draft.truncate("from bus".len());
        if round == 1 && fault == 2 {
            release = Some(clock(&app).fail_next_write_completion(std::io::ErrorKind::Other));
        }
        let verified_frame = if idle_before_submit && round == 1 {
            IDLE
        } else {
            frame
        };
        paint(&app, verified_frame, &draft, round + 2);
    }
    let key = next_or_response(&mut writes, &response).await;
    assert_eq!(
        key.as_ref(),
        if steer && !idle_before_submit {
            b"\x13"
        } else {
            b"\r"
        }
    );
    let submitted_frame = if idle_before_submit { IDLE } else { frame };
    if fault == 1 {
        paint(&app, submitted_frame, &draft, 5);
        clock(&app).advance(Duration::from_secs(20));
    } else {
        paint(&app, submitted_frame, "", 5);
    }
    if let Some(release) = release {
        release.send(()).unwrap();
    }
    let result = response.recv_timeout(WAIT).unwrap();
    if fault == 0 {
        assert!(result.contains("agent_prompted"), "{result}");
        assert!(!std::fs::read_dir(&directory).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("draft-")));
    } else {
        assert!(
            !result.contains("agent_prompted"),
            "spinner/write is not proof: {result}"
        );
        assert!(
            !result.contains("agent_prompt_not_shown"),
            "submit may have run: {result}"
        );
        assert!(
            !result.contains("agent_composer_busy"),
            "submit may have run: {result}"
        );
    }
    assert!(
        writes.try_recv().is_err(),
        "no retry or duplicate submission"
    );
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn kimi_approval_never_receives_editor_or_submit_keys() {
    let _scratch = Scratch::new();
    let (mut app, method, mut writes, directory) = fixture(APPROVAL, true);
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(!response.contains("agent_prompted"));
    assert!(writes.try_recv().is_err());
    assert!(!directory.exists());
}

#[tokio::test]
async fn kimi_stranded_owned_draft_is_withdrawn_before_fresh_delivery() {
    let _scratch = Scratch::new();
    let (mut app, method, mut writes, directory) = fixture(IDLE, false);
    let old = "older Bus prompt that was never submitted";
    let exchange = crate::platform::draft_editor::Exchange::begin(directory.clone(), WAIT).unwrap();
    exchange.save_recovery(old).unwrap();
    drop(exchange);
    paint(&app, IDLE, old, 0);
    let response = start(&mut app, method);
    let file = crate::utils::test_temp::unique_temp_path("kimi-stranded-editor");
    let mut draft = old.to_owned();
    // Recover the full old owned draft, then restore and verify the new one.
    for round in 0..3 {
        assert_eq!(
            next_or_response(&mut writes, &response).await.as_ref(),
            b"\x07"
        );
        std::fs::write(&file, &draft).unwrap();
        crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
        draft = std::fs::read_to_string(&file).unwrap();
        let expected = if round == 0 { "" } else { "from bus" };
        let marker = draft.strip_prefix(expected).unwrap().to_owned();
        assert!(marker.starts_with("__BUS_DRAFT_END_"));
        paint(&app, IDLE, &draft, round);
        assert_eq!(
            next_or_response(&mut writes, &response).await.as_ref(),
            vec![0x7f; marker.len()]
        );
        draft = expected.to_owned();
        paint(&app, IDLE, &draft, round + 1);
    }
    assert_eq!(
        next_or_response(&mut writes, &response).await.as_ref(),
        b"\r"
    );
    paint(&app, IDLE, "", 5);
    let result = response.recv_timeout(WAIT).unwrap();
    assert!(result.contains("agent_prompted"), "{result}");
    assert!(
        writes.try_recv().is_err(),
        "never submit or replay the stranded prompt"
    );
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn kimi_foreign_draft_without_recovery_receives_no_keys() {
    let _scratch = Scratch::new();
    let (mut app, method, mut writes, directory) = fixture(IDLE, false);
    paint(&app, IDLE, "developer's text", 0);
    let result = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(!result.contains("agent_prompted"));
    assert!(writes.try_recv().is_err());
    assert!(!directory.exists());
}
