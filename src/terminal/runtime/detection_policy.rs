use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::agents::{AgentDetection, AgentKind, AgentState};

const TICK_UNIDENTIFIED: Duration = Duration::from_millis(500);
pub(super) const TICK_IDENTIFIED: Duration = Duration::from_millis(300);
const TICK_TRANSIENT_COLOR_OVERRIDE: Duration = Duration::from_millis(50);

pub(super) fn detection_tick_delay(
    has_transient_color_override: bool,
    pending_idle_active: bool,
    has_agent: bool,
) -> Duration {
    if has_transient_color_override {
        TICK_TRANSIENT_COLOR_OVERRIDE
    } else if pending_idle_active {
        AGENT_PENDING_IDLE_RECHECK
    } else if !has_agent {
        TICK_UNIDENTIFIED
    } else {
        TICK_IDENTIFIED
    }
}

pub(super) const AGENT_PENDING_IDLE_RECHECK: std::time::Duration =
    std::time::Duration::from_millis(100);
const AGENT_PENDING_IDLE_CONFIRMATIONS: u8 = 3;
pub(super) const AGENT_PENDING_IDLE_CAP: std::time::Duration =
    std::time::Duration::from_millis(700);
pub(super) const STABLE_VISIBLE_SIGNAL_REFRESH: std::time::Duration =
    std::time::Duration::from_millis(800);
pub(super) const AGENT_STARTUP_GRACE_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(3);

