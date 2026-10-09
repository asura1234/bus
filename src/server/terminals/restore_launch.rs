//! Execute the shell descriptions returned by session restore.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};

use crate::server::persistence::{RestoreLaunch, RestoredSession};
use crate::server::workspaces::Workspace;
use crate::terminal::events::TerminalEvent;
use crate::terminal::runtime::{PaneLaunchEnv, PaneShellConfig};
use crate::terminal::{TerminalRuntime, TerminalState};
use crate::utils::ids::TerminalId;
use crate::utils::render::signal::RenderSignal;

pub(in crate::server) struct RestoreLaunchContext<'a> {
    pub(in crate::server) rows: u16,
    pub(in crate::server) cols: u16,
    pub(in crate::server) scrollback_limit_bytes: usize,
    pub(in crate::server) shell_config: PaneShellConfig<'a>,
    pub(in crate::server) events: mpsc::Sender<TerminalEvent>,
    pub(in crate::server) render_notify: Arc<Notify>,
    pub(in crate::server) render_dirty: Arc<RenderSignal>,
}

type ExecutedSession = (
    Vec<Workspace>,
    HashMap<TerminalId, TerminalState>,
    HashMap<TerminalId, TerminalRuntime>,
);

/// Consume a restore plan once, dropping only panes whose shells failed.
/// Native agent plans retain their existing deferred geometry/theme lifecycle.
pub(in crate::server) fn execute(
    restored: RestoredSession,
    context: RestoreLaunchContext<'_>,
) -> ExecutedSession {
    let (restored, runtimes) = execute_with(restored, |launch| spawn_shell(launch, &context));
    (restored.workspaces, restored.terminals, runtimes)
}

fn execute_with<T>(
    mut restored: RestoredSession,
    mut spawn: impl FnMut(&RestoreLaunch) -> std::io::Result<T>,
) -> (RestoredSession, HashMap<TerminalId, T>) {
    let mut runtimes = HashMap::new();
    let mut failed = HashSet::new();
    for launch in std::mem::take(&mut restored.launches) {
        match spawn(&launch) {
            Ok(runtime) => {
                runtimes.insert(launch.terminal_id, runtime);
            }
            Err(err) => {
                tracing::error!(tab = ?launch.tab_name, pane_id = launch.pane_id.raw(),
                    err = %err, "failed to restore pane, skipping");
                failed.insert(launch.terminal_id);
            }
        }
    }
    restored.discard_failed_launches(&failed);
    crate::server::workspaces::reserve_workspace_ids(&restored.workspaces);
    (restored, runtimes)
}

fn spawn_shell(
    launch: &RestoreLaunch,
    context: &RestoreLaunchContext<'_>,
) -> std::io::Result<TerminalRuntime> {
    let launch_env = launch
        .identity
        .as_ref()
        .map(|identity| {
            PaneLaunchEnv::from_extra(Vec::new()).with_identity(
                identity.workspace_id.clone(),
                identity.tab_id.clone(),
                identity.pane_id.clone(),
            )
        })
        .unwrap_or_default();
    TerminalRuntime::spawn_with_initial_history(
        launch.pane_id,
        context.rows,
        context.cols,
        launch.cwd.clone(),
        context.scrollback_limit_bytes,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        context.shell_config,
        &launch_env,
        launch.initial_history_ansi.as_deref(),
        context.events.clone(),
        context.render_notify.clone(),
        context.render_dirty.clone(),
    )
}

#[cfg(test)]
#[path = "tests/restore_launch_test.rs"]
pub(in crate::server) mod tests;
