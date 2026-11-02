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
        let chunks: Vec<String> = draft
            .split('\n')
            .flat_map(|line| {
                let chars: Vec<_> = line.chars().collect();
                if chars.is_empty() {
                    vec![String::new()]
                } else {
                    chars.chunks(45).map(|row| row.iter().collect()).collect()
                }
            })
            .collect();
        let mut rows: Vec<_> = chunks
            .iter()
            .enumerate()
            .map(|(n, row)| format!(" │ {}{:<49}│", if n == 0 { "> " } else { "  " }, row))
            .collect();
        if rows.is_empty() {
            rows.push(" │ >                                                  │".into());
        }
        if draft.chars().count() > 1200 {
            rows = rows.split_off(rows.len().saturating_sub(8));
            rows[0] = rows[0].replacen("│   ", "│ > ", 1);
            lines[top] = lines[top].replacen("────────", "─↑ more─", 1);
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
async fn kimi_thinking_steer_uses_native_guidance_after_editor_swap() {
    delivery(THINKING, true, 0).await;
}

#[tokio::test]
async fn kimi_command_steer_uses_native_guidance_after_editor_swap() {
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
    delivery_body(frame, steer, fault, idle_before_submit, "from bus").await;
}

#[tokio::test]
async fn kimi_long_clipped_editor_steer_installs_the_complete_prompt() {
    delivery_body(COMMAND, true, 0, false, &"long owned prompt ".repeat(120)).await;
}

async fn delivery_body(frame: &str, steer: bool, fault: u8, idle_before_submit: bool, body: &str) {
    delivery_with_draft(frame, steer, fault, idle_before_submit, body, "").await;
}

async fn editor_exchange(
    app: &App,
    frame: &str,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &std::sync::mpsc::Receiver<String>,
    directory: &std::path::Path,
    file: &std::path::Path,
    before: &str,
    expected: &str,
    fail_submit_ack: bool,
) -> Option<std::sync::mpsc::Sender<()>> {
    assert_eq!(next_or_response(writes, response).await.as_ref(), b"\x07");
    std::fs::write(file, before).unwrap();
    crate::platform::draft_editor::test_run_at(directory, file).unwrap();
    let returned = std::fs::read_to_string(file).unwrap();
    let marker = returned
        .strip_prefix(expected)
        .expect("exact complete native input");
    assert!(marker.starts_with("__BUS_DRAFT_END_"));
    paint(app, frame, &returned, 1);
    assert_eq!(
        next_or_response(writes, response).await.as_ref(),
        vec![0x7f; marker.len()]
    );
    let release =
        fail_submit_ack.then(|| clock(app).fail_next_write_completion(std::io::ErrorKind::Other));
    std::fs::write(file, expected).unwrap();
    paint(app, frame, expected, 2);
    release
}

async fn delivery_with_draft(
    frame: &str,
    steer: bool,
    fault: u8,
    idle_before_submit: bool,
    body: &str,
    original: &str,
) {
    let _scratch = Scratch::new();
    let (mut app, mut method, mut writes, directory) = fixture(frame, steer);
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = body.into();
    }
    paint(&app, frame, original, 0);
    let response = start(&mut app, method);
    let file = crate::utils::test_temp::unique_temp_path("kimi-native-editor");
    let submitted_frame = if idle_before_submit { IDLE } else { frame };
    let release = editor_exchange(
        &app,
        submitted_frame,
        &mut writes,
        &response,
        &directory,
        &file,
        original,
        body,
        fault == 2,
    )
    .await;
    let key = next_or_response(&mut writes, &response).await;
    assert_eq!(
        key.as_ref(),
        if steer && !idle_before_submit {
            b"\x13"
        } else {
            b"\r"
        }
    );
    if fault == 1 {
        paint(&app, submitted_frame, body, 3);
        let generation = clock(&app).advance(Duration::ZERO);
        clock(&app).wait_for_submission_check(generation);
        clock(&app).advance(
            crate::protocol::api::submission::confirmation_budget(Duration::ZERO)
                + Duration::from_millis(1),
        );
    } else {
        paint(&app, submitted_frame, "", 3);
    }
    if let Some(release) = release {
        release.send(()).unwrap();
    }
    editor_exchange(
        &app,
        submitted_frame,
        &mut writes,
        &response,
        &directory,
        &file,
        if fault == 1 { body } else { "" },
        original,
        false,
    )
    .await;
    editor_exchange(
        &app,
        submitted_frame,
        &mut writes,
        &response,
        &directory,
        &file,
        original,
        original,
        false,
    )
    .await;
    let result = response.recv_timeout(WAIT).unwrap();
    if fault == 0 {
        assert!(result.contains("agent_prompted"), "{result}");
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
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert!(!crate::platform::draft_editor::Exchange::has_preserved_draft(&directory));
    assert!(!std::fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("draft-")));
    assert!(
        writes.try_recv().is_err(),
        "three handoffs and one submit; no retry or duplicate"
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
async fn kimi_developer_draft_does_not_block_delivery_and_is_restored_byte_exact() {
    delivery_with_draft(
        IDLE,
        false,
        0,
        false,
        "from bus",
        "\t developer 中文\n  second line\ntrailing \t \n\n",
    )
    .await;
}

#[tokio::test]
async fn kimi_cancelled_exchange_active_owner_is_preserved() {
    protected_exchange(false).await;
}

#[tokio::test]
async fn kimi_cancelled_exchange_expired_unacknowledged_owner_is_preserved() {
    protected_exchange(true).await;
}

async fn protected_exchange(expired: bool) {
    let _scratch = Scratch::new();
    let (mut app, method, mut writes, directory) = fixture(IDLE, false);
    let exchange = crate::platform::draft_editor::Exchange::begin(directory.clone(), WAIT).unwrap();
    exchange.save_recovery("older Bus prompt").unwrap();
    exchange.request_verify().unwrap();
    if expired {
        let request_file = directory.join("request.json");
        let mut request: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&request_file).unwrap()).unwrap();
        request["expires_ms"] = serde_json::json!(0);
        std::fs::write(request_file, serde_json::to_vec(&request).unwrap()).unwrap();
    }
    paint(&app, IDLE, "older Bus prompt", 0);
    let result = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(result.contains("agent_composer_busy"), "{result}");
    assert!(writes.try_recv().is_err());
    assert_eq!(
        std::fs::read_to_string(exchange.recovery()).unwrap(),
        "older Bus prompt"
    );
}

