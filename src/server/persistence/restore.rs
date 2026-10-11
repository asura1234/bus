use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::layout::Direction;
use tokio::sync::{mpsc, Notify};
use tracing::warn;

use crate::agents::AgentState;
use crate::server::workspaces::pane::PaneState;
use crate::server::workspaces::Workspace;
use crate::terminal::events::TerminalEvent;
use crate::utils::render::signal::RenderSignal;
use crate::{
    server::workspaces::layout::{Node, TileLayout},
    utils::ids::PaneId,
};
use crate::{terminal::TerminalState, utils::ids::TerminalId};

use super::schema::{
    PaneAgentSessionSnapshot, PaneHistorySnapshot, TabHistorySnapshot, WorkspaceHistorySnapshot,
};
use super::{
    DirectionSnapshot, LayoutSnapshot, SessionHistorySnapshot, SessionSnapshot, TabSnapshot,
    WorkspaceSnapshot,
};

struct AgentRestoreState<'a> {
    enabled: bool,
    resumed_sessions: &'a mut HashSet<String>,
}

struct PaneRestoreStartup<'a> {
    restore_plan: Option<crate::agents::resume::catalog::AgentResumePlan>,
    initial_history_ansi: Option<&'a str>,
    duplicate_agent_session: bool,
}

struct RestoreModelContext {
    resume_agents_on_restore: bool,
    events: mpsc::Sender<TerminalEvent>,
    render_notify: Arc<Notify>,
    render_dirty: Arc<RenderSignal>,
}

/// Restored model and shell descriptions. Planning never opens a PTY.
pub struct RestoredSession {
    pub(in crate::server) workspaces: Vec<Workspace>,
    pub(in crate::server) terminals: HashMap<TerminalId, TerminalState>,
    pub(in crate::server) launches: Vec<RestoreLaunch>,
    // Saved high-water marks must survive failed launches, while legacy tab
    // numbers are advanced only by tabs whose panes actually launched.
    workspace_tab_number_bases: Vec<usize>,
}

pub struct RestoreLaunch {
    pub(in crate::server) pane_id: PaneId,
    pub(in crate::server) terminal_id: TerminalId,
    pub(in crate::server) cwd: PathBuf,
    pub(in crate::server) identity: Option<RestoreLaunchIdentity>,
    pub(in crate::server) initial_history_ansi: Option<String>,
    pub(in crate::server) tab_name: Option<String>,
}

pub struct RestoreLaunchIdentity {
    pub(in crate::server) workspace_id: String,
    pub(in crate::server) tab_id: String,
    pub(in crate::server) pane_id: String,
}

type RestoredWorkspace = (Workspace, Vec<TerminalState>, Vec<RestoreLaunch>, usize);
type RestoredTab = (
    crate::server::workspaces::Tab,
    Vec<TerminalState>,
    Vec<RestoreLaunch>,
    HashMap<PaneId, u32>,
);

#[path = "restore/pruning.rs"]
mod pruning;

/// Restore workspace and terminal state without starting shell processes.
/// Native agent plans stay on TerminalState until geometry/theme are available.
pub fn restore(
    snapshot: &SessionSnapshot,
    history: Option<&SessionHistorySnapshot>,
    resume_agents_on_restore: bool,
    events: mpsc::Sender<TerminalEvent>,
    render_notify: Arc<Notify>,
    render_dirty: Arc<RenderSignal>,
) -> RestoredSession {
    let model_context = RestoreModelContext {
        resume_agents_on_restore,
        events,
        render_notify,
        render_dirty,
    };
    restore_workspaces(snapshot, history, &model_context)
}

fn migrated_public_pane_numbers_by_old_raw(
    snap: &WorkspaceSnapshot,
    next_public_pane_number: &mut usize,
) -> HashMap<u32, usize> {
    let mut public_numbers = snap.public_pane_numbers.clone();
    for tab in &snap.tabs {
        let mut pane_ids = Vec::new();
        collect_layout_snapshot_pane_ids(&tab.layout, &mut pane_ids);
        for old_raw in pane_ids {
            public_numbers.entry(old_raw).or_insert_with(|| {
                let number = *next_public_pane_number;
                *next_public_pane_number += 1;
                number
            });
        }
    }
    public_numbers
}

