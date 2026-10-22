//! Installs a managed agent's own update when its startup chooser offers one.
//!
//! Left alone, the chooser blocked a fresh agent, and a queued message's Enter
//! could pick "Update now" itself, after which the agent exited under the
//! message. Bus instead picks the option the manifest's `auto_update` names,
//! waits for the agent to exit, and types its launch command again in the same
//! pane under the same name: the `agent.start` command, or for a restored agent
//! the resume command its restore typed. When the install fails, the agent
//! starts again anyway and Bus leaves its chooser for a person, so the agent
//! shows blocked with `AgentInfo::update_error` and queued messages wait.
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::agents::AgentState;
use crate::protocol::api::schema::AgentStartParams;
use crate::server::app::App;
use crate::server::app_state::AppState;
use crate::terminal::runtime::DialogChoice;
use crate::terminal::{SelfUpdate, SelfUpdatePhase};
use crate::utils::ids::{PaneId, TerminalId};

/// How long an install may run before Bus interrupts it.
const SELF_UPDATE_INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long the agent may take to exit after an interrupt, and the shell to
/// take the relaunch command after the agent exited.
const SELF_UPDATE_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const RELAUNCH_RETRY: Duration = Duration::from_millis(500);

impl AppState {
    pub(crate) fn next_self_update_deadline(&self) -> Option<Instant> {
        self.terminals
            .values()
            .filter_map(|terminal| match terminal.self_update.as_ref()?.phase {
                SelfUpdatePhase::Installing { deadline, .. } => Some(deadline),
                SelfUpdatePhase::Relaunching { next_try, .. } => Some(next_try),
                SelfUpdatePhase::Stuck(_)
                | SelfUpdatePhase::Relaunched
                | SelfUpdatePhase::Failed(_) => None,
            })
            .min()
    }
}

