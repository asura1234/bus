//! Room history is a projection of durable requests, not the latest-reply cache.
use super::render::{provider, wrap, Action};
use crate::bus::model::*;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Text,
    Muted,
    You,
    Agent(AgentId),
}

pub(super) struct Line {
    pub text: String,
    pub action: Option<Action>,
    pub tone: Tone,
}

#[derive(Default)]
pub(super) struct History {
    key: Option<(u64, RoomId, u16, u64)>,
    source: Option<(u64, RoomId)>,
    signature: u64,
    lines: Vec<Line>,
}

enum Message<'a> {
    Prompt(&'a Prompt),
    Reply {
        request: RequestId,
        agent: AgentId,
        text: &'a str,
        at: u64,
    },
}

impl History {
    pub fn lines(
        &mut self,
        state: &BusState,
        room: &Room,
        width: u16,
        revision: u64,
        now: u64,
    ) -> &[Line] {
        if self.source != Some((revision, room.id)) {
            self.signature = signature(state, room);
            self.source = Some((revision, room.id));
        }
        let key = (self.signature, room.id, width, now / 60_000);
        if self.key == Some(key) {
            return &self.lines;
        }
        let mut messages = BTreeMap::new();
        let mut prompts = BTreeSet::new();
        let mut replies = BTreeSet::new();
        for request in state.requests().filter(|r| r.room_id == room.id) {
            let prompt = &request.prompt;
            if prompts.insert(prompt.id) {
                messages.insert(
                    (prompt.submitted_at_ms, prompt.id.0, 0),
                    Message::Prompt(prompt),
                );
            }
            if let Some(final_reply) = request
                .pending_final
                .as_ref()
                .filter(|_| request.phase == RequestPhase::Completed)
            {
                replies.insert(request.id);
                messages.insert(
                    (final_reply.received_at_ms, request.id.0, 1),
                    Message::Reply {
                        request: request.id,
                        agent: request.agent_id,
                        text: &final_reply.text,
                        at: final_reply.received_at_ms,
                    },
                );
            }
        }
        // Retain compatibility with saved latest-only records and a prompt
        // whose final recipient has since been deleted. Never duplicate it.
        if let Some(prompt) = room
            .latest_prompt
            .as_ref()
            .filter(|p| !prompts.contains(&p.id))
        {
            messages.insert(
                (prompt.submitted_at_ms, prompt.id.0, 0),
                Message::Prompt(prompt),
            );
        }
        for reply in room
            .latest_replies
            .values()
            .filter(|r| !replies.contains(&r.request_id))
        {
            messages.insert(
                (reply.received_at_ms, reply.request_id.0, 1),
                Message::Reply {
                    request: reply.request_id,
                    agent: reply.agent_id,
                    text: &reply.text,
                    at: reply.received_at_ms,
                },
            );
        }
        let mut lines = Vec::new();
        for message in messages.into_values() {
            let (header, text, tone, files, quote) = match message {
                Message::Prompt(prompt) => {
                    let recipients = prompt
                        .recipient_ids
                        .iter()
                        .filter_map(|id| state.agent(*id))
                        .map(|agent| agent.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    (
                        format!(
                            "You → {recipients}  {}",
                            timestamp(prompt.submitted_at_ms, now)
                        ),
                        prompt.text.as_str(),
                        Tone::You,
                        prompt.files.as_slice(),
                        None,
                    )
                }
                Message::Reply {
                    request,
                    agent,
                    text,
                    at,
                } => {
                    let Some(agent) = state.agent(agent) else {
                        continue;
                    };
                    (
                        format!(
                            "{}  {}  {}",
                            agent.name,
                            provider(agent.provider),
                            timestamp(at, now)
                        ),
                        text,
                        Tone::Agent(agent.id),
                        &[][..],
                        Some(request),
                    )
                }
            };
            lines.extend(wrap(&header, width).into_iter().map(|text| Line {
                text,
                action: None,
                tone,
            }));
            lines.extend(wrap(text, width).into_iter().map(|text| Line {
                text,
                action: None,
                tone: Tone::Text,
            }));
            for path in files {
                lines.push(Line {
                    text: format!(
                        "[{}]",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    action: Some(Action::FileDetail(path.clone())),
                    tone: Tone::Muted,
                });
            }
            if let Some(request) = quote {
                lines.push(Line {
                    text: "Quote".into(),
                    action: Some(Action::Quote(request)),
                    tone: Tone::Muted,
                });
            }
            lines.push(Line {
                text: String::new(),
                action: None,
                tone: Tone::Text,
            });
        }
        self.key = Some(key);
        self.lines = lines;
        &self.lines
    }
}

// Prompts and completed finals are immutable after creation. Check their
// identity/settlement metadata, not their potentially large text, when a
// global poll/draft revision arrives. Only a changed room history, names,
// width, or age label requires sorting and wrapping those immutable bodies.
fn signature(state: &BusState, room: &Room) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for request in state.requests().filter(|r| r.room_id == room.id) {
        request.id.0.hash(&mut hash);
        request.prompt.id.0.hash(&mut hash);
        request.prompt.submitted_at_ms.hash(&mut hash);
        if request.phase == RequestPhase::Completed {
            request
                .pending_final
                .as_ref()
                .map(|r| r.received_at_ms)
                .hash(&mut hash);
        }
    }
    room.latest_prompt.as_ref().map(|p| p.id.0).hash(&mut hash);
    for reply in room.latest_replies.values() {
        (reply.request_id.0, reply.received_at_ms).hash(&mut hash);
    }
    for agent in state.agents().filter(|a| a.room_id == room.id) {
        (agent.id.0, &agent.name, agent.color).hash(&mut hash);
    }
    hash.finish()
}

pub(super) fn reply(state: &BusState, room: RoomId, id: RequestId) -> Option<(AgentId, &str)> {
    if let Some(request) = state
        .request(id)
        .filter(|r| r.room_id == room && r.phase == RequestPhase::Completed)
    {
        if let Some(reply) = &request.pending_final {
            return Some((request.agent_id, &reply.text));
        }
    }
    state
        .room(room)?
        .latest_replies
        .values()
        .find(|r| r.request_id == id)
        .map(|r| (r.agent_id, r.text.as_str()))
}

pub(super) fn timestamp(at: u64, now: u64) -> String {
    if now.saturating_sub(at) > 86_400_000 {
        return "> 1 day".into();
    }
    i64::try_from(at / 1000)
        .ok()
        .and_then(crate::platform::local_datetime_at)
        .map(|local| format!("{:02}:{:02}", local.hour(), local.minute()))
        .unwrap_or_else(|| "--:--".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routine_poll_revisions_and_draft_edits_reuse_history_lines() {
        let mut state = BusState::default();
        let room = state.create_room("test").unwrap();
        let agent = state
            .create_agent(room, "agent", Provider::Codex, "/project".into(), None)
            .unwrap();
        state.set_draft_recipients(room, [agent]).unwrap();
        for i in 0..100 {
            state
                .set_draft_text(
                    room,
                    &format!("request {i}\n{}", "saved history ".repeat(100)),
                )
                .unwrap();
            state.submit_draft(room, i + 1).unwrap();
        }
        let mut history = History::default();
        let original = history
            .lines(&state, state.room(room).unwrap(), 80, 1, 1000)
            .as_ptr();
        for revision in 2..=30 {
            state
                .set_draft_text(room, "draft edits do not change history")
                .unwrap();
            state
                .set_room_notes(room, "notes do not change history")
                .unwrap();
            assert_eq!(
                history
                    .lines(&state, state.room(room).unwrap(), 80, revision, 1000)
                    .as_ptr(),
                original
            );
        }
        state.rename_agent(agent, "renamed").unwrap();
        assert!(history
            .lines(&state, state.room(room).unwrap(), 80, 31, 1000)
            .iter()
            .any(|line| line.text.contains("renamed")));
    }
    #[test]
    fn day_cutoff_is_elapsed_time_not_midnight() {
        let at = 1_800_000_000_000;
        assert_ne!(timestamp(at, at + 86_400_000), "> 1 day");
        assert_eq!(timestamp(at, at + 86_400_001), "> 1 day");
        assert_eq!(timestamp(at, at + 60_000), timestamp(at, at));
        assert_eq!(timestamp(at, at - 1), timestamp(at, at));
    }
}
