//! Tells a room's orchestrator, or the Human, when an agent waits on a dialog.
use super::*;
use serde_json::Value;

/// The notice key for a blocked screen without a readable numbered dialog.
pub(super) const BLOCKED: &str = "blocked";
/// Polls a change must hold before Bus reports it, so a redraw never counts.
const STEADY_POLLS: u8 = 2;

impl Worker {
    /// `waits` holds, for each agent the poll saw, its dialog `id`, `BLOCKED`,
    /// or `None` when it waits on nothing. Each distinct wait is reported once
    /// and its end once, unless it ended because Bus answered it.
    pub(super) fn notify_dialogs(
        &mut self,
        waits: Vec<(AgentId, Option<String>)>,
    ) -> Result<(), String> {
        for (id, wait) in waits {
            let steady = match self.dialog_seen.get_mut(&id) {
                Some((seen, polls)) if *seen == wait => {
                    *polls = polls.saturating_add(1);
                    *polls >= STEADY_POLLS
                }
                _ => {
                    self.dialog_seen.insert(id, (wait.clone(), 1));
                    false
                }
            };
            let Some(agent) = self.state.agent(id) else {
                continue;
            };
            if !steady || agent.dialog_notice == wait || agent.deletion_pending {
                continue;
            }
            let name = agent.name.clone();
            let room = self
                .state
                .room(agent.room_id)
                .map_or_else(String::new, |room| room.name.clone());
            let text = match wait.as_deref() {
                Some(BLOCKED) => Some(format!(
                    "{name} (agent {}) in room {room} is blocked, but Bus cannot read a numbered dialog on its screen. Inspect it with:\nbus agent read {} --source visible",
                    id.0, id.0
                )),
                Some(_) => match self.observe_dialog(id) {
                    Ok(observed) if !observed["dialog"].is_null() => {
                        Some(dialog_notice(id, &name, &room, &observed))
                    }
                    // Closed or changed since the poll; the next poll decides.
                    Ok(_) => continue,
                    Err(error) => {
                        tracing::warn!(event = "bus.dialog.observe_failed", agent_id = id.0, %error,
                            "Dialog notice deferred");
                        continue;
                    }
                },
                // 读不到选项的阻塞自己结束时不再追一条消息。
                None if agent.dialog_notice.as_deref() == Some(BLOCKED) => None,
                None => Some(match agent.dialog_answer {
                    Some(option) => format!("answered: option {option}"),
                    None => "answered".to_owned(),
                }),
            };
            let mut state = self.state.clone();
            if let Some(text) = text {
                post_dialog_notice(&mut state, id, text)?;
            }
            state
                .set_dialog_notice(id, wait)
                .map_err(|error| error.to_string())?;
            self.save(state)?;
        }
        let state = &self.state;
        self.dialog_seen.retain(|id, _| state.agent(*id).is_some());
        Ok(())
    }
}

fn dialog_notice(id: AgentId, name: &str, room: &str, observed: &Value) -> String {
    let dialog = &observed["dialog"];
    let mut text = format!("{}\n", who(name, room, id));
    if let Some(question) = dialog["text"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(|text| wants_line(name, text))
        .filter(|line| !line.is_empty())
    {
        text.push_str(&question);
        text.push('\n');
    }
    for option in dialog["options"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "{}. {}{}\n",
            option["number"],
            option["label"].as_str().unwrap_or_default(),
            if option["selected"] == true {
                " (selected)"
            } else {
                ""
            }
        ));
    }
    text.push_str(&format!(
        "\nAnswer: bus agent dialog {id}, then bus agent choose {id} --option N",
        id = id.0
    ));
    text
}

fn who(name: &str, room: &str, id: AgentId) -> String {
    if room.is_empty() {
        format!("{name} (agent {})", id.0)
    } else {
        format!("{name} in {room} (agent {})", id.0)
    }
}

/// 给人看的一行：要批准的命令，或问题本身。
fn wants_line(name: &str, text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(command) = lines.iter().find_map(|line| {
        line.strip_prefix("$ ")
            .map(str::trim)
            .filter(|command| !command.is_empty())
    }) {
        return format!("{name} wants to run: {command}");
    }
    let asks_to_run = lines.iter().any(|line| {
        let lower = line.to_lowercase();
        lower.contains("run the following command")
            || lower.contains("bash command")
            || lower.contains("requires approval")
            || lower.contains("do you want to proceed")
    });
    if asks_to_run {
        if let Some(command) = lines.iter().copied().find(|line| {
            let lower = line.to_lowercase();
            !line.ends_with('?')
                && !lower.contains("command")
                && !lower.contains("approval")
                && !lower.contains("proceed")
        }) {
            return format!("{name} wants to run: {command}");
        }
    }
    if let Some(question) = lines.iter().rev().find(|line| line.ends_with('?')) {
        return (*question).to_owned();
    }
    lines.last().copied().unwrap_or_default().to_owned()
}

/// Delivers the notice to the room's orchestrator in MASTER, like any message,
/// or posts it in the agent's own room for the Human when none orchestrates it.
fn post_dialog_notice(state: &mut BusState, id: AgentId, text: String) -> Result<(), String> {
    let room = state.agent(id).ok_or("Unknown agent")?.room_id;
    let now = crate::bus::io::now_ms();
    let orchestrator = state
        .orchestrator_of(room)
        .filter(|orchestrator| orchestrator.id != id && !orchestrator.deletion_pending)
        .map(|orchestrator| (orchestrator.id, orchestrator.room_id));
    match orchestrator {
        Some((orchestrator, master)) => state
            .submit_message_from(
                master,
                Draft {
                    text,
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Bus,
                now,
            )
            .map(|_| ()),
        None => state.post_notice(room, text, now).map(|_| ()),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "runtime_dialogs_tests.rs"]
mod tests;