impl App {
    /// Advances every pane's self-update; called after terminal state events
    /// and at `next_self_update_deadline`.
    pub(crate) fn supervise_self_updates(&mut self, now: Instant) -> bool {
        let panes: Vec<_> = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, workspace)| {
                workspace.tabs.iter().flat_map(move |tab| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| {
                            tab.panes
                                .get(&pane_id)
                                .map(|pane| (ws_idx, pane_id, pane.attached_terminal_id.clone()))
                        })
                })
            })
            .collect();
        let mut changed = false;
        for (ws_idx, pane_id, terminal_id) in panes {
            if self.supervise_self_update(ws_idx, pane_id, &terminal_id, now) {
                changed = true;
                self.emit_pane_updated(ws_idx, pane_id);
            }
        }
        changed
    }

    fn supervise_self_update(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        terminal_id: &TerminalId,
        now: Instant,
    ) -> bool {
        let Some(terminal) = self.state.terminals.get(terminal_id) else {
            return false;
        };
        let Some(mut update) = terminal.self_update.clone() else {
            return self.answer_update_chooser(terminal_id, now);
        };
        update.phase = match update.phase.clone() {
            SelfUpdatePhase::Installing {
                deadline,
                interrupted,
            } => {
                let exited = terminal.managed_agent_kind().is_none();
                match self.installing(terminal_id, &update, exited, deadline, interrupted, now) {
                    Some(phase) => phase,
                    None => return false,
                }
            }
            SelfUpdatePhase::Relaunching {
                failure,
                next_try,
                deadline,
            } => {
                if now < next_try {
                    return false;
                }
                let Some(phase) = self.relaunch(ws_idx, pane_id, &update, failure, deadline, now)
                else {
                    if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
                        terminal.self_update = None;
                    }
                    return true;
                };
                phase
            }
            // The interrupted install finally exited: start the agent again
            // once, with its chooser left for a person.
            SelfUpdatePhase::Stuck(_) if terminal.managed_agent_kind().is_none() => {
                SelfUpdatePhase::Relaunching {
                    failure: Some(INTERRUPT_IGNORED.to_owned()),
                    next_try: now,
                    deadline: now + SELF_UPDATE_EXIT_TIMEOUT,
                }
            }
            SelfUpdatePhase::Stuck(_) => return false,
            SelfUpdatePhase::Relaunched | SelfUpdatePhase::Failed(_)
                if terminal.managed_agent_interactive_ready()
                    && terminal.state == AgentState::Idle =>
            {
                // Running again, or a person answered the chooser.
                if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
                    terminal.self_update = None;
                }
                return true;
            }
            SelfUpdatePhase::Relaunched if self.update_chooser_visible(terminal_id) => {
                let reason = "the agent offered the same update again after installing it";
                tracing::warn!(event = "bus.self_update.failed", agent = %update.name, reason,
                    "Leaving the agent's update chooser for a person");
                SelfUpdatePhase::Failed(failed_install(&update, reason))
            }
            SelfUpdatePhase::Relaunched | SelfUpdatePhase::Failed(_) => return false,
        };
        // The agent just exited: start it again in this same pass.
        let relaunch_now = matches!(
            update.phase,
            SelfUpdatePhase::Relaunching { next_try, .. } if next_try <= now
        );
        let Some(terminal) = self.state.terminals.get_mut(terminal_id) else {
            return false;
        };
        terminal.self_update = Some(update);
        if relaunch_now {
            self.supervise_self_update(ws_idx, pane_id, terminal_id, now);
        }
        true
    }

    /// The next phase of an install, or `None` while it is still running.
    fn installing(
        &self,
        terminal_id: &TerminalId,
        update: &SelfUpdate,
        exited: bool,
        deadline: Instant,
        interrupted: bool,
        now: Instant,
    ) -> Option<SelfUpdatePhase> {
        let runtime = self.terminal_runtimes.get(terminal_id)?;
        if exited {
            // Codex prints its success line last, before it exits.
            let screen = runtime.visible_text();
            let failure = if interrupted {
                Some(format!(
                    "the install did not finish within {} minutes",
                    SELF_UPDATE_INSTALL_TIMEOUT.as_secs() / 60
                ))
            } else if screen.contains(&update.success) {
                None
            } else {
                Some(exit_failure(&screen))
            };
            return Some(SelfUpdatePhase::Relaunching {
                failure,
                next_try: now,
                deadline: now + SELF_UPDATE_EXIT_TIMEOUT,
            });
        }
        if now < deadline {
            return None;
        }
        if interrupted {
            // Bus does not kill it; a person stops it, and its exit still
            // relaunches the agent.
            tracing::warn!(event = "bus.self_update.failed", agent = %update.name,
                reason = INTERRUPT_IGNORED, "The agent's update ignored Bus's interrupt");
            return Some(SelfUpdatePhase::Stuck(format!(
                "Bus could not install the {} update ({INTERRUPT_IGNORED}). Stop it in {}'s terminal; Bus then starts {} again and queued messages wait.",
                crate::agents::agent_label(update.kind),
                update.name,
                update.name
            )));
        }
        // A hung install: interrupting it makes the agent exit with a
        // failure, which relaunches it for a person.
        if let Err(error) = runtime.try_send_bytes(Bytes::from_static(b"\x03")) {
            tracing::warn!(event = "bus.self_update.interrupt_failed", agent = %update.name, %error,
                "Could not interrupt a hung agent update");
        }
        Some(SelfUpdatePhase::Installing {
            deadline: now + SELF_UPDATE_EXIT_TIMEOUT,
            interrupted: true,
        })
    }

    /// Types the agent's launch command again. `None` drops the update: the
    /// pane is gone, or its shell never took the command and is a plain shell
    /// with no agent to list an error on.
    fn relaunch(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        update: &SelfUpdate,
        failure: Option<String>,
        deadline: Instant,
        now: Instant,
    ) -> Option<SelfUpdatePhase> {
        let public_pane_id = self.public_pane_id(ws_idx, pane_id)?;
        let started = self.start_agent(AgentStartParams {
            name: update.name.clone(),
            kind: crate::agents::agent_label(update.kind).into(),
            pane_id: public_pane_id,
            args: update.args.clone(),
            executable: None,
            grok_home: None,
            timeout_ms: None,
        });
        Some(match started {
            Ok(_) => {
                // A resumed agent's own session, which its exit cleared and
                // Codex reports again only on its next prompt.
                if let Some(session) = update.session.clone() {
                    if let Some(terminal) = self.state.workspaces[ws_idx]
                        .terminal_id(pane_id)
                        .cloned()
                        .and_then(|id| self.state.terminals.get_mut(&id))
                    {
                        terminal.set_managed_agent_launch_session(session);
                    }
                }
                tracing::info!(event = "bus.self_update.relaunched", agent = %update.name,
                    failure = ?failure, "Started the agent again after its update");
                match failure {
                    Some(reason) => SelfUpdatePhase::Failed(failed_install(update, &reason)),
                    None => SelfUpdatePhase::Relaunched,
                }
            }
            // The shell may not have the terminal back yet.
            Err(_) if now < deadline => SelfUpdatePhase::Relaunching {
                failure,
                next_try: now + RELAUNCH_RETRY,
                deadline,
            },
            Err(error) => {
                let message = self.agent_start_error_body(error).message;
                tracing::warn!(event = "bus.self_update.relaunch_failed", agent = %update.name,
                    %message, "Could not start the agent again after its update");
                return None;
            }
        })
    }

    /// Picks the update in a managed agent's self-update chooser, once.
    fn answer_update_chooser(&mut self, terminal_id: &TerminalId, now: Instant) -> bool {
        let (Some(terminal), Some(runtime)) = (
            self.state.terminals.get(terminal_id),
            self.terminal_runtimes.get(terminal_id),
        ) else {
            return false;
        };
        let (Some(kind), Some(name), Some(args)) = (
            terminal.managed_agent_kind(),
            terminal.agent_name.clone(),
            terminal.managed_agent_args.clone(),
        ) else {
            return false;
        };
        if terminal.state != AgentState::Blocked {
            return false;
        }
        let Some(spec) = crate::agents::manifest::auto_update(kind, &runtime.visible_text()) else {
            return false;
        };
        match runtime.try_choose_dialog_option_labeled(&spec.choose) {
            Ok(DialogChoice::Sent(keys)) => {
                tracing::info!(event = "bus.self_update.chosen", agent = %name, ?keys,
                    "Chose the agent's update in its startup chooser");
            }
            // A redraw between reading and choosing; the next event retries.
            Ok(outcome) => {
                tracing::debug!(event = "bus.self_update.not_chosen", agent = %name, ?outcome,
                    "Update chooser did not take the choice");
                return false;
            }
            Err(error) => {
                tracing::warn!(event = "bus.self_update.choose_failed", agent = %name, %error,
                    "Could not send the update choice");
                return false;
            }
        }
        let Some(terminal) = self.state.terminals.get_mut(terminal_id) else {
            return false;
        };
        let session = terminal.persisted_agent_session.clone();
        // Otherwise a started agent reads launch-pending for the whole install,
        // and the coordinator closes its queued messages as stalled.
        terminal.finish_managed_launch_for_update();
        terminal.self_update = Some(SelfUpdate {
            name,
            kind,
            args,
            session,
            success: spec.success,
            phase: SelfUpdatePhase::Installing {
                deadline: now + SELF_UPDATE_INSTALL_TIMEOUT,
                interrupted: false,
            },
        });
        true
    }

    fn update_chooser_visible(&self, terminal_id: &TerminalId) -> bool {
        let (Some(terminal), Some(runtime)) = (
            self.state.terminals.get(terminal_id),
            self.terminal_runtimes.get(terminal_id),
        ) else {
            return false;
        };
        terminal.state == AgentState::Blocked
            && terminal.managed_agent_kind().is_some_and(|kind| {
                crate::agents::manifest::auto_update(kind, &runtime.visible_text()).is_some()
            })
    }
}

