use crate::agents as detect;
use crate::agents::Agent;
use crate::agents::AgentState;
use crate::layout::PaneId;
use crate::terminal::emulator::PaneTerminal;
use crate::terminal::events::AppEvent;
use crate::terminal::runtime::detection_policy::decide_detection_screen_read;
use crate::terminal::runtime::detection_policy::decide_screen_detection_publish;
use crate::terminal::runtime::detection_policy::detection_update_for_publish_with_osc;
use crate::terminal::runtime::detection_policy::DetectionPublishDecision;
use crate::terminal::runtime::detection_policy::DetectionScreenReadDecision;
use crate::terminal::runtime::detection_policy::DetectionScreenReadInput;
use crate::terminal::runtime::detection_policy::PendingIdleConfirmation;
use crate::terminal::runtime::detection_policy::ScreenDetectionPublishInput;
use crate::terminal::runtime::detection_policy::AGENT_PENDING_IDLE_RECHECK;
use crate::terminal::runtime::detection_policy::AGENT_STARTUP_GRACE_WINDOW;
use crate::terminal::runtime::detection_process::apply_foreground_shell_agent_action;
use crate::terminal::runtime::detection_process::clear_osc_evidence_for_agent_transition;
use crate::terminal::runtime::detection_process::foreground_group_changed;
use crate::terminal::runtime::detection_process::foreground_shell_agent_action;
use crate::terminal::runtime::detection_process::probe_foreground_process;
use crate::terminal::runtime::detection_process::process_group_for_change_tracking;
#[cfg(windows)]
use crate::terminal::runtime::detection_process::should_observe_foreground_process_group;
use crate::terminal::runtime::detection_process::should_probe_foreground_job;
use crate::terminal::runtime::detection_process::sync_content_change_acquisition;
use crate::terminal::runtime::detection_process::AgentDetectionPresence;
use crate::terminal::runtime::detection_process::ForegroundShellAgentAction;
use crate::terminal::runtime::detection_process::ProcessProbeInput;
use crate::terminal::runtime::AgentDetection;
use crate::utils::render::signal::RenderSignal;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tracing::info;
use tracing::warn;

pub(super) async fn publish_state_changed_event(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Option<Agent>,
    state: AgentState,
    visible_blocker: bool,
    process_exited: bool,
    observed_at: std::time::Instant,
) {
    // This runs on the async detector task, not the PTY reader thread.
    // Waiting for queue space here preserves correctness-critical state transitions
    // without blocking pane I/O.
    if let Err(e) = state_events
        .send(AppEvent::StateChanged {
            pane_id,
            agent,
            state,
            visible_blocker,
            process_exited,
            observed_at,
        })
        .await
    {
        warn!(
            pane = pane_id.raw(),
            err = %e,
            "failed to deliver StateChanged event"
        );
    }
}