fn collect_layout_snapshot_pane_ids(node: &LayoutSnapshot, ids: &mut Vec<u32>) {
    match node {
        LayoutSnapshot::Pane(id) => ids.push(*id),
        LayoutSnapshot::Split { first, second, .. } => {
            collect_layout_snapshot_pane_ids(first, ids);
            collect_layout_snapshot_pane_ids(second, ids);
        }
    }
}

fn restore_workspaces(
    snapshot: &SessionSnapshot,
    history: Option<&SessionHistorySnapshot>,
    model_context: &RestoreModelContext,
) -> RestoredSession {
    let mut workspaces = Vec::new();
    let mut terminals = HashMap::new();
    let mut launches = Vec::new();
    let mut workspace_tab_number_bases = Vec::new();
    let mut resumed_agent_sessions = HashSet::new();
    for (idx, ws_snap) in snapshot.workspaces.iter().enumerate() {
        let restored = restore_workspace(
            ws_snap,
            history.and_then(|history| history.workspaces.get(idx)),
            model_context,
            &mut resumed_agent_sessions,
        );
        if let Some((workspace, restored_terminals, restored_launches, tab_number_base)) = restored
        {
            for terminal in restored_terminals {
                terminals.insert(terminal.id.clone(), terminal);
            }
            launches.extend(restored_launches);
            workspaces.push(workspace);
            workspace_tab_number_bases.push(tab_number_base);
        }
    }
    RestoredSession {
        workspaces,
        terminals,
        launches,
        workspace_tab_number_bases,
    }
}

fn restore_workspace(
    snap: &WorkspaceSnapshot,
    history: Option<&WorkspaceHistorySnapshot>,
    model_context: &RestoreModelContext,
    resumed_agent_sessions: &mut HashSet<String>,
) -> Option<RestoredWorkspace> {
    let mut tabs = Vec::new();
    let mut terminals = Vec::new();
    let mut launches = Vec::new();
    let workspace_id = snap
        .id
        .clone()
        .unwrap_or_else(crate::server::workspaces::generate_workspace_id);
    let mut next_public_pane_number = snap
        .public_pane_numbers
        .values()
        .copied()
        .max()
        .and_then(|max| max.checked_add(1))
        .unwrap_or(1)
        .max(snap.next_public_pane_number);
    let public_pane_numbers_by_old_raw =
        migrated_public_pane_numbers_by_old_raw(snap, &mut next_public_pane_number);
    let public_pane_ids_by_old_raw: HashMap<u32, String> = public_pane_numbers_by_old_raw
        .iter()
        .map(|(old_raw, public_number)| {
            (
                *old_raw,
                format!(
                    "{}:p{}",
                    workspace_id,
                    crate::server::workspaces::encode_public_number(*public_number)
                ),
            )
        })
        .collect();
    let mut public_pane_numbers = HashMap::new();
    let mut next_public_tab_number = snap
        .public_tab_numbers
        .iter()
        .copied()
        .max()
        .and_then(|max| max.checked_add(1))
        .unwrap_or(1)
        .max(snap.next_public_tab_number);

    let tab_number_base = next_public_tab_number;

    for (idx, tab_snap) in snap.tabs.iter().enumerate() {
        let tab_number = snap.public_tab_numbers.get(idx).copied().unwrap_or(idx + 1);
        let restored_tab = restore_tab(
            tab_snap,
            history.and_then(|history| history.tabs.get(idx)),
            tab_number,
            &workspace_id,
            model_context,
            resumed_agent_sessions,
            &public_pane_ids_by_old_raw,
        );
        let Some((tab, restored_terminals, restored_launches, reverse_id_map)) = restored_tab
        else {
            continue;
        };
        next_public_tab_number = next_public_tab_number.max(tab.number + 1);
        for pane_id in tab.layout.pane_ids() {
            let public_number = public_pane_numbers_by_old_raw
                .get(
                    &reverse_id_map
                        .get(&pane_id)
                        .copied()
                        .unwrap_or(pane_id.raw()),
                )
                .copied()
                .unwrap_or_else(|| {
                    let number = next_public_pane_number;
                    next_public_pane_number += 1;
                    number
                });
            public_pane_numbers.insert(pane_id, public_number);
            next_public_pane_number = next_public_pane_number.max(public_number + 1);
        }
        terminals.extend(restored_terminals);
        launches.extend(restored_launches);
        tabs.push(tab);
    }

    if tabs.is_empty() {
        return None;
    }

    let cached_auto_label = crate::server::workspaces::workspace_auto_label(&snap.identity_cwd);

    Some(Workspace {
        id: workspace_id,
        custom_name: snap.custom_name.clone(),
        identity_cwd: snap.identity_cwd.clone(),
        cached_identity_cwd: snap.identity_cwd.clone(),
        cached_auto_label,
        public_pane_numbers,
        next_public_pane_number,
        next_public_tab_number,
        active_tab: snap.active_tab.min(tabs.len().saturating_sub(1)),
        tabs,
        #[cfg(test)]
        test_runtimes: HashMap::new(),
    })
    .map(|workspace| (workspace, terminals, launches, tab_number_base))
}