#[tokio::test]
async fn delivery_latency_kimi_editor_records_payload_and_ownership_timestamps() {
    let _scratch = Scratch::new();
    let (mut app, mut method, mut writes, directory) = fixture(IDLE, false);
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = "from bus".into();
    }
    let response = start(&mut app, method);
    let file = crate::utils::test_temp::unique_temp_path("kimi-editor-latency");
    editor_exchange(
        &app,
        IDLE,
        &mut writes,
        &response,
        &directory,
        &file,
        "",
        "from bus",
        false,
    )
    .await;
    assert_eq!(
        next_or_response(&mut writes, &response).await.as_ref(),
        b"\r"
    );
    paint(&app, IDLE, "", 3);
    editor_exchange(
        &app,
        IDLE,
        &mut writes,
        &response,
        &directory,
        &file,
        "",
        "",
        false,
    )
    .await;
    editor_exchange(
        &app,
        IDLE,
        &mut writes,
        &response,
        &directory,
        &file,
        "",
        "",
        false,
    )
    .await;
    let result = response.recv_timeout(WAIT).unwrap();
    let value: serde_json::Value = serde_json::from_str(&result).unwrap();
    let no_duplicate_keys = writes.try_recv().is_err();
    std::fs::remove_file(file).unwrap();
    std::fs::remove_dir_all(directory).unwrap();

    assert!(result.contains("agent_prompted"), "{result}");
    assert!(no_duplicate_keys);
    let timestamps = &value["result"]["timestamps"];
    assert!(timestamps["native_confirmed_at_ms"].is_u64(), "{result}");
    assert!(timestamps["text_written_at_ms"].is_u64(), "{result}");
    assert!(timestamps["composer_owned_at_ms"].is_u64(), "{result}");
}
