use crate::agents::AgentKind;
use crate::terminal::emulator::PaneTerminal;

/// Runtime detection owns process selection; emulator consumers receive only
/// the resulting neutral job, and invoke the probe when their existing policy needs it.
pub(super) fn foreground_job(shell_pid: u32) -> Option<crate::platform::ForegroundJob> {
    crate::agents::foreground_job(shell_pid)
}

pub(super) const AGENT_MISS_CONFIRMATION_ATTEMPTS: u8 = 6;

pub(super) const PROCESS_RECHECK_IDENTIFIED: std::time::Duration =
    std::time::Duration::from_secs(5);

pub(super) const PROCESS_RECHECK_MISSING_FOREGROUND_GROUP: std::time::Duration =
    std::time::Duration::from_secs(30);

pub(super) const PROCESS_ACQUISITION_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(8);

pub(super) const PROCESS_ACQUISITION_FAST_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(1500);

pub(super) const PROCESS_ACQUISITION_FAST_RECHECK: std::time::Duration =
    std::time::Duration::from_millis(500);

pub(super) const PROCESS_ACQUISITION_SLOW_RECHECK: std::time::Duration =
    std::time::Duration::from_secs(2);

pub(super) const PROCESS_ACQUISITION_IDLE_RESET: std::time::Duration =
    std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
pub(super) struct AgentDetectionPresence {
    pub(super) current_agent: Option<AgentKind>,
    pub(super) consecutive_misses: u8,
}

#[cfg(unix)]
pub(super) fn absolute_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    crate::platform::process_cwd(pid).filter(|cwd| cwd.is_absolute())
}

#[cfg(unix)]
pub(super) fn usable_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    absolute_process_cwd(pid).filter(|cwd| cwd.is_dir())
}

