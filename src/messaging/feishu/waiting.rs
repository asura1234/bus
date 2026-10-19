//! Waiting-on-you detection and fixed two-second burst policy for MASTER.
use crate::messaging::model::{Author, BusState, PromptId, RequestId, RequestPhase, RoomId};
use std::collections::BTreeMap;

const BURST_WINDOW: u64 = 2_000;

#[derive(Debug, Default)]
pub(crate) struct WaitingNotifications {
    allowed: bool,
    pending: Option<Burst>,
}

#[derive(Debug)]
struct Burst {
    deadline: u64,
    lines: Vec<String>,
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

impl WaitingNotifications {
    pub(crate) fn configure(&mut self, enabled: bool) {
        self.allowed = enabled;
        if !self.allowed {
            self.pending = None;
        }
    }

    pub(crate) fn observe(&mut self, previous: &BusState, next: &BusState, now: u64) {
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
            let Some(action) = waiting_action(message.text) else {
                continue;
            };
            let line = format!(
                "{} is waiting on you: {}",
                plain_line(message.sender),
                plain_line(action)
            );
            if let Some(burst) = self.pending.as_mut() {
                burst.lines.push(line);
            } else {
                self.pending = Some(Burst {
                    deadline: now + BURST_WINDOW,
                    lines: vec![line],
                });
            }
        }
    }

    pub(crate) fn take(&mut self, now: u64) -> Option<String> {
        if self
            .pending
            .as_ref()
            .is_none_or(|burst| now < burst.deadline)
        {
            return None;
        }
        let burst = self.pending.take()?;
        Some(burst.lines.join("\n"))
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

/// Match MASTER's content projection before selecting the action line.
fn visible_text<'a>(state: &BusState, room: RoomId, author: &Author, text: &'a str) -> &'a str {
    if state.is_developer_attention_author(room, author) {
        crate::messaging::attention::text(text).unwrap_or(text)
    } else {
        text
    }
}

/// ASCII case folding preserves byte offsets into the original Unicode text.
/// The phrase also covers "still waiting on you", but not partial words.
fn waiting_action(text: &str) -> Option<&str> {
    const PHRASE: &str = "waiting on you";
    let folded = text.to_ascii_lowercase();
    let tag_end = crate::messaging::attention::WAITING_TAGS
        .iter()
        .filter_map(|tag| folded.find(tag).map(|start| start + tag.len()))
        .min();
    let phrase_end = folded.match_indices(PHRASE).find_map(|(start, _)| {
        let end = start + PHRASE.len();
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        (!text[..start].chars().next_back().is_some_and(is_word)
            && !text[end..].chars().next().is_some_and(is_word))
        .then_some(end)
    });
    let end = [tag_end, phrase_end].into_iter().flatten().min()?;
    // Close Markdown wrapped around the marker, preserving action text after a space.
    let action = text[end..]
        .trim_start_matches(':')
        .trim_start_matches(['*', '_', '`']);
    Some(action.trim_start_matches(|ch: char| ch.is_whitespace() || ch == ':'))
}

/// Preserve the full action line, filtering terminal controls without including
/// another line or any additional message context.
fn plain_line(text: &str) -> String {
    text.split(['\n', '\r', '\u{2028}', '\u{2029}'])
        .next()
        .unwrap_or("")
        .chars()
        .filter_map(|ch| {
            if ch == '\t' {
                Some(' ')
            } else {
                (!ch.is_control()).then_some(ch)
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
#[path = "tests/waiting_test.rs"]
mod tests;
