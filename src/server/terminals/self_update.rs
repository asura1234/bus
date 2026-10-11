//! Installs a managed agent's own update when its startup chooser offers one.
//!
//! Left alone, the chooser blocked a fresh agent, and a queued message's Enter
//! could pick "Update now" itself, after which the agent exited under the
//! message. Bus instead picks the option the manifest's `auto_update` names,
//! waits for the agent to exit, and types the same `agent.start` command again
//! in the same pane under the same name. When the install fails, the agent
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
pub(crate) const SELF_UPDATE_INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long the agent may take to exit after an interrupt, and the shell to
/// take the relaunch command after the agent exited.
pub(crate) const SELF_UPDATE_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const RELAUNCH_RETRY: Duration = Duration::from_millis(500);

impl AppState {
    pub(crate) fn next_self_update_deadline(&self) -> Option<Instant> {
        self.terminals
            .values()
            .filter_map(|terminal| match terminal.self_update.as_ref()?.phase {
                SelfUpdatePhase::Installing { deadline, .. } => Some(deadline),
                SelfUpdatePhase::Relaunching { next_try, .. } => Some(next_try),
                SelfUpdatePhase::Relaunched | SelfUpdatePhase::Failed(_) => None,
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
        let (Some(terminal), Some(runtime)) = (
            self.state.terminals.get(terminal_id),
            self.terminal_runtimes.get(terminal_id),
        ) else {
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
                if terminal.managed_agent_kind().is_none() {
                    // The agent exited: Codex prints its success line last.
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
                    SelfUpdatePhase::Relaunching {
                        failure,
                        next_try: now,
                        deadline: now + SELF_UPDATE_EXIT_TIMEOUT,
                    }
                } else if now < deadline {
                    return false;
                } else if interrupted {
                    SelfUpdatePhase::Failed(failed_install(
                        &update,
                        "the install did not exit after Bus interrupted it",
                    ))
                } else {
                    // A hung install: interrupting it makes the agent exit
                    // with a failure, which relaunches it for a person.
                    if let Err(error) = runtime.try_send_bytes(Bytes::from_static(b"\x03")) {
                        tracing::warn!(event = "bus.self_update.interrupt_failed", agent = %update.name, %error,
                            "Could not interrupt a hung agent update");
                    }
                    SelfUpdatePhase::Installing {
                        deadline: now + SELF_UPDATE_EXIT_TIMEOUT,
                        interrupted: true,
                    }
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
                let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
                    return false;
                };
                let started = self.start_agent(AgentStartParams {
                    name: update.name.clone(),
                    kind: crate::agents::agent_label(update.kind).into(),
                    pane_id: public_pane_id,
                    args: update.args.clone(),
                    timeout_ms: None,
                });
                match started {
                    Ok(_) => {
                        tracing::info!(event = "bus.self_update.relaunched", agent = %update.name,
                            failure = ?failure, "Started the agent again after its update");
                        match failure {
                            Some(reason) => {
                                SelfUpdatePhase::Failed(failed_install(&update, &reason))
                            }
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
                        SelfUpdatePhase::Failed(format!(
                            "Bus could not start {} again after its update: {message}. Start it again in its terminal.",
                            update.name
                        ))
                    }
                }
            }
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
                SelfUpdatePhase::Failed(failed_install(
                    &update,
                    "the agent offered the same update again after installing it",
                ))
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
        terminal.self_update = Some(SelfUpdate {
            name,
            kind,
            args,
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
