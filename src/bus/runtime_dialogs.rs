//! Tells a room's orchestrator when an agent waits on a dialog.
use super::*;
use serde_json::Value;

/// The notice key for a blocked screen without a readable dialog.
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
                Some(BLOCKED) => format!(
                    "{name} (agent {}) in room {room} is blocked, but Bus cannot read a dialog on its screen. Inspect it with:\nbus agent read {} --source visible",
                    id.0, id.0
                ),
                Some(_) => match self.observe_dialog(id) {
                    Ok(observed) if !observed["dialog"].is_null() => {
                        dialog_notice(id, &name, &room, &observed)
                    }
                    // Closed or changed since the poll; the next poll decides.
                    Ok(_) => continue,
                    Err(error) => {
                        tracing::warn!(event = "bus.dialog.observe_failed", agent_id = id.0, %error,
                            "Dialog notice deferred");
                        continue;
                    }
                },
                // 保留已发出的 blocked 标记，屏幕来回闪时不再把同一条通知发第二次。
                None if agent.dialog_notice.as_deref() == Some(BLOCKED) => continue,
                None => match agent.dialog_answer {
                    Some(option) => format!("answered: option {option}"),
                    None => "answered".to_owned(),
                },
            };
            let mut state = self.state.clone();
            post_dialog_notice(&mut state, id, text)?;
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
        .map(|text| {
            if dialog["kind"] == "question" {
                text.to_owned()
            } else {
                wants_line(name, text)
            }
        })
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
    if dialog["kind"] == "question" {
        text.push_str(&format!("\nAnswer: bus agent dialog {id}, then bus agent answer {id} --text \"...\" or bus agent answer {id} --skip", id = id.0));
    } else {
        text.push_str(&format!(
            "\nAnswer: bus agent dialog {id}, then bus agent choose {id} --option N",
            id = id.0
        ));
    }
    text
}

fn who(name: &str, room: &str, id: AgentId) -> String {
    if room.is_empty() {
        format!("{name} (agent {})", id.0)
    } else {
        format!("{name} in {room} (agent {})", id.0)
    }
}

/// The command and its reason, or the question. Never hide a later command line.
fn wants_line(name: &str, text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(start) = lines.iter().position(|line| line.starts_with("$ ")) {
        let command = lines[start..]
            .iter()
            .map(|line| line.strip_prefix("$ ").unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        let mut notice = format!("{name} wants to run: {command}");
        if let Some(reason) = lines[..start]
            .iter()
            .position(|line| line.starts_with("Reason:"))
        {
            notice.push('\n');
            notice.push_str(&lines[reason..start].join("\n"));
        }
        return notice;
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

/// Delivers the notice to the room's orchestrator in MASTER, like any message.
/// Dialog notices are noise for the Human, who watches the agent's terminal:
/// the request is delivery-only (`Request::delivery_only`), and a room without
/// an orchestrator gets no notice at all.
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
        None => Ok(()),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "runtime_dialogs_tests.rs"]
mod tests;