/// The arguments to type again after `kind`'s update: the command a restore
/// typed into the pane's shell, without its executable. A restored agent's
/// command resumes its own session, so its relaunch does too.
pub(super) fn managed_relaunch_args(
    kind: Option<crate::agents::AgentKind>,
    argv: &[String],
) -> Option<Vec<String>> {
    let (executable, args) = argv.split_first()?;
    (executable == crate::agents::interactive_agent_executable(kind?)).then(|| args.to_vec())
}

const INTERRUPT_IGNORED: &str = "the install did not exit after Bus interrupted it";

fn failed_install(update: &SelfUpdate, reason: &str) -> String {
    format!(
        "Bus could not install the {} update ({reason}). Answer the update prompt in {}'s terminal; queued messages wait until then.",
        crate::agents::agent_label(update.kind),
        update.name
    )
}

/// The updater's error the agent printed before it exited. The shell prompt
/// follows it, so the last line is not it.
fn exit_failure(screen: &str) -> String {
    screen
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            let line = line.to_lowercase();
            line.contains("error") || line.contains("failed")
        })
        .map_or_else(
            || "the agent exited without reporting a finished update".to_owned(),
            |line| format!("the agent exited after: {line}"),
        )
}

#[cfg(test)]
#[path = "tests/self_update_test.rs"]
mod tests;
