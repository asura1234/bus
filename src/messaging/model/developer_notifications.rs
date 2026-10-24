//! Shared developer alert sources: explicit notices and tagged agent messages in every room.
use super::{Author, BusState, Prompt, PromptId, RequestId, RequestPhase};
use crate::messaging::attention::{
    has_you_mention, plain_line, waiting_action, waiting_tag_action,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum Key {
    Prompt(PromptId),
    Final(RequestId),
}

struct Message<'a> {
    sender: &'a str,
    text: &'a str,
    directed: bool,
    developer_context: bool,
    at_ms: u64,
}

/// Retained and archived sources share stable IDs. Snapshot repeats, archive moves and
/// coalesced finals cannot replay an alert; the caller's initial snapshot is its baseline.
/// `waiting_only` keeps just `[waiting-on-you]`-tagged hard blockers: Feishu reaches the
/// developer away from the desk, so untagged posts and replies stay in the TUI (sound + dot).
pub(crate) fn new_developer_notifications(
    previous: &BusState,
    next: &BusState,
    waiting_only: bool,
) -> Vec<String> {
    let known = messages(previous);
    let mut added: Vec<_> = messages(next)
        .into_iter()
        .filter(|(key, _)| !known.contains_key(key))
        .collect();
    added.sort_by_key(|(_, message)| message.at_ms);
    added
        .into_iter()
        .filter_map(|(_, message)| {
            let sender = plain_line(message.sender);
            // Feishu needs the literal tag: the legacy phrase also matches "nothing is
            // waiting on you", which turned plain status replies into DMs.
            let action = if message.developer_context && !waiting_only {
                waiting_action(message.text)
            } else {
                waiting_tag_action(message.text)
            };
            if let Some(action) = action {
                Some(format!(
                    "{sender} is waiting on you: {}",
                    plain_line(action)
                ))
            } else if !waiting_only && (message.directed || has_you_mention(message.text)) {
                let line = plain_line(message.text.trim_start());
                let line = if line.is_empty() {
                    "Attached files"
                } else {
                    &line
                };
                Some(format!("{sender} to you: {line}"))
            } else {
                None
            }
        })
        .collect()
}

fn messages(state: &BusState) -> BTreeMap<Key, Message<'_>> {
    let mut result = BTreeMap::new();
    for room in state.rooms() {
        for notice in state.notices(room.id) {
            insert_prompt(state, notice, &mut result);
        }
    }
    let forwarded: BTreeSet<_> = state
        .reply_routes
        .values()
        .filter_map(|route| route.notification())
        .chain(
            state
                .worker_events
                .values()
                .filter_map(|event| event.route.notification()),
        )
        .collect();
    for request in state.requests() {
        // These prompts quote an already observed reply or room fact, not a new agent send.
        insert_prompt(state, &request.prompt, &mut result);
        if forwarded.contains(&request.id) {
            if let Some(message) = result.get_mut(&Key::Prompt(request.prompt.id)) {
                message.directed = false;
                message.text = "";
            }
        }
        if request.phase == RequestPhase::Completed {
            if let Some(reply) = &request.pending_final {
                let key = Key::Final(request.group.unwrap_or(request.id));
                let developer_context = request.prompt.author == Author::Developer
                    || result
                        .get(&key)
                        .is_some_and(|message| message.developer_context);
                result.insert(
                    key,
                    Message {
                        sender: state
                            .agent(request.agent_id)
                            .map_or("Bus", |a| a.name.as_str()),
                        text: crate::messaging::attention::text(&reply.text).unwrap_or(&reply.text),
                        directed: false,
                        developer_context,
                        at_ms: request.completed_at_ms.unwrap_or(reply.received_at_ms),
                    },
                );
            }
        }
    }
    result
}

fn insert_prompt<'a>(
    state: &'a BusState,
    prompt: &'a Prompt,
    messages: &mut BTreeMap<Key, Message<'a>>,
) {
    let (sender, directed) = match prompt.author {
        Author::Agent(id) => (
            state.agent(id).map_or("Bus", |a| a.name.as_str()),
            prompt.recipient_ids.is_empty(),
        ),
        Author::Bus => match &prompt.attention_origin {
            Some(origin) => (origin.agent_name.as_str(), false),
            None => return,
        },
        Author::Developer => return,
    };
    messages.insert(
        Key::Prompt(prompt.id),
        Message {
            sender,
            text: crate::messaging::attention::text(&prompt.text).unwrap_or(&prompt.text),
            directed,
            developer_context: prompt.recipient_ids.is_empty(),
            at_ms: prompt.submitted_at_ms,
        },
    );
}