#[cfg(unix)]
pub(super) fn foreground_member_cwd_different_from_shell(
    shell_pid: u32,
    shell_cwd: Option<&std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let job = crate::agents::foreground_job(shell_pid)?;
    for process in job.processes {
        if process.pid == shell_pid {
            continue;
        }
        let Some(cwd) = absolute_process_cwd(process.pid) else {
            continue;
        };
        if shell_cwd != Some(&cwd) {
            return Some(cwd);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ForegroundShellAgentAction {
    ObserveProbe,
    ReportProcessExit,
    ReportReplacementProcess,
    ClearAgent,
}

pub(super) fn foreground_shell_agent_action(
    previous_agent: Option<AgentKind>,
    new_agent: Option<AgentKind>,
    foreground_is_pane_shell: bool,
    process_exit_reported: bool,
) -> ForegroundShellAgentAction {
    let Some(previous_agent) = previous_agent else {
        return ForegroundShellAgentAction::ObserveProbe;
    };
    if process_exit_reported {
        return if new_agent == Some(previous_agent) {
            ForegroundShellAgentAction::ReportReplacementProcess
        } else if new_agent.is_none() {
            ForegroundShellAgentAction::ClearAgent
        } else {
            ForegroundShellAgentAction::ObserveProbe
        };
    }
    if new_agent.is_some() {
        return ForegroundShellAgentAction::ObserveProbe;
    }

    if foreground_is_pane_shell {
        // Do not clear identity immediately. First publish an idle process-exit
        // transition for the previous agent so notifications and wait-agent callers
        // observe completion before the pane becomes unknown.
        return ForegroundShellAgentAction::ReportProcessExit;
    }

    ForegroundShellAgentAction::ObserveProbe
}

/// Drops retained OSC evidence when changing away from an identified agent.
/// First acquisition keeps bytes that the newly identified process may have
/// emitted before the process probe recognized it.
pub(super) fn clear_osc_evidence_for_agent_transition(
    terminal: &PaneTerminal,
    previous_agent: Option<AgentKind>,
) {
    if previous_agent.is_some() {
        terminal.clear_agent_osc_state();
    }
}

pub(super) fn apply_foreground_shell_agent_action(
    agent_presence: &mut AgentDetectionPresence,
    action: ForegroundShellAgentAction,
    previous_agent: Option<AgentKind>,
    new_agent: Option<AgentKind>,
    pending_foreground_shell_clear: &mut bool,
    foreground_shell_exit_reported: &mut bool,
) -> bool {
    match action {
        ForegroundShellAgentAction::ReportReplacementProcess => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.observe_process_probe(previous_agent);
            true
        }
        ForegroundShellAgentAction::ReportProcessExit => {
            *pending_foreground_shell_clear = true;
            false
        }
        ForegroundShellAgentAction::ClearAgent => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.clear_current_agent()
        }
        ForegroundShellAgentAction::ObserveProbe => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.observe_process_probe(new_agent)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProcessProbeInput {
    pub(super) current_agent: Option<AgentKind>,
    pub(super) foreground_pgid: Option<u32>,
    pub(super) last_foreground_pgid: Option<u32>,
    pub(super) has_process_probe: bool,
    pub(super) acquisition_age: Option<std::time::Duration>,
    pub(super) pending_foreground_shell_clear: bool,
    pub(super) pending_restore_probe: bool,
    pub(super) elapsed_since_process_check: std::time::Duration,
}

pub(super) fn foreground_group_changed(
    foreground_pgid: Option<u32>,
    last_foreground_pgid: Option<u32>,
) -> bool {
    foreground_pgid != last_foreground_pgid
}

// Only kernel-observed foreground groups drive change detection. Remembering an
// inferred group would look like a change on every tick while the kernel stays silent.
pub(super) fn process_group_for_change_tracking(
    observed_foreground_pgid: Option<u32>,
    probed_process_group_id: Option<u32>,
) -> Option<u32> {
    observed_foreground_pgid?;
    probed_process_group_id.or(observed_foreground_pgid)
}

#[cfg(any(windows, test))]
pub(super) fn should_observe_foreground_process_group(
    content_changed: bool,
    input: ProcessProbeInput,
) -> bool {
    !input.has_process_probe
        || input.current_agent.is_none()
        || input.pending_foreground_shell_clear
        || input.pending_restore_probe
        || content_changed
        || input.elapsed_since_process_check >= PROCESS_RECHECK_IDENTIFIED
}

pub(super) fn should_probe_foreground_job(input: ProcessProbeInput) -> bool {
    if input.pending_foreground_shell_clear || input.pending_restore_probe {
        return true;
    }

    let foreground_group_changed =
        foreground_group_changed(input.foreground_pgid, input.last_foreground_pgid);

    if let Some(acquisition_age) = input.acquisition_age {
        let acquisition_interval = if acquisition_age <= PROCESS_ACQUISITION_FAST_WINDOW {
            PROCESS_ACQUISITION_FAST_RECHECK
        } else {
            PROCESS_ACQUISITION_SLOW_RECHECK
        };
        if acquisition_age <= PROCESS_ACQUISITION_WINDOW
            && input.elapsed_since_process_check >= acquisition_interval
        {
            return true;
        }
    }

    if input.current_agent.is_none() {
        return !input.has_process_probe
            || foreground_group_changed
            || (input.foreground_pgid.is_none()
                && input.elapsed_since_process_check >= PROCESS_RECHECK_MISSING_FOREGROUND_GROUP);
    }

    foreground_group_changed || input.elapsed_since_process_check >= PROCESS_RECHECK_IDENTIFIED
}

pub(super) fn sync_content_change_acquisition(
    current_agent: Option<AgentKind>,
    process_group_changed: bool,
    content_changed: bool,
    now: std::time::Instant,
    acquisition_started_at: &mut Option<std::time::Instant>,
    last_content_change_at: &mut Option<std::time::Instant>,
) {
    if current_agent.is_some() || process_group_changed {
        return;
    }

    if content_changed {
        let should_start = acquisition_started_at.is_none_or(|started| {
            now.duration_since(started) > PROCESS_ACQUISITION_WINDOW
                && last_content_change_at.is_none_or(|last_change| {
                    now.duration_since(last_change) >= PROCESS_ACQUISITION_IDLE_RESET
                })
        });
        if should_start {
            *acquisition_started_at = Some(now);
        }
        *last_content_change_at = Some(now);
        return;
    }

    let Some(acquisition_started) = *acquisition_started_at else {
        return;
    };
    let Some(last_content_change) = *last_content_change_at else {
        return;
    };

    if now.duration_since(acquisition_started) > PROCESS_ACQUISITION_WINDOW
        && now.duration_since(last_content_change) >= PROCESS_ACQUISITION_IDLE_RESET
    {
        *acquisition_started_at = None;
        *last_content_change_at = None;
    }
}

#[derive(Debug, Clone)]
pub(super) struct ProcessProbeResult {
    pub(super) process_group_id: Option<u32>,
    pub(super) foreground_is_pane_shell: bool,
    pub(super) agent: Option<AgentKind>,
    pub(super) process_name: Option<String>,
}

pub(super) fn agent_hint_for_foreground_job_members(
    job: &crate::platform::ForegroundJob,
    read_hint: impl Fn(u32) -> Option<AgentKind>,
) -> Option<AgentKind> {
    read_hint(job.process_group_id)
        .or_else(|| agent_hint_for_non_leader_foreground_job_members(job, read_hint))
}

pub(super) fn agent_hint_for_non_leader_foreground_job_members(
    job: &crate::platform::ForegroundJob,
    read_hint: impl Fn(u32) -> Option<AgentKind>,
) -> Option<AgentKind> {
    job.processes
        .iter()
        .filter(|process| process.pid != job.process_group_id)
        .find_map(|process| read_hint(process.pid))
}

pub(super) fn identify_process_group_leader_in_job(
    job: &crate::platform::ForegroundJob,
) -> Option<(AgentKind, String)> {
    let leader = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)?;
    let leader_job = crate::platform::ForegroundJob {
        process_group_id: job.process_group_id,
        processes: vec![leader.clone()],
    };
    crate::agents::identify_agent_in_job(&leader_job)
}

pub(super) fn process_probe_result(
    job: &crate::platform::ForegroundJob,
    pid: u32,
    agent: AgentKind,
    process_name: String,
) -> ProcessProbeResult {
    ProcessProbeResult {
        process_group_id: Some(job.process_group_id),
        foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
        agent: Some(agent),
        process_name: Some(process_name),
    }
}

pub(super) fn hinted_process_probe_result(
    job: &crate::platform::ForegroundJob,
    pid: u32,
    read_hint: impl Fn(u32) -> Option<AgentKind>,
) -> Option<ProcessProbeResult> {
    let agent = agent_hint_for_foreground_job_members(job, read_hint)?;
    Some(process_probe_result(
        job,
        pid,
        agent,
        crate::agents::agent_label(agent).to_string(),
    ))
}

pub(super) fn probe_foreground_process_from_jobs(
    pid: u32,
    foreground_pgid: Option<u32>,
    leader_job: Option<crate::platform::ForegroundJob>,
    foreground_job: impl FnOnce() -> Option<crate::platform::ForegroundJob>,
    read_hint: impl Fn(u32) -> Option<AgentKind> + Copy,
) -> ProcessProbeResult {
    if let Some(job) = leader_job.as_ref() {
        if let Some(hinted) = hinted_process_probe_result(job, pid, read_hint) {
            return hinted;
        }
        if let Some((agent, process_name)) = crate::agents::identify_agent_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
    }

    let foreground_job = foreground_job();
    if let Some(job) = foreground_job.as_ref() {
        if let Some(agent) = read_hint(job.process_group_id) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::agents::agent_label(agent).to_string(),
            );
        }
        if let Some((agent, process_name)) = identify_process_group_leader_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
        if let Some(agent) = agent_hint_for_non_leader_foreground_job_members(job, read_hint) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::agents::agent_label(agent).to_string(),
            );
        }

        let identified = crate::agents::identify_agent_in_job(job);
        return ProcessProbeResult {
            process_group_id: Some(job.process_group_id),
            foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
            agent: identified.as_ref().map(|(agent, _)| *agent),
            process_name: identified.map(|(_, process_name)| process_name),
        };
    }

    ProcessProbeResult {
        process_group_id: foreground_pgid,
        foreground_is_pane_shell: false,
        agent: None,
        process_name: None,
    }
}