fn restore_tab(
    snap: &TabSnapshot,
    history: Option<&TabHistorySnapshot>,
    number: usize,
    workspace_id: &str,
    model_context: &RestoreModelContext,
    resumed_agent_sessions: &mut HashSet<String>,
    public_pane_ids_by_old_raw: &HashMap<u32, String>,
) -> Option<RestoredTab> {
    let (node, id_map) = restore_node_remapped(&snap.layout);
    let reverse_id_map: HashMap<PaneId, u32> = id_map
        .iter()
        .map(|(&old_id, &new_id)| (new_id, old_id))
        .collect();
    let pane_ids = collect_pane_ids(&node);

    let mut panes = HashMap::new();
    let mut terminals = Vec::new();
    let mut launches = Vec::new();
    for id in &pane_ids {
        let (pane, terminal, launch) = restore_tab_pane(
            *id,
            reverse_id_map.get(id),
            snap,
            history,
            number,
            workspace_id,
            model_context,
            resumed_agent_sessions,
            public_pane_ids_by_old_raw,
        );
        panes.insert(*id, pane);
        if let Some(launch) = launch {
            launches.push(launch);
        }
        terminals.push(terminal);
    }

    if panes.is_empty() {
        warn!(
            tab = ?snap.custom_name,
            "no panes could be restored for tab, dropping it"
        );
        return None;
    }

    let surviving: HashSet<PaneId> = panes.keys().copied().collect();
    let Some(node) = prune_restored_node(node, &surviving) else {
        warn!(
            tab = ?snap.custom_name,
            "restored tab lost all panes after pruning missing layout nodes"
        );
        return None;
    };
    let pane_ids = collect_pane_ids(&node);
    let focus = resolve_restored_pane(snap.focused, &id_map, &surviving, &pane_ids)?;
    let root_pane = resolve_restored_pane(snap.root_pane, &id_map, &surviving, &pane_ids)?;
    let layout = TileLayout::from_saved(node, focus);

    Some((
        crate::server::workspaces::Tab {
            custom_name: snap.custom_name.clone(),
            number,
            root_pane,
            layout,
            panes,
            #[cfg(test)]
            runtimes: HashMap::new(),
            zoomed: snap.zoomed,
            events: model_context.events.clone(),
            render_notify: model_context.render_notify.clone(),
            render_dirty: model_context.render_dirty.clone(),
        },
        terminals,
        launches,
        reverse_id_map,
    ))
}

