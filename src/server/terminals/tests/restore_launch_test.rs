use super::*;

use crate::server::persistence::{restore, SessionHistorySnapshot, SessionSnapshot};

#[cfg(windows)]
fn test_restore_shell() -> &'static str {
    "C:\\Windows\\System32\\whoami.exe"
}

#[cfg(not(windows))]
fn test_restore_shell() -> &'static str {
    "/bin/sh"
}

// The two legacy persistence test identities delegate their runtime assertions
// here so the execution checks live beside the launch executor.
pub(in crate::server) fn assert_history_restore(
    snapshot: &SessionSnapshot,
    history: Option<&SessionHistorySnapshot>,
    expect_history: bool,
) {
    let (events, _events_rx) = mpsc::channel(8);
    let render_notify = Arc::new(Notify::new());
    let render_dirty = Arc::new(RenderSignal::new());
    let restored = restore(
        snapshot,
        history,
        false,
        events.clone(),
        render_notify.clone(),
        render_dirty.clone(),
    );
    let (_workspaces, _terminals, runtimes) = execute(
        restored,
        RestoreLaunchContext {
            rows: 5,
            cols: 40,
            scrollback_limit_bytes: 4096,
            shell_config: PaneShellConfig::new(
                test_restore_shell(),
                crate::utils::config::ShellModeConfig::NonLogin,
            ),
            events,
            render_notify,
            render_dirty,
        },
    );
    let runtime = runtimes
        .values()
        .next()
        .expect("restored runtime should exist");
    let restored_text = runtime.recent_unwrapped_text(10);
    if expect_history {
        assert!(
            restored_text.contains("RESTORED_HISTORY 👨‍👩‍👧 LINK"),
            "styled Unicode and hyperlink text should survive history replay"
        );
    } else {
        assert!(
            !restored_text.contains("RESTORED_HISTORY"),
            "pane history should not restore unless a history snapshot is supplied"
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while runtime.cwd().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = runtime.try_send_bytes(bytes::Bytes::from_static(b"exit\n"));
}

fn pane(cwd: &std::path::Path) -> serde_json::Value {
    serde_json::json!({ "cwd": cwd, "label": null })
}

fn snapshot() -> SessionSnapshot {
    let cwd = std::env::current_dir().unwrap();
    serde_json::from_value(serde_json::json!({
        "version": 3, "active": 0, "selected": 0,
        "workspaces": [{
            "id": "w1", "identity_cwd": cwd,
            "public_pane_numbers": { "10": 1, "20": 3, "30": 8 },
            "next_public_pane_number": 99,
            "public_tab_numbers": [5, 9], "next_public_tab_number": 11,
            "active_tab": 1, "tabs": [
                { "layout": { "Split": { "direction": "Horizontal", "ratio": 0.3,
                    "first": { "Pane": 10 }, "second": { "Pane": 20 } } },
                  "panes": { "10": pane(&cwd), "20": pane(&cwd) },
                  "zoomed": true, "focused": 10, "root_pane": 10 },
                { "layout": { "Pane": 30 }, "panes": { "30": pane(&cwd) },
                  "zoomed": false, "focused": 30, "root_pane": 30 }
            ]
        }]
    }))
    .unwrap()
}

fn plan(snapshot: &SessionSnapshot, resume_agents: bool) -> RestoredSession {
    let (events, _events_rx) = mpsc::channel(8);
    restore(
        snapshot,
        None,
        resume_agents,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    )
}

#[test]
fn shell_descriptions_execute_once_and_keep_pending_agent_deferred() {
    let mut snapshot = snapshot();
    let pane = snapshot.workspaces[0].tabs[1].panes.get_mut(&30).unwrap();
    pane.agent_session = Some(
        serde_json::from_value(serde_json::json!({
            "source": "herdr:codex", "agent": "codex", "kind": "id", "value": "codex-session"
        }))
        .unwrap(),
    );
    let restored = plan(&snapshot, true);
    let mut attempts = Vec::new();
    let (restored, runtimes) = execute_with(restored, |launch| {
        attempts.push(launch.terminal_id.clone());
        Ok(())
    });
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts.iter().collect::<HashSet<_>>().len(), 2);
    assert_eq!(runtimes.len(), 2);
    assert_eq!(restored.terminals.len(), 3);
    assert_eq!(
        restored
            .terminals
            .values()
            .filter(|terminal| terminal.pending_agent_resume_plan.is_some())
            .count(),
        1
    );
    assert!(restored.launches.is_empty());
    let (_, repeated) = execute_with(restored, |_| -> std::io::Result<()> {
        panic!("consumed launch descriptions cannot execute again")
    });
    assert!(repeated.is_empty());
}

#[test]
fn unstarted_managed_codex_restores_a_pending_launch_without_a_provider_session() {
    let mut snapshot = snapshot();
    let pane = snapshot.workspaces[0].tabs[1].panes.get_mut(&30).unwrap();
    pane.agent_name = Some("bus-r1-a2".into());
    pane.managed_agent_kind = Some("codex".into());
    pane.agent_session = None;
    let restored = plan(&snapshot, true);
    let terminal = restored
        .terminals
        .values()
        .find(|terminal| terminal.agent_name.as_deref() == Some("bus-r1-a2"))
        .expect("unstarted Codex keeps its managed identity");
    assert!(terminal.persisted_agent_session.is_none());
    assert_eq!(
        terminal.pending_agent_resume_plan.as_ref().unwrap().argv,
        vec!["codex"]
    );
    assert_eq!(
        restored.launches.len(),
        2,
        "agent is deferred until capture is verified"
    );
}

#[test]
fn failed_shell_launch_prunes_layout_focus_root_and_public_numbers() {
    let restored = plan(&snapshot(), false);
    let survivor = restored.workspaces[0].tabs[0].layout.pane_ids()[1];
    let (restored, runtimes) = execute_with(restored, |launch| {
        if launch.pane_id == survivor {
            Ok(())
        } else {
            Err(std::io::Error::other("test spawn failure"))
        }
    });
    assert_eq!(runtimes.len(), 1);
    assert_eq!(restored.terminals.len(), 1);
    let workspace = &restored.workspaces[0];
    assert_eq!(workspace.tabs.len(), 1);
    assert_eq!(workspace.active_tab, 0);
    assert_eq!(workspace.next_public_tab_number, 11);
    assert_eq!(workspace.next_public_pane_number, 99);
    assert_eq!(
        workspace.public_pane_numbers,
        HashMap::from([(survivor, 3)])
    );
    let tab = &workspace.tabs[0];
    assert_eq!(tab.number, 5);
    assert!(tab.zoomed);
    assert_eq!(tab.root_pane, survivor);
    assert_eq!(tab.layout.focused(), survivor);
    assert_eq!(tab.layout.pane_ids(), vec![survivor]);
}

#[test]
fn all_failed_shells_drop_empty_workspaces() {
    let restored = plan(&snapshot(), false);
    let (restored, runtimes) = execute_with(restored, |_| -> std::io::Result<()> {
        Err(std::io::Error::other("test spawn failure"))
    });
    assert!(restored.workspaces.is_empty());
    assert!(restored.terminals.is_empty());
    assert!(runtimes.is_empty());
}

#[test]
fn failed_legacy_tab_does_not_advance_the_public_tab_counter() {
    let mut snapshot = snapshot();
    snapshot.workspaces[0].public_tab_numbers.clear();
    snapshot.workspaces[0].next_public_tab_number = 0;
    let restored = plan(&snapshot, false);
    let last_pane = restored.workspaces[0].tabs[1].root_pane;
    let (restored, _) = execute_with(restored, |launch| {
        if launch.pane_id == last_pane {
            Err(std::io::Error::other("test spawn failure"))
        } else {
            Ok(())
        }
    });
    let workspace = &restored.workspaces[0];
    assert_eq!(workspace.tabs.len(), 1);
    assert_eq!(workspace.tabs[0].number, 1);
    assert_eq!(workspace.next_public_tab_number, 2);
}

#[test]
fn duplicate_native_session_keeps_one_pending_resume_and_suppresses_saved_history() {
    let mut snapshot = snapshot();
    let session = serde_json::json!({ "source": "herdr:codex", "agent": "codex",
        "kind": "id", "value": "same-codex-session" });
    for id in [10, 20] {
        snapshot.workspaces[0].tabs[0]
            .panes
            .get_mut(&id)
            .unwrap()
            .agent_session = Some(serde_json::from_value(session.clone()).unwrap());
    }
    let history: SessionHistorySnapshot = serde_json::from_value(serde_json::json!({
        "version": 3, "workspaces": [{ "tabs": [{ "panes": {
            "10": { "ansi": "NATIVE_HISTORY", "lines": 1 },
            "20": { "ansi": "DUPLICATE_HISTORY", "lines": 1 }
        } }] }]
    }))
    .unwrap();
    let (events, _events_rx) = mpsc::channel(8);
    let restored = restore(
        &snapshot,
        Some(&history),
        true,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    let duplicate = restored.workspaces[0].tabs[0].layout.pane_ids()[1];
    let duplicate_terminal = &restored.workspaces[0].tabs[0].panes[&duplicate].attached_terminal_id;
    assert!(restored.terminals[duplicate_terminal]
        .persisted_agent_session
        .is_none());
    assert!(restored
        .launches
        .iter()
        .all(|launch| launch.initial_history_ansi.is_none()));
    let (restored, runtimes) = execute_with(restored, |_| Ok(()));
    assert_eq!(runtimes.len(), 2);
    assert_eq!(
        restored
            .terminals
            .values()
            .filter(|terminal| terminal.pending_agent_resume_plan.is_some())
            .count(),
        1
    );
}
