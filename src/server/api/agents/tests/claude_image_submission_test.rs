use super::*;

struct Scratch {
    _env: crate::utils::test_env::EnvLock,
    root: std::path::PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl Scratch {
    fn new() -> Self {
        let env = crate::utils::test_env::env_lock();
        let root = crate::utils::test_temp::unique_temp_path("claude-image-delivery");
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut previous = Vec::new();
        for (key, value) in [
            ("BUS_DATA_DIR", root.clone()),
            ("CLAUDE_CONFIG_DIR", root.join("claude")),
        ] {
            previous.push((key, std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        Self {
            _env: env,
            root,
            previous,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn step(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &std::sync::mpsc::Receiver<String>,
    phase: &str,
) -> Bytes {
    tokio::time::timeout(WAIT, writes.recv())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{phase}: {error}; native response: {:?}",
                response.try_recv()
            )
        })
        .unwrap()
}

async fn focused_delivery(draft: &str, image: bool) {
    let _scratch = Scratch::new();
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, draft);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(pane));
    let text = if image {
        "from bus\n\"/tmp/large.png\""
    } else {
        "from bus"
    };
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = text.into();
    }
    let response = start(&mut app, method);
    if !draft.is_empty() {
        assert_eq!(
            step(&mut writes, &response, "stash").await.as_ref(),
            b"\x13"
        );
        redraw(&app, AgentKind::Claude, "");
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes("\r\n› stashed".as_bytes());
    }
    assert_eq!(
        step(&mut writes, &response, "paste").await,
        Bytes::from(format!("\x1b[200~{text}\x1b[201~"))
    );
    if image {
        // Claude 2.1.316 loads image paths asynchronously. Enter during this
        // interval is consumed by its paste handler without submitting.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), writes.recv())
                .await
                .is_err(),
            "Enter must follow image registration, not the PTY write receipt"
        );
        assert!(response.try_recv().is_err());
        redraw(&app, AgentKind::Claude, "[Image #7]from bus");
    } else {
        redraw(&app, AgentKind::Claude, text);
    }
    assert_eq!(step(&mut writes, &response, "submit").await.as_ref(), b"\r");
    // New developer input remains buffered until the native stash restores.
    if !draft.is_empty() {
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .try_send_developer_bytes(Bytes::from_static(b" more"))
            .unwrap();
    }
    redraw(&app, AgentKind::Claude, draft);
    let reply = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap())
        .await
        .unwrap();
    assert!(reply.contains("agent_prompted"), "{reply}");
    if !draft.is_empty() {
        assert_eq!(next_write(&mut writes).await.as_ref(), b" more");
        assert!(app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_text()
            .contains(draft));
    }
    assert!(writes.try_recv().is_err(), "one paste and one submit only");
}

#[tokio::test]
async fn focused_idle_claude_waits_only_for_image_registration_before_submit() {
    focused_delivery("", true).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_image_and_restores_half_typed_draft() {
    focused_delivery("half-typed developer draft", true).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_text_after_the_owned_paste_frame() {
    focused_delivery("", false).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_text_and_restores_half_typed_draft() {
    focused_delivery("half-typed developer draft", false).await;
}