fn restore_tab_pane(
    id: PaneId,
    old_id: Option<&u32>,
    snap: &TabSnapshot,
    history: Option<&TabHistorySnapshot>,
    number: usize,
    workspace_id: &str,
    model_context: &RestoreModelContext,
    resumed_agent_sessions: &mut HashSet<String>,
    public_pane_ids_by_old_raw: &HashMap<u32, String>,
) -> (PaneState, TerminalState, Option<RestoreLaunch>) {
    let saved_pane = old_id.and_then(|old_id| snap.panes.get(old_id));
    let cwd = restored_pane_cwd(saved_pane);

    let saved_label = saved_pane.and_then(|p| p.label.clone());
    let saved_agent_name = saved_pane.and_then(|p| p.agent_name.clone());
    let saved_managed_agent = saved_pane
        .and_then(|pane| pane.managed_agent_kind.as_deref())
        .and_then(crate::agents::parse_canonical_agent_label);
    let saved_agent_session = saved_pane.and_then(|p| p.agent_session.as_ref());
    let saved_history =
        old_id.and_then(|old_id| history.and_then(|history| history.panes.get(old_id)));
    let startup = {
        let mut agent_restore = AgentRestoreState {
            enabled: model_context.resume_agents_on_restore,
            resumed_sessions: resumed_agent_sessions,
        };
        let mut startup =
            pane_restore_startup(saved_agent_session, saved_history, &mut agent_restore);
        if model_context.resume_agents_on_restore && saved_agent_session.is_none() {
            startup.restore_plan = crate::server::terminals::unstarted_restore_plan(
                saved_agent_name.as_deref(),
                saved_managed_agent,
                saved_pane.and_then(|pane| pane.launch_argv.as_deref()),
            )
            .filter(|plan| {
                agent_restore
                    .resumed_sessions
                    .insert(plan.dedupe_key.clone())
            });
            if startup.restore_plan.is_some() {
                startup.initial_history_ansi = None;
            }
        }
        startup
    };
    let restored_agent_session =
        restored_terminal_agent_session(saved_agent_session, startup.duplicate_agent_session);
    let old_pane_id = old_id.copied();
    let public_pane_id = old_pane_id
        .and_then(|old_id| public_pane_ids_by_old_raw.get(&old_id))
        .map(String::as_str);
    let identity = public_pane_id.map(|pane_id| RestoreLaunchIdentity {
        workspace_id: workspace_id.to_string(),
        tab_id: crate::server::workspaces::public_tab_id_for_number(workspace_id, number),
        pane_id: pane_id.to_string(),
    });
    if let Some(plan) = startup.restore_plan {
        let terminal = restored_pending_agent_terminal(
            cwd,
            plan,
            saved_label,
            restored_agent_session,
            saved_agent_name,
            saved_managed_agent,
        );
        return (PaneState::new(terminal.id.clone()), terminal, None);
    }

    let terminal_id = TerminalId::alloc();
    let mut terminal = TerminalState::new(terminal_id.clone(), cwd.clone());
    if let Some(label) = saved_label {
        terminal.set_manual_label(label);
    }
    if let Some(session) = restored_agent_session {
        terminal.set_persisted_agent_session(session);
    }
    let launch = RestoreLaunch {
        pane_id: id,
        terminal_id: terminal_id.clone(),
        cwd,
        identity,
        initial_history_ansi: startup.initial_history_ansi.map(str::to_owned),
        tab_name: snap.custom_name.clone(),
    };
    (PaneState::new(terminal_id), terminal, Some(launch))
}

fn restored_pane_cwd(saved_pane: Option<&super::schema::PaneSnapshot>) -> PathBuf {
    let saved_cwd = saved_pane
        .map(|p| p.cwd.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| "/".into()));

    if saved_cwd.exists() {
        saved_cwd
    } else {
        warn!(
            cwd = %saved_cwd.display(),
            "saved pane cwd does not exist, falling back to HOME"
        );
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/"));
        if home.exists() {
            home
        } else {
            PathBuf::from("/")
        }
    }
}

fn restored_pending_agent_terminal(
    cwd: PathBuf,
    plan: crate::agents::resume::catalog::AgentResumePlan,
    saved_label: Option<String>,
    restored_agent_session: Option<crate::agents::resume::catalog::PersistedAgentSession>,
    saved_agent_name: Option<String>,
    saved_managed_agent: Option<crate::agents::AgentKind>,
) -> TerminalState {
    let initial_restore_agent = crate::agents::parse_agent_label(&plan.agent);
    let launch_argv = plan.argv.clone();
    let terminal_id = TerminalId::alloc();
    let mut terminal =
        TerminalState::new(terminal_id.clone(), cwd).with_pending_agent_resume_plan(plan);
    terminal.launch_argv = Some(launch_argv);
    if let Some(label) = saved_label {
        terminal.set_manual_label(label);
    }
    if let Some(session) = restored_agent_session {
        terminal.set_persisted_agent_session(session);
    }
    match (saved_agent_name, saved_managed_agent) {
        (Some(agent_name), Some(agent)) => terminal.restore_managed_agent(agent_name, agent),
        (Some(_), None) => {}
        (None, _) => {}
    }
    if let Some(agent) = initial_restore_agent {
        let _ = terminal.set_detected_state_with_screen_signals_at(
            Some(agent),
            AgentState::Idle,
            false,
            false,
            None,
            std::time::Instant::now(),
        );
    }
    terminal
}