pub(super) fn probe_foreground_process(
    pid: u32,
    foreground_pgid: Option<u32>,
) -> ProcessProbeResult {
    probe_foreground_process_from_jobs(
        pid,
        foreground_pgid,
        foreground_pgid.and_then(crate::agents::foreground_group_leader_job),
        || crate::agents::foreground_job(pid),
        crate::agents::process_agent_hint,
    )
}

impl AgentDetectionPresence {
    pub(super) fn from_agent(current_agent: Option<AgentKind>) -> Self {
        Self {
            current_agent,
            consecutive_misses: 0,
        }
    }

    pub(super) fn current_agent(&self) -> Option<AgentKind> {
        self.current_agent
    }

    pub(super) fn clear_current_agent(&mut self) -> bool {
        if self.current_agent.is_none() {
            self.consecutive_misses = 0;
            return false;
        }
        self.current_agent = None;
        self.consecutive_misses = 0;
        true
    }

    pub(super) fn observe_process_probe(&mut self, identified_agent: Option<AgentKind>) -> bool {
        match identified_agent {
            Some(agent) => {
                self.consecutive_misses = 0;
                if Some(agent) == self.current_agent {
                    return false;
                }
                self.current_agent = Some(agent);
                true
            }
            None => {
                if self.current_agent.is_none() {
                    self.consecutive_misses = 0;
                    return false;
                }
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses < AGENT_MISS_CONFIRMATION_ATTEMPTS {
                    return false;
                }
                self.current_agent = None;
                self.consecutive_misses = 0;
                true
            }
        }
    }
}

/// Follows the identified agent job's leader by incarnation. Returns the previous
/// leader when a different one took its place and the previous one is gone.
pub(super) fn track_agent_leader(
    leader: &mut Option<crate::platform::ProcessInstance>,
    identified_agent: Option<AgentKind>,
    agent_present: bool,
    process_group_id: Option<u32>,
) -> Option<crate::platform::ProcessInstance> {
    if identified_agent.is_none() {
        // An exit keeps its leader until the exit is published.
        if !agent_present {
            *leader = None;
        }
        return None;
    }
    let current = process_group_id.and_then(|pid| match *leader {
        Some(known) if known.pid == pid => Some(known),
        _ => crate::platform::process_instance(pid),
    })?;
    let previous = leader.replace(current);
    previous.filter(|previous| *previous != current && !process_is_alive(*previous))
}

pub(super) fn process_is_alive(process: crate::platform::ProcessInstance) -> bool {
    crate::platform::process_instance(process.pid) == Some(process)
}
