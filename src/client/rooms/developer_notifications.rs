//! Desktop notification policy for messages addressed to the developer in MASTER.
use super::{BusUi, Effect};
use crate::messaging::{
    coordinator::BusCommand,
    model::{Author, BusState, PromptId, RequestId, RequestPhase, RoomId},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

const BURST_WINDOW: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct DeveloperNotifications {
    allowed: bool,
    pending: Option<Burst>,
}

struct Burst {
    deadline: Instant,
    sender: String,
    preview: String,
    count: usize,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum MessageKey {
    Notice(PromptId),
    Final(RequestId),
}

struct Message<'a> {
    sender: &'a str,
    text: &'a str,
    at_ms: u64,
}

impl DeveloperNotifications {
    pub(super) fn configure(&mut self, enabled: bool, master_visible: bool, focus: Option<bool>) {
        self.allowed = enabled && !(master_visible && focus != Some(false));
        if !self.allowed {
            self.pending = None;
        }
    }

    pub(super) fn observe(&mut self, previous: &BusState, next: &BusState, now: Instant) {
        if !self.allowed {
            return;
        }
        let known = messages(previous);
        let next = messages(next);
        let mut added: Vec<_> = next
            .iter()
            .filter(|(key, _)| !known.contains_key(key))
            .collect();
        added.sort_by_key(|(key, message)| (message.at_ms, *key));
        for (_, message) in added {
            if let Some(burst) = self.pending.as_mut() {
                burst.count = burst.count.saturating_add(1);
            } else {
                self.pending = Some(Burst {
                    deadline: now + BURST_WINDOW,
                    sender: plain_preview(message.sender, usize::MAX),
                    preview: plain_preview(message.text, 100),
                    count: 1,
                });
            }
        }
    }

    pub(super) fn take(&mut self, now: Instant) -> Option<String> {
        if self
            .pending
            .as_ref()
            .is_none_or(|burst| now < burst.deadline)
        {
            return None;
        }
        let burst = self.pending.take()?;
        Some(if burst.count == 1 {
            format!("{}: {}", burst.sender, burst.preview)
        } else {
            format!("{} and {} more", burst.sender, burst.count - 1)
        })
    }
}

/// Compare retained sources, including archived history, so snapshot repeats and
/// archive moves cannot replay a notification. A coalesced request group is one final.
fn messages(state: &BusState) -> BTreeMap<MessageKey, Message<'_>> {
    let mut result = BTreeMap::new();
    let Some(master) = state.master_room() else {
        return result;
    };
    for notice in state
        .notices(master.id)
        .filter(|notice| notice.recipient_ids.is_empty())
    {
        let sender = match notice.author {
            Author::Agent(id) => state
                .agent(id)
                .map(|agent| agent.name.as_str())
                .unwrap_or("Bus"),
            Author::Bus => match notice.attention_origin.as_ref() {
                Some(origin) => &origin.agent_name,
                None => continue,
            },
            Author::Developer => continue,
        };
        result.insert(
            MessageKey::Notice(notice.id),
            Message {
                sender,
                text: visible_text(state, master.id, &notice.author, &notice.text),
                at_ms: notice.submitted_at_ms,
            },
        );
    }
    for request in state.requests().filter(|request| {
        request.room_id == master.id
            && request.phase == RequestPhase::Completed
            && request.prompt.author == Author::Developer
    }) {
        if let Some(reply) = &request.pending_final {
            let sender = state
                .agent(request.agent_id)
                .map(|agent| agent.name.as_str())
                .unwrap_or("Bus");
            result.insert(
                MessageKey::Final(request.group.unwrap_or(request.id)),
                Message {
                    sender,
                    text: visible_text(
                        state,
                        master.id,
                        &Author::Agent(request.agent_id),
                        &reply.text,
                    ),
                    at_ms: request.completed_at_ms.unwrap_or(reply.received_at_ms),
                },
            );
        }
    }
    result
}

/// Match MASTER's content projection before applying the preview budget.
fn visible_text<'a>(state: &BusState, room: RoomId, author: &Author, text: &'a str) -> &'a str {
    if state.is_developer_attention_author(room, author) {
        crate::messaging::attention::text(text).unwrap_or(text)
    } else {
        text
    }
}

/// Literal text only: never include another line, terminal controls, or the
/// rest of a long message. Character limits are Unicode-safe.
fn plain_preview(text: &str, limit: usize) -> String {
    let mut chars = text
        .split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .next()
        .unwrap_or("")
        .chars()
        .filter_map(|ch| {
            if ch == '\t' {
                Some(' ')
            } else {
                (!ch.is_control()).then_some(ch)
            }
        });
    let mut preview: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        preview.push('…');
    }
    preview.trim().to_owned()
}

impl BusUi {
    pub(super) fn developer_notifications_field(&self) -> usize {
        self.freeze_field() + 1
    }

    pub(super) fn toggle_developer_notifications(&mut self) {
        self.settings_field = self.developer_notifications_field();
        let on = !self.settings.developer_notifications;
        self.settings.developer_notifications = on;
        self.configure_developer_notifications();
        self.queue(BusCommand::SetDeveloperNotifications(on), Effect::None);
    }

    pub(super) fn configure_developer_notifications(&mut self) {
        let master_visible = self
            .snapshot
            .state
            .master_room()
            .is_some_and(|master| self.room == Some(master.id))
            && self.terminal.is_none()
            && self.form.is_none()
            && self.deletion.is_none()
            && self.alert.is_none();
        self.developer_notifications.configure(
            self.settings.developer_notifications,
            master_visible,
            self.outer_focused,
        );
    }

    pub(super) fn take_developer_notification(&mut self, now: Instant) -> Option<String> {
        self.configure_developer_notifications();
        self.developer_notifications.take(now)
    }
}

#[cfg(test)]
#[path = "tests/developer_notifications_test.rs"]
mod tests;