fn pane_restore_startup<'a>(
    session: Option<&PaneAgentSessionSnapshot>,
    history: Option<&'a PaneHistorySnapshot>,
    agent_restore: &mut AgentRestoreState<'_>,
) -> PaneRestoreStartup<'a> {
    // Native agent resume owns the conversation history. If a pane has a
    // resumable agent session and resume is enabled, do not replay saved pane
    // presentation history into that terminal, even when this pane is a
    // duplicate suppressed by session de-duplication.
    let restore_plan =
        session.and_then(|session| restore_plan_for_snapshot(session, agent_restore.enabled));
    let has_native_agent_restore = restore_plan.is_some();
    // Reserve before deferring launch so later panes in the same restore pass
    // cannot launch the same native agent session.
    let duplicate_agent_session = restore_plan.as_ref().is_some_and(|plan| {
        !agent_restore
            .resumed_sessions
            .insert(plan.dedupe_key.clone())
    });
    let restore_plan = if duplicate_agent_session {
        None
    } else {
        restore_plan
    };

    PaneRestoreStartup {
        restore_plan,
        initial_history_ansi: if has_native_agent_restore {
            None
        } else {
            history.map(|history| history.ansi.as_str())
        },
        duplicate_agent_session,
    }
}

fn restore_plan_for_snapshot(
    session: &PaneAgentSessionSnapshot,
    resume_agents_on_restore: bool,
) -> Option<crate::agents::resume::catalog::AgentResumePlan> {
    if !resume_agents_on_restore {
        return None;
    }
    let persisted = persisted_agent_session_from_snapshot(session)?;
    crate::agents::resume::catalog::plan(&session.source, &session.agent, &persisted.session_ref)
}

fn persisted_agent_session_from_snapshot(
    session: &PaneAgentSessionSnapshot,
) -> Option<crate::agents::resume::catalog::PersistedAgentSession> {
    crate::agents::resume::catalog::session_ref_from_snapshot(
        &session.source,
        &session.agent,
        session.kind,
        &session.value,
    )
}

fn restored_terminal_agent_session(
    session: Option<&PaneAgentSessionSnapshot>,
    duplicate_agent_session: bool,
) -> Option<crate::agents::resume::catalog::PersistedAgentSession> {
    if duplicate_agent_session {
        return None;
    }
    session.and_then(persisted_agent_session_from_snapshot)
}

pub(super) fn prune_restored_node(node: Node, surviving: &HashSet<PaneId>) -> Option<Node> {
    match node {
        Node::Pane(id) => surviving.contains(&id).then_some(Node::Pane(id)),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first = prune_restored_node(*first, surviving);
            let second = prune_restored_node(*second, surviving);
            match (first, second) {
                (Some(first), Some(second)) => Some(Node::Split {
                    direction,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            }
        }
    }
}

pub(super) fn resolve_restored_pane(
    saved_old_id: Option<u32>,
    id_map: &HashMap<u32, PaneId>,
    surviving: &HashSet<PaneId>,
    pane_ids: &[PaneId],
) -> Option<PaneId> {
    saved_old_id
        .and_then(|old_id| id_map.get(&old_id).copied())
        .filter(|pane_id| surviving.contains(pane_id))
        .or_else(|| pane_ids.first().copied())
}

/// Restore a layout tree, remapping every pane ID to a fresh globally unique one.
/// Returns the new tree and a map of old_raw_id → new PaneId.
pub(super) fn restore_node_remapped(snap: &LayoutSnapshot) -> (Node, HashMap<u32, PaneId>) {
    let mut id_map = HashMap::new();
    let node = remap_inner(snap, &mut id_map);
    (node, id_map)
}

fn remap_inner(snap: &LayoutSnapshot, id_map: &mut HashMap<u32, PaneId>) -> Node {
    match snap {
        LayoutSnapshot::Pane(old_id) => {
            let new_id = PaneId::alloc();
            id_map.insert(*old_id, new_id);
            Node::Pane(new_id)
        }
        LayoutSnapshot::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first_node = remap_inner(first, id_map);
            let second_node = remap_inner(second, id_map);
            let dir = match direction {
                DirectionSnapshot::Horizontal => Direction::Horizontal,
                DirectionSnapshot::Vertical => Direction::Vertical,
            };
            Node::Split {
                direction: dir,
                ratio: *ratio,
                first: Box::new(first_node),
                second: Box::new(second_node),
            }
        }
    }
}

pub(super) fn collect_pane_ids(node: &Node) -> Vec<PaneId> {
    let mut ids = Vec::new();
    collect_ids_inner(node, &mut ids);
    ids
}

fn collect_ids_inner(node: &Node, ids: &mut Vec<PaneId>) {
    match node {
        Node::Pane(id) => ids.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids_inner(first, ids);
            collect_ids_inner(second, ids);
        }
    }
}

#[cfg(test)]
#[path = "tests/restore_test.rs"]
mod tests;