pub(super) async fn publish_agent_process_detected_event(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Agent,
    observed_at: std::time::Instant,
) {
    if let Err(e) = state_events
        .send(AppEvent::AgentProcessDetected {
            pane_id,
            agent,
            observed_at,
        })
        .await
    {
        warn!(
            pane = pane_id.raw(),
            err = %e,
            "failed to deliver AgentProcessDetected event"
        );
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AgentDetectionPublishUpdate {
    pub(super) state: AgentState,
    pub(super) visible_idle: bool,
    pub(super) visible_blocker: bool,
    pub(super) visible_working: bool,
    pub(super) process_exited: bool,
}

pub(super) async fn apply_agent_detection_publish_update(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Option<Agent>,
    update: AgentDetectionPublishUpdate,
    observed_at: std::time::Instant,
    state: &mut AgentState,
    last_visible_idle: &mut bool,
    last_visible_blocker: &mut bool,
    last_visible_working: &mut bool,
    last_visible_signal_refresh: &mut Option<std::time::Instant>,
    foreground_shell_exit_reported: &mut bool,
) {
    *state = update.state;
    *last_visible_idle = update.visible_idle;
    *last_visible_blocker = update.visible_blocker;
    *last_visible_working = update.visible_working;
    *last_visible_signal_refresh = if update.visible_blocker || update.visible_working {
        Some(observed_at)
    } else {
        None
    };
    if update.process_exited {
        *foreground_shell_exit_reported = true;
    }
    publish_state_changed_event(
        state_events,
        pane_id,
        agent,
        update.state,
        update.visible_blocker,
        update.process_exited,
        observed_at,
    )
    .await;
}

pub(super) fn spawn_detection_task(
    pane_id: PaneId,
    agent_detection: AgentDetection,
    initial_agent: Option<Agent>,
    child_pid: Arc<AtomicU32>,
    terminal: Arc<PaneTerminal>,
    events: mpsc::Sender<AppEvent>,
    detection_content_seq: Arc<AtomicU64>,
    render_notify: Arc<Notify>,
    render_dirty: Arc<RenderSignal>,
) -> Option<tokio::task::AbortHandle> {
    if agent_detection != AgentDetection::Enabled {
        return None;
    }
    let task = DetectionTask {
        pane_id,
        child_pid,
        terminal,
        state_events: events,
        detection_content_seq,
        render_notify,
        render_dirty,
    };
    let handle = tokio::spawn(task.run(initial_agent));
    Some(handle.abort_handle())
}

const TICK_UNIDENTIFIED: Duration = Duration::from_millis(500);
const TICK_IDENTIFIED: Duration = Duration::from_millis(300);
const TICK_TRANSIENT_COLOR_OVERRIDE: Duration = Duration::from_millis(50);

struct DetectionTask {
    pane_id: PaneId,
    child_pid: Arc<AtomicU32>,
    terminal: Arc<PaneTerminal>,
    state_events: mpsc::Sender<AppEvent>,
    detection_content_seq: Arc<AtomicU64>,
    render_notify: Arc<Notify>,
    render_dirty: Arc<RenderSignal>,
}

struct DetectionState {
    agent_presence: AgentDetectionPresence,
    state: AgentState,
    last_visible_idle: bool,
    last_process_check: Instant,
    #[cfg(windows)]
    last_observation: (Instant, Option<u64>),
    last_foreground_pgid: Option<u32>,
    has_process_probe: bool,
    acquisition_started_at: Option<Instant>,
    last_content_change_at: Option<Instant>,
    pending_foreground_shell_clear: bool,
    foreground_shell_exit_reported: bool,
    pending_restore_probe: bool,
    last_visible_blocker: bool,
    last_visible_working: bool,
    last_visible_signal_refresh: Option<Instant>,
    last_detection_text: String,
    last_screen_scan_detection_content_seq: Option<u64>,
    agent_startup_grace_until: Option<Instant>,
    pending_idle: PendingIdleConfirmation,
}

impl DetectionState {
    fn new(initial_agent: Option<Agent>) -> Self {
        Self {
            agent_presence: AgentDetectionPresence::from_agent(initial_agent),
            state: AgentState::Idle,
            last_visible_idle: initial_agent.is_some(),
            last_process_check: Instant::now(),
            #[cfg(windows)]
            last_observation: (Instant::now(), Some(0)),
            last_foreground_pgid: None,
            has_process_probe: false,
            acquisition_started_at: None,
            last_content_change_at: None,
            pending_foreground_shell_clear: false,
            foreground_shell_exit_reported: false,
            pending_restore_probe: initial_agent.is_some(),
            last_visible_blocker: false,
            last_visible_working: false,
            last_visible_signal_refresh: None,
            last_detection_text: String::new(),
            last_screen_scan_detection_content_seq: None,
            agent_startup_grace_until: None,
            pending_idle: PendingIdleConfirmation::default(),
        }
    }
}

impl DetectionTask {
    async fn run(self, initial_agent: Option<Agent>) {
        let mut status = DetectionState::new(initial_agent);
        tokio::time::sleep(Duration::from_millis(50)).await;
        loop {
            tokio::time::sleep(self.tick_delay(&status)).await;
            let now = Instant::now();
            let pid = self.child_pid.load(Ordering::Acquire);
            let mut agent = status.agent_presence.current_agent();
            let (foreground_pgid, process_group_changed, should_check_process) =
                self.observe_foreground_group(&mut status, pid, now);
            let mut agent_changed = false;
            if should_check_process {
                agent_changed = self
                    .check_process(
                        &mut status,
                        pid,
                        foreground_pgid,
                        process_group_changed,
                        now,
                    )
                    .await;
                if agent_changed {
                    agent = status.agent_presence.current_agent();
                }
            }
            let pid = self.child_pid.load(Ordering::Acquire);
            // Keep the terminal restore side effect separate from render notification state.
            #[allow(clippy::collapsible_if)]
            if pid > 0
                && self
                    .terminal
                    .maybe_restore_host_terminal_theme(self.pane_id, pid, || {
                        super::detection_process::foreground_job(pid)
                    })
            {
                if self.render_dirty.request_pty(self.pane_id) {
                    self.render_notify.notify_one();
                }
            }
            let process_exited = status.pending_foreground_shell_clear
                && agent.is_some()
                && !status.foreground_shell_exit_reported;
            if self.startup_grace_blocks_read(&mut status, now, process_exited) {
                continue;
            }
            self.scan_screen(
                &mut status,
                agent,
                agent_changed,
                process_group_changed,
                process_exited,
                now,
            )
            .await;
        }
    }

    fn tick_delay(&self, status: &DetectionState) -> Duration {
        if self.terminal.has_transient_default_color_override() {
            TICK_TRANSIENT_COLOR_OVERRIDE
        } else if status.pending_idle.active() {
            AGENT_PENDING_IDLE_RECHECK
        } else if status.agent_presence.current_agent().is_none() {
            TICK_UNIDENTIFIED
        } else {
            TICK_IDENTIFIED
        }
    }

    fn observe_foreground_group(
        &self,
        status: &mut DetectionState,
        pid: u32,
        now: Instant,
    ) -> (Option<u32>, bool, bool) {
        let agent = status.agent_presence.current_agent();
        let process_probe_input = ProcessProbeInput {
            current_agent: agent,
            foreground_pgid: status.last_foreground_pgid,
            last_foreground_pgid: status.last_foreground_pgid,
            has_process_probe: status.has_process_probe,
            acquisition_age: status
                .acquisition_started_at
                .map(|started| now.duration_since(started)),
            pending_foreground_shell_clear: status.pending_foreground_shell_clear,
            pending_restore_probe: status.pending_restore_probe,
            elapsed_since_process_check: now.duration_since(status.last_process_check),
        };
        #[cfg(windows)]
        let content_seq = self.detection_content_seq.load(Ordering::Relaxed);
        #[cfg(windows)]
        let last_content_seq = status.last_observation.1;
        #[cfg(windows)]
        let foreground_observation_due = should_observe_foreground_process_group(
            last_content_seq != Some(content_seq)
                && (last_content_seq.is_some()
                    || now.duration_since(status.last_observation.0) >= TICK_IDENTIFIED),
            process_probe_input,
        );
        #[cfg(not(windows))]
        let foreground_observation_due = true;
        let foreground_pgid = match (pid, foreground_observation_due) {
            (0, _) => None,
            (_, true) => detect::foreground_process_group_id(pid),
            _ => status.last_foreground_pgid,
        };
        #[cfg(windows)]
        if pid > 0 && foreground_observation_due {
            let retry = last_content_seq.is_some() && last_content_seq != Some(content_seq);
            status.last_observation = (now, (!retry).then_some(content_seq));
        }
        let process_group_changed =
            foreground_group_changed(foreground_pgid, status.last_foreground_pgid);
        let should_check_process = pid > 0 && {
            let process_probe_input = ProcessProbeInput {
                foreground_pgid,
                ..process_probe_input
            };
            should_probe_foreground_job(process_probe_input)
        };

        (foreground_pgid, process_group_changed, should_check_process)
    }

    async fn check_process(
        &self,
        status: &mut DetectionState,
        pid: u32,
        foreground_pgid: Option<u32>,
        process_group_changed: bool,
        now: Instant,
    ) -> bool {
        status.last_process_check = now;
        let had_process_probe = status.has_process_probe;
        status.has_process_probe = true;
        let probe = probe_foreground_process(pid, foreground_pgid);
        let process_name = probe.process_name;
        let process_group_id = probe.process_group_id;
        let tracked_process_group_id =
            process_group_for_change_tracking(foreground_pgid, process_group_id);
        let foreground_is_pane_shell = probe.foreground_is_pane_shell;
        let new_agent = probe.agent;

        let previous_agent = status.agent_presence.current_agent();
        let foreground_action = foreground_shell_agent_action(
            previous_agent,
            new_agent,
            foreground_is_pane_shell,
            status.foreground_shell_exit_reported,
        );
        let changed = apply_foreground_shell_agent_action(
            &mut status.agent_presence,
            foreground_action,
            previous_agent,
            new_agent,
            &mut status.pending_foreground_shell_clear,
            &mut status.foreground_shell_exit_reported,
        );
        status.last_foreground_pgid = tracked_process_group_id;
        if new_agent.is_some() {
            status.acquisition_started_at = None;
            status.last_content_change_at = None;
        } else if status.agent_presence.current_agent().is_none()
            && had_process_probe
            && process_group_changed
        {
            status.acquisition_started_at = Some(now);
        }
        status.pending_restore_probe = false;

        if changed {
            self.accept_agent_transition(
                status,
                previous_agent,
                foreground_action,
                process_name,
                process_group_id,
                now,
            )
            .await;
        }
        changed
    }

    async fn accept_agent_transition(
        &self,
        status: &mut DetectionState,
        previous_agent: Option<Agent>,
        foreground_action: ForegroundShellAgentAction,
        process_name: Option<String>,
        process_group_id: Option<u32>,
        now: Instant,
    ) {
        let agent = status.agent_presence.current_agent();
        if agent != previous_agent
            || foreground_action == ForegroundShellAgentAction::ReportReplacementProcess
        {
            status.pending_idle.clear();
            status.last_screen_scan_detection_content_seq = None;
            // A replacement agent must not inherit OSC
            // evidence from the previous process; a first
            // acquisition keeps the evidence its own
            // process already emitted.
            clear_osc_evidence_for_agent_transition(&self.terminal, previous_agent);
            if let Some(agent) = agent {
                status.agent_startup_grace_until = Some(now + AGENT_STARTUP_GRACE_WINDOW);
                status.state = AgentState::Unknown;
                status.last_visible_idle = false;
                status.last_visible_blocker = false;
                status.last_visible_working = false;
                status.last_visible_signal_refresh = None;
                publish_agent_process_detected_event(
                    self.state_events.clone(),
                    self.pane_id,
                    agent,
                    now,
                )
                .await;
            } else {
                status.agent_startup_grace_until = None;
            }
        }
        if let Some(process_name) = process_name {
            info!(
                pane = self.pane_id.raw(),
                previous_agent = ?previous_agent,
                ?agent,
                process = %process_name,
                pgid = ?process_group_id,
                "agent changed"
            );
        } else {
            info!(
                pane = self.pane_id.raw(),
                previous_agent = ?previous_agent,
                ?agent,
                pgid = ?process_group_id,
                "agent changed"
            );
        }
    }

    fn startup_grace_blocks_read(
        &self,
        status: &mut DetectionState,
        now: Instant,
        process_exited: bool,
    ) -> bool {
        if let Some(until) = status.agent_startup_grace_until {
            if process_exited {
                status.agent_startup_grace_until = None;
                status.last_screen_scan_detection_content_seq = None;
                status.pending_idle.clear();
            } else {
                if now < until {
                    status.pending_idle.clear();
                    return true;
                }
                status.agent_startup_grace_until = None;
                status.pending_idle.clear();
                return true;
            }
        }

        false
    }

    async fn scan_screen(
        &self,
        status: &mut DetectionState,
        agent: Option<Agent>,
        agent_changed: bool,
        process_group_changed: bool,
        process_exited: bool,
        now: Instant,
    ) {
        let current_detection_content_seq = if agent.is_some() {
            Some(self.detection_content_seq.load(Ordering::Relaxed))
        } else {
            None
        };
        match decide_detection_screen_read(DetectionScreenReadInput {
            state: status.state,
            agent,
            pending_idle_active: status.pending_idle.active(),
            agent_changed,
            process_exited,
            current_detection_content_seq,
            last_screen_scan_detection_content_seq: status.last_screen_scan_detection_content_seq,
        }) {
            DetectionScreenReadDecision::Read => {}
            DetectionScreenReadDecision::Skip => return,
        }

        let content = self.terminal.detection_text();
        status.last_screen_scan_detection_content_seq = current_detection_content_seq;
        let content_changed = content != status.last_detection_text;
        status.last_detection_text.clone_from(&content);
        if detect::should_skip_state_update(agent, &content) {
            status.pending_idle.clear();
            return;
        }
        sync_content_change_acquisition(
            status.agent_presence.current_agent(),
            process_group_changed,
            content_changed,
            now,
            &mut status.acquisition_started_at,
            &mut status.last_content_change_at,
        );

        let osc_title = self.terminal.agent_osc_title();
        let osc_progress = self.terminal.agent_osc_progress();
        let Some(screen_detection) = detection_update_for_publish_with_osc(
            agent,
            &content,
            &osc_title,
            &osc_progress,
            process_exited,
        ) else {
            status.pending_idle.clear();
            return;
        };
        let decision = decide_screen_detection_publish(
            ScreenDetectionPublishInput {
                screen_detection,
                current_state: status.state,
                last_visible_idle: status.last_visible_idle,
                last_visible_blocker: status.last_visible_blocker,
                last_visible_working: status.last_visible_working,
                last_visible_signal_refresh: status.last_visible_signal_refresh,
                process_exited,
                agent_changed,
                now,
            },
            &mut status.pending_idle,
        );
        self.publish_screen_detection(status, agent, decision, now)
            .await;
    }

    async fn publish_screen_detection(
        &self,
        status: &mut DetectionState,
        agent: Option<Agent>,
        decision: DetectionPublishDecision,
        now: Instant,
    ) {
        match decision {
            DetectionPublishDecision::NoPublish => {}
            DetectionPublishDecision::Publish {
                state: new_state,
                visible_idle,
                visible_blocker,
                visible_working,
                process_exited: publish_process_exited,
            } => {
                apply_agent_detection_publish_update(
                    self.state_events.clone(),
                    self.pane_id,
                    agent,
                    AgentDetectionPublishUpdate {
                        state: new_state,
                        visible_idle,
                        visible_blocker,
                        visible_working,
                        process_exited: publish_process_exited,
                    },
                    now,
                    &mut status.state,
                    &mut status.last_visible_idle,
                    &mut status.last_visible_blocker,
                    &mut status.last_visible_working,
                    &mut status.last_visible_signal_refresh,
                    &mut status.foreground_shell_exit_reported,
                )
                .await;
            }
        }
    }
}