/// Current foreground controls override lifecycle signals; pending shortcuts
/// coexist with positive activity. A title-only Blocked signal does not hide a
/// live activity line when no input control is present. Exited processes keep
/// their exit decision.
pub(super) fn apply_driver_controls(
    content: &str,
    mut detection: AgentDetection,
    process_exited: bool,
) -> AgentDetection {
    if process_exited {
        return detection;
    }
    if crate::terminal::driver::has_blocking_input_controls(content, detection.visible_working) {
        detection.state = AgentState::Blocked;
        detection.visible_blocker = true;
        detection.visible_idle = false;
        detection.visible_working = false;
        return detection;
    }
    if detection.state == AgentState::Blocked
        && !crate::terminal::driver::has_blocking_input_controls(content, false)
        && crate::terminal::driver::has_live_activity(content)
    {
        detection.state = AgentState::Working;
        detection.visible_working = true;
        detection.visible_blocker = false;
        detection.visible_idle = false;
    }
    detection
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DetectionPublishState {
    pub(super) state: AgentState,
    pub(super) visible_idle: bool,
    pub(super) visible_blocker: bool,
    pub(super) visible_working: bool,
}

#[derive(Debug, Default)]
pub(super) struct PendingIdleConfirmation {
    started_at: Option<std::time::Instant>,
    confirmations: u8,
}

impl PendingIdleConfirmation {
    pub(super) fn active(&self) -> bool {
        self.started_at.is_some()
    }

    pub(super) fn clear(&mut self) {
        self.started_at = None;
        self.confirmations = 0;
    }

    pub(super) fn should_hold_working_to_idle(
        &mut self,
        previous: DetectionPublishState,
        next: DetectionPublishState,
        agent_changed: bool,
        process_exited: bool,
        now: std::time::Instant,
    ) -> bool {
        let is_working_to_plain_idle = previous.state == AgentState::Working
            && matches!(next.state, AgentState::Idle | AgentState::Blocked)
            && !next.visible_idle
            && !next.visible_blocker
            && !agent_changed
            && !process_exited;

        if !is_working_to_plain_idle {
            self.clear();
            return false;
        }

        let Some(started_at) = self.started_at else {
            self.started_at = Some(now);
            self.confirmations = 0;
            return true;
        };

        if now.duration_since(started_at) >= AGENT_PENDING_IDLE_CAP {
            self.clear();
            return false;
        }

        self.confirmations = self.confirmations.saturating_add(1);
        if self.confirmations >= AGENT_PENDING_IDLE_CONFIRMATIONS {
            self.clear();
            return false;
        }

        true
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct IdleScreenScanSkipInput {
    pub(super) state: AgentState,
    pub(super) agent: Option<AgentKind>,
    pub(super) pending_idle_active: bool,
    pub(super) agent_changed: bool,
    pub(super) process_exited: bool,
    pub(super) current_detection_content_seq: Option<u64>,
    pub(super) last_screen_scan_detection_content_seq: Option<u64>,
}

pub(super) fn should_skip_idle_screen_scan(input: IdleScreenScanSkipInput) -> bool {
    if input.state != AgentState::Idle
        || input.agent.is_none()
        || input.pending_idle_active
        || input.agent_changed
        || input.process_exited
    {
        return false;
    }

    input.current_detection_content_seq.is_some()
        && input.last_screen_scan_detection_content_seq == input.current_detection_content_seq
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetectionScreenReadDecision {
    Read,
    Skip,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DetectionScreenReadInput {
    pub(super) state: AgentState,
    pub(super) agent: Option<AgentKind>,
    pub(super) pending_idle_active: bool,
    pub(super) agent_changed: bool,
    pub(super) process_exited: bool,
    pub(super) current_detection_content_seq: Option<u64>,
    pub(super) last_screen_scan_detection_content_seq: Option<u64>,
}

pub(super) fn decide_detection_screen_read(
    input: DetectionScreenReadInput,
) -> DetectionScreenReadDecision {
    if should_skip_idle_screen_scan(IdleScreenScanSkipInput {
        state: input.state,
        agent: input.agent,
        pending_idle_active: input.pending_idle_active,
        agent_changed: input.agent_changed,
        process_exited: input.process_exited,
        current_detection_content_seq: input.current_detection_content_seq,
        last_screen_scan_detection_content_seq: input.last_screen_scan_detection_content_seq,
    }) {
        DetectionScreenReadDecision::Skip
    } else {
        DetectionScreenReadDecision::Read
    }
}

pub(super) fn should_publish_detection_update(
    previous: DetectionPublishState,
    next: DetectionPublishState,
    agent_changed: bool,
    process_exited: bool,
    stable_visible_signal_refresh_due: bool,
) -> bool {
    next.state != previous.state
        || next.visible_idle != previous.visible_idle
        || next.visible_blocker != previous.visible_blocker
        || next.visible_working != previous.visible_working
        || agent_changed
        || process_exited
        || (stable_visible_signal_refresh_due && next.visible_blocker && previous.visible_blocker)
}

pub(super) fn stable_visible_signal_refresh_due(
    previous: DetectionPublishState,
    next: DetectionPublishState,
    last_refresh: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    let stable_visible_signal = next.visible_blocker && previous.visible_blocker;

    stable_visible_signal
        && last_refresh.is_none_or(|last_refresh| {
            now.duration_since(last_refresh) >= STABLE_VISIBLE_SIGNAL_REFRESH
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetectionTransitionDecision {
    NoPublish,
    PublishNext,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DetectionTransitionInput {
    pub(super) previous_publish: DetectionPublishState,
    pub(super) next_publish: DetectionPublishState,
    pub(super) agent_changed: bool,
    pub(super) process_exited: bool,
    pub(super) stable_refresh_due: bool,
    pub(super) now: std::time::Instant,
}

pub(super) fn decide_detection_transition(
    input: DetectionTransitionInput,
    pending_idle: &mut PendingIdleConfirmation,
) -> DetectionTransitionDecision {
    if pending_idle.should_hold_working_to_idle(
        input.previous_publish,
        input.next_publish,
        input.agent_changed,
        input.process_exited,
        input.now,
    ) {
        return DetectionTransitionDecision::NoPublish;
    }

    if should_publish_detection_update(
        input.previous_publish,
        input.next_publish,
        input.agent_changed,
        input.process_exited,
        input.stable_refresh_due,
    ) {
        return DetectionTransitionDecision::PublishNext;
    }

    DetectionTransitionDecision::NoPublish
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetectionPublishDecision {
    NoPublish,
    Publish {
        state: AgentState,
        visible_idle: bool,
        visible_blocker: bool,
        visible_working: bool,
        process_exited: bool,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ScreenDetectionPublishInput {
    pub(super) current_state: AgentState,
    pub(super) last_visible_idle: bool,
    pub(super) last_visible_blocker: bool,
    pub(super) last_visible_working: bool,
    pub(super) last_visible_signal_refresh: Option<std::time::Instant>,
    pub(super) screen_detection: AgentDetection,
    pub(super) process_exited: bool,
    pub(super) agent_changed: bool,
    pub(super) now: std::time::Instant,
}

pub(super) fn decide_screen_detection_publish(
    input: ScreenDetectionPublishInput,
    pending_idle: &mut PendingIdleConfirmation,
) -> DetectionPublishDecision {
    let detection = input.screen_detection;
    let new_state = crate::terminal::state::stabilize_agent_detection(detection);
    let visible_idle = detection.visible_idle && new_state == AgentState::Idle;
    let visible_blocker = detection.visible_blocker && new_state == AgentState::Blocked;
    let visible_working = detection.visible_working && new_state == AgentState::Working;

    let previous_publish = DetectionPublishState {
        state: input.current_state,
        visible_idle: input.last_visible_idle,
        visible_blocker: input.last_visible_blocker,
        visible_working: input.last_visible_working,
    };
    let next_publish = DetectionPublishState {
        state: new_state,
        visible_idle,
        visible_blocker,
        visible_working,
    };
    let stable_refresh_due = stable_visible_signal_refresh_due(
        previous_publish,
        next_publish,
        input.last_visible_signal_refresh,
        input.now,
    );

    match decide_detection_transition(
        DetectionTransitionInput {
            previous_publish,
            next_publish,
            agent_changed: input.agent_changed,
            process_exited: input.process_exited,
            stable_refresh_due,
            now: input.now,
        },
        pending_idle,
    ) {
        DetectionTransitionDecision::NoPublish => DetectionPublishDecision::NoPublish,
        DetectionTransitionDecision::PublishNext => DetectionPublishDecision::Publish {
            state: new_state,
            visible_idle,
            visible_blocker,
            visible_working,
            process_exited: input.process_exited,
        },
    }
}

pub(super) fn detection_update_for_publish_with_osc(
    agent: Option<AgentKind>,
    content: &str,
    osc_title: &str,
    osc_progress: &str,
    process_exited: bool,
    managed_launch_pending: bool,
) -> Option<crate::agents::AgentDetection> {
    if process_exited {
        return Some(crate::agents::AgentDetection {
            state: AgentState::Idle,
            skip_state_update: false,
            visible_idle: true,
            visible_blocker: false,
            visible_working: false,
        });
    }

    let detection = if managed_launch_pending {
        crate::agents::detect_agent_with_launch_context(
            agent,
            content,
            osc_title,
            osc_progress,
            true,
        )
    } else {
        crate::agents::detect_agent_with_osc(agent, content, osc_title, osc_progress)
    };
    (!detection.skip_state_update).then_some(detection)
}

/// A live obligation needs either a ready composer or positive work evidence.
/// Working rules stay authoritative; even a matched blocker waits for activity
/// to go quiet, without adding another dialog parser.
pub(super) fn apply_obligation_detection(
    mut detection: AgentDetection,
    obligation: bool,
    composer_ready: bool,
    activity_working: bool,
) -> AgentDetection {
    if !obligation
        || detection.skip_state_update
        || detection.state == AgentState::Working
        || (detection.state == AgentState::Blocked && !activity_working)
    {
        return detection;
    }
    detection.state = if composer_ready {
        AgentState::Idle
    } else if activity_working {
        AgentState::Working
    } else {
        AgentState::Blocked
    };
    detection.visible_idle = composer_ready;
    detection.visible_blocker = false;
    detection.visible_working = false;
    detection
}

pub(super) fn observe_detection_content_change(bytes: &[u8], detection_content_seq: &AtomicU64) {
    if !bytes.is_empty() {
        detection_content_seq.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn mark_detection_content_changed(detection_content_seq: &AtomicU64) {
    detection_content_seq.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish_state(state: AgentState) -> DetectionPublishState {
        DetectionPublishState {
            state,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
        }
    }

    fn screen_detection(state: AgentState) -> AgentDetection {
        AgentDetection {
            state,
            skip_state_update: false,
            visible_idle: state == AgentState::Idle,
            visible_blocker: false,
            visible_working: state == AgentState::Working,
        }
    }

    fn screen_publish_input(
        current_state: AgentState,
        screen_detection: AgentDetection,
        now: std::time::Instant,
    ) -> ScreenDetectionPublishInput {
        ScreenDetectionPublishInput {
            current_state,
            last_visible_idle: false,
            last_visible_blocker: false,
            last_visible_working: false,
            last_visible_signal_refresh: None,
            screen_detection,
            process_exited: false,
            agent_changed: false,
            now,
        }
    }

    fn screen_read_input(state: AgentState, current_seq: u64) -> DetectionScreenReadInput {
        DetectionScreenReadInput {
            state,
            agent: Some(AgentKind::Codex),
            pending_idle_active: false,
            agent_changed: false,
            process_exited: false,
            current_detection_content_seq: Some(current_seq),
            last_screen_scan_detection_content_seq: Some(10),
        }
    }

    #[test]
    fn screen_read_skips_unchanged_idle_bottom_buffer() {
        assert_eq!(
            decide_detection_screen_read(screen_read_input(AgentState::Idle, 10)),
            DetectionScreenReadDecision::Skip
        );
    }

    #[test]
    fn screen_read_reads_when_idle_bottom_buffer_changes() {
        assert_eq!(
            decide_detection_screen_read(screen_read_input(AgentState::Idle, 11)),
            DetectionScreenReadDecision::Read
        );
    }

    #[test]
    fn screen_read_reads_for_transitions_and_missing_agent() {
        let mut input = screen_read_input(AgentState::Idle, 10);
        input.pending_idle_active = true;
        assert_eq!(
            decide_detection_screen_read(input),
            DetectionScreenReadDecision::Read
        );

        let mut input = screen_read_input(AgentState::Idle, 10);
        input.agent_changed = true;
        assert_eq!(
            decide_detection_screen_read(input),
            DetectionScreenReadDecision::Read
        );

        let mut input = screen_read_input(AgentState::Idle, 10);
        input.process_exited = true;
        assert_eq!(
            decide_detection_screen_read(input),
            DetectionScreenReadDecision::Read
        );

        let mut input = screen_read_input(AgentState::Idle, 10);
        input.agent = None;
        assert_eq!(
            decide_detection_screen_read(input),
            DetectionScreenReadDecision::Read
        );
    }

    #[test]
    fn pending_idle_holds_working_to_plain_idle_until_confirmed() {
        let now = std::time::Instant::now();
        let previous = publish_state(AgentState::Working);
        let next = publish_state(AgentState::Idle);
        let mut pending = PendingIdleConfirmation::default();

        assert!(pending.should_hold_working_to_idle(previous, next, false, false, now));
        assert!(pending.should_hold_working_to_idle(
            previous,
            next,
            false,
            false,
            now + AGENT_PENDING_IDLE_RECHECK
        ));
        assert!(pending.should_hold_working_to_idle(
            previous,
            next,
            false,
            false,
            now + AGENT_PENDING_IDLE_RECHECK * 2
        ));
        assert!(!pending.should_hold_working_to_idle(
            previous,
            next,
            false,
            false,
            now + AGENT_PENDING_IDLE_RECHECK * 3
        ));
    }

    #[test]
    fn visible_idle_bypasses_plain_idle_hold() {
        let now = std::time::Instant::now();
        let previous = publish_state(AgentState::Working);
        let mut next = publish_state(AgentState::Idle);
        next.visible_idle = true;
        let mut pending = PendingIdleConfirmation::default();

        assert!(!pending.should_hold_working_to_idle(previous, next, false, false, now));
    }

    #[test]
    fn transition_decision_publishes_next_for_visible_blocker() {
        let now = std::time::Instant::now();
        let mut pending_idle = PendingIdleConfirmation::default();
        let mut blocked = publish_state(AgentState::Blocked);
        blocked.visible_blocker = true;

        assert_eq!(
            decide_detection_transition(
                DetectionTransitionInput {
                    previous_publish: publish_state(AgentState::Idle),
                    next_publish: blocked,
                    agent_changed: false,
                    process_exited: false,
                    stable_refresh_due: false,
                    now,
                },
                &mut pending_idle,
            ),
            DetectionTransitionDecision::PublishNext
        );
    }

    #[test]
    fn screen_publish_keeps_visible_working_without_pty_activity() {
        let now = std::time::Instant::now();
        let mut pending_idle = PendingIdleConfirmation::default();

        assert_eq!(
            decide_screen_detection_publish(
                screen_publish_input(AgentState::Idle, screen_detection(AgentState::Working), now,),
                &mut pending_idle,
            ),
            DetectionPublishDecision::Publish {
                state: AgentState::Working,
                visible_idle: false,
                visible_blocker: false,
                visible_working: true,
                process_exited: false,
            }
        );
    }

    #[test]
    fn screen_publish_can_publish_idle_without_input_taint_delay() {
        let now = std::time::Instant::now();
        let mut pending_idle = PendingIdleConfirmation::default();

        assert_eq!(
            decide_screen_detection_publish(
                screen_publish_input(AgentState::Blocked, screen_detection(AgentState::Idle), now,),
                &mut pending_idle,
            ),
            DetectionPublishDecision::Publish {
                state: AgentState::Idle,
                visible_idle: true,
                visible_blocker: false,
                visible_working: false,
                process_exited: false,
            }
        );
    }

    #[test]
    fn detection_content_change_tracks_raw_nonempty_reads_for_scan_scheduling() {
        let seq = AtomicU64::new(0);

        observe_detection_content_change(b"", &seq);
        assert_eq!(seq.load(Ordering::Relaxed), 0);

        observe_detection_content_change(b"\x1b[?2026h", &seq);
        assert_eq!(seq.load(Ordering::Relaxed), 1);

        observe_detection_content_change(b"body bytes", &seq);
        assert_eq!(seq.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn local_terminal_mutations_can_invalidate_idle_scan_skip() {
        let seq = AtomicU64::new(0);

        mark_detection_content_changed(&seq);

        assert_eq!(seq.load(Ordering::Relaxed), 1);
    }

    fn published(agent: AgentKind, screen: &str, title: &str) -> AgentDetection {
        apply_driver_controls(
            screen,
            crate::agents::detect_agent_with_osc(Some(agent), screen, title, ""),
            false,
        )
    }

    #[test]
    fn action_required_title_does_not_block_a_live_working_screen() {
        // Captured from codex worker 12114 while Bus had posted false Blocked
        // notices and `bus state` showed working with dialog=false. The working
        // line sits above the footer, so the bottom-four fallback never sees it.
        let captured = include_str!("../../../tests/fixtures/codex-activity/working-room-1.txt");
        for screen in [captured.to_owned(), captured.replace("2m 38s", "17m 40s")] {
            let result = published(AgentKind::Codex, &screen, "[ ! ] Action Required | bus");
            assert_eq!(result.state, AgentState::Working, "{screen}");
            assert!(result.visible_working, "{screen}");
            assert!(!result.visible_blocker, "{screen}");
        }
    }

    #[test]
    fn a_queued_send_hint_or_quoted_working_line_stays_blocked() {
        for screen in [
            "• Messages to be submitted after next tool call (press esc to interrupt and send immediately)\n› Ask Codex to do anything\n",
            "› Explain the text ◦ Working (3s • esc to interrupt)\n",
        ] {
            let result = published(AgentKind::Codex, screen, "[ ! ] Action Required | bus");
            assert_eq!(result.state, AgentState::Blocked, "{screen}");
            assert!(result.visible_blocker, "{screen}");
        }
    }

    #[test]
    fn a_real_trust_prompt_stays_blocked_under_an_action_required_title() {
        let screen = include_str!("../../../tests/fixtures/grok-dialog/trust.txt");
        let result = published(AgentKind::Grok, screen, "[ ! ] Action Required");
        assert_eq!(result.state, AgentState::Blocked);
        assert!(result.visible_blocker);
        assert!(!result.visible_working);
    }

    #[test]
    fn a_foreground_question_beside_a_working_line_stays_blocked() {
        let screen = include_str!("../../../tests/fixtures/codex-question/expanded-working.txt");
        let later = screen.replace("(51s", "(52s");
        for screen in [screen, later.as_str()] {
            let result = published(AgentKind::Codex, screen, "[ ! ] Action Required | bus");
            assert_eq!(result.state, AgentState::Blocked, "{screen}");
            assert!(result.visible_blocker, "{screen}");
        }
    }

    #[test]
    fn an_empty_action_required_screen_stays_blocked() {
        let result = published(AgentKind::Codex, "", "Action Required");
        assert_eq!(result.state, AgentState::Blocked);
        assert!(result.visible_blocker);
    }
}
