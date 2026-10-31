use super::{AgentId, Author, BusState, RoomId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AttentionOrigin {
    pub(crate) room_id: RoomId,
    pub(crate) room_name: String,
    pub(crate) agent_id: AgentId,
    pub(crate) agent_name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct AttentionDelivery {
    pub(crate) initialized: bool,
    pub(crate) generation: u64,
    pub(crate) handled: BTreeSet<String>,
    pub(crate) interrupted: BTreeSet<String>,
    pub(crate) recent_attempts: Vec<AttentionAttempt>,
    pub(crate) last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AttentionAttempt {
    pub(crate) source: String,
    pub(crate) signature: String,
    pub(crate) at_ms: u64,
}

impl AttentionDelivery {
    pub(super) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

impl BusState {
    pub(crate) fn prune_attention(&mut self) -> bool {
        let before = (
            self.attention_delivery.handled.len(),
            self.attention_delivery.interrupted.len(),
            self.attention_episodes.len(),
        );
        let sources: BTreeSet<_> = self
            .attention_sources()
            .map(|(source, _, _)| source)
            .collect();
        self.attention_delivery
            .handled
            .retain(|source| sources.contains(source));
        self.attention_delivery
            .interrupted
            .retain(|source| self.attention_delivery.handled.contains(source));
        let episodes: BTreeSet<_> = self
            .attention_episodes
            .iter()
            .filter(|key| {
                if let Some(id) = key
                    .strip_prefix("stall:")
                    .and_then(|id| id.parse::<u64>().ok())
                {
                    return self.request(super::RequestId(id)).is_some();
                }
                if let Some((id, launch)) = key
                    .strip_prefix("exit:")
                    .and_then(|value| value.split_once(':'))
                {
                    return id
                        .parse::<u64>()
                        .ok()
                        .and_then(|id| self.agent(AgentId(id)))
                        .is_some_and(|agent| {
                            agent
                                .runtime_identity
                                .launch_id
                                .as_deref()
                                .unwrap_or("unknown")
                                == launch
                        });
                }
                key.starts_with("quota:")
            })
            .cloned()
            .collect();
        self.attention_episodes = episodes;
        before
            != (
                self.attention_delivery.handled.len(),
                self.attention_delivery.interrupted.len(),
                self.attention_episodes.len(),
            )
    }

    pub(crate) fn post_developer_attention(
        &mut self,
        agent: AgentId,
        text: &str,
        now_ms: u64,
    ) -> Result<(), super::ModelError> {
        let origin = self
            .attention_origin(agent)
            .ok_or(super::ModelError::UnknownAgent(agent))?;
        if self
            .agent(agent)
            .is_some_and(|agent| agent.deletion_pending)
        {
            return Ok(());
        }
        let master = self.ensure_master_room();
        let id = super::PromptId(self.allocate_id());
        let visible = self.visible_room == Some(master);
        let room = self
            .rooms
            .get_mut(&master)
            .ok_or(super::ModelError::UnknownRoom(master))?;
        let prompt = super::Prompt {
            id,
            author: Author::Bus,
            text: format!("{} {text}", crate::messaging::attention::TAG),
            files: Vec::new(),
            recipient_ids: Default::default(),
            submitted_at_ms: now_ms,
            attention_origin: Some(origin),
        };
        room.latest_prompt = Some(prompt.clone());
        room.notices.push(prompt);
        if !visible {
            room.unread_count = room.unread_count.saturating_add(1);
            room.unseen_to_developer = true;
        }
        Ok(())
    }

    pub(crate) fn attention_episode(
        &mut self,
        key: String,
        active: Option<bool>,
        agent: AgentId,
        text: &str,
        now_ms: u64,
    ) -> Result<(), super::ModelError> {
        match active {
            Some(true) if !self.episode_latched(&key) => {
                self.post_developer_attention(agent, text, now_ms)?;
                self.clear_previous_quota_episode(&key);
                self.attention_episodes.insert(key);
            }
            Some(false) => {
                self.clear_previous_quota_episode(&key);
                self.attention_episodes.remove(&key);
            }
            _ => {}
        }
        Ok(())
    }

    /// One open episode per provider, window, and reset. A suffix after the reset is the same episode.
    fn episode_latched(&self, key: &str) -> bool {
        match quota_reset(key) {
            Some((family, reset)) => self.attention_episodes.iter().any(|old| {
                quota_reset(old).is_some_and(|(old_family, old_reset)| {
                    old_family == family && old_reset == reset
                })
            }),
            None => self.attention_episodes.contains(key),
        }
    }

    /// Drop other resets for this provider and window. Recovery and a new reset both do this.
    fn clear_previous_quota_episode(&mut self, key: &str) {
        let Some((family, _)) = quota_reset(key) else {
            return;
        };
        self.attention_episodes
            .retain(|old| quota_reset(old).is_none_or(|(old_family, _)| old_family != family));
    }

    pub(super) fn unconfirmed_submission_message(prompt: super::PromptId) -> String {
        format!(
            "The outcome of message {} is unknown (submission_unconfirmed). \
             The agent may have run it. Bus will not retry automatically. \
             Check the agent's transcript and composer before resending. Inspect: bus message status {}",
            prompt.0, prompt.0
        )
    }

    pub(crate) fn record_stalled_errors(&mut self, now_ms: u64) -> Result<(), super::ModelError> {
        let observed: Vec<_> = self
            .requests
            .values()
            .filter(|request| request.group.is_none() && !request.settled())
            .map(|request| {
                (
                    request.id,
                    request.prompt.id,
                    request.agent_id,
                    self.stall_reason(request, now_ms),
                )
            })
            .collect();
        // A request keeps its alert marker through Working/Idle flaps.
        // prune_attention removes it only after the request leaves history.
        for (request, prompt, agent, reason) in observed {
            if let Some(reason) = reason {
                let message = if reason == "submission_unconfirmed" {
                    Self::unconfirmed_submission_message(prompt)
                } else {
                    format!("Agent stalled past its timeout on message {} ({reason}). Inspect: bus message status {}", prompt.0, prompt.0)
                };
                if self
                    .attention_episodes
                    .insert(format!("stall:{}", request.0))
                {
                    tracing::warn!(
                        event = "bus.message.stalled",
                        request_id = request.0,
                        message_id = prompt.0,
                        agent_id = agent.0,
                        reason,
                        detail = message,
                        "Request is stalled; inspect message status"
                    );
                }
            }
        }
        Ok(())
    }

    pub(crate) fn record_error_episode(&mut self, key: String) -> bool {
        self.attention_episodes.insert(key)
    }

    pub(crate) fn attention_origin(&self, id: AgentId) -> Option<AttentionOrigin> {
        let agent = self.agent(id)?;
        let room = self.room(agent.orchestrates.unwrap_or(agent.room_id))?;
        Some(AttentionOrigin {
            room_id: room.id,
            room_name: room.name.clone(),
            agent_id: id,
            agent_name: agent.name.clone(),
        })
    }

    pub(crate) fn attention_sources(
        &self,
    ) -> impl Iterator<Item = (String, AttentionOrigin, &str)> {
        let master = self.master_room().map(|room| room.id);
        let notices = master
            .into_iter()
            .flat_map(|room| self.notices(room))
            .filter_map(move |notice| {
                if !self.is_developer_attention_author(master?, &notice.author)
                    || crate::messaging::attention::text(&notice.text).is_none()
                {
                    return None;
                }
                let origin = notice
                    .attention_origin
                    .clone()
                    .or_else(|| match notice.author {
                        Author::Agent(id) => self.attention_origin(id),
                        _ => None,
                    })?;
                Some((
                    format!("notice:{}", notice.id.0),
                    origin,
                    notice.text.as_str(),
                ))
            });
        let replies = self.requests().filter_map(move |request| {
            if Some(request.room_id) != master
                || request.reply().is_none()
                || !self.is_developer_attention_author(
                    request.room_id,
                    &Author::Agent(request.agent_id),
                )
            {
                return None;
            }
            let reply = request.reply()?;
            crate::messaging::attention::text(&reply.text)?;
            Some((
                format!("final:{}:{}", request.id.0, reply.callback_id),
                self.attention_origin(request.agent_id)?,
                reply.text.as_str(),
            ))
        });
        notices.chain(replies)
    }

    pub(crate) fn is_developer_attention_author(&self, room: RoomId, author: &Author) -> bool {
        if self.master_room().is_none_or(|master| master.id != room) {
            return false;
        }
        match author {
            Author::Bus => true,
            Author::Agent(id) => self
                .agent(*id)
                .is_some_and(|agent| agent.room_id == room && agent.orchestrates.is_some()),
            Author::Developer => false,
        }
    }
}

/// `quota:{provider}:{window}:{reset}` with an optional `:{suffix}`.
fn quota_reset(key: &str) -> Option<(&str, u64)> {
    let rest = key.strip_prefix("quota:")?;
    let mut parts = rest.split(':');
    let provider = parts.next()?;
    let window = parts.next()?;
    let reset = parts.next()?.parse::<u64>().ok()?;
    if provider.is_empty() || window.is_empty() {
        return None;
    }
    let family_len = "quota:".len() + provider.len() + 1 + window.len();
    Some((&key[..family_len], reset))
}

#[cfg(test)]
mod tests {
    use super::super::{BusState, Provider};

    fn quota_room() -> (BusState, super::super::AgentId) {
        let mut state = BusState::default();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let agent = state
            .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
            .unwrap();
        state.bind_orchestrator(agent, work).unwrap();
        (state, agent)
    }

    fn master_texts(state: &BusState) -> Vec<String> {
        let master = state.master_room().unwrap().id;
        state
            .notices(master)
            .map(|notice| notice.text.clone())
            .collect()
    }

    #[test]
    fn quota_same_key_and_suffix_page_once() {
        let (mut state, agent) = quota_room();
        let key = "quota:claude:five_hour:1793368200";
        let text = format!(
            "Provider usage limit reached ({key}). Wait for reset or choose an available provider."
        );
        let start = 1_793_364_695_031;
        for minute in 0..3 {
            state
                .attention_episode(
                    key.to_string(),
                    Some(true),
                    agent,
                    &text,
                    start + minute * 60_000,
                )
                .unwrap();
        }
        state
            .attention_episode(
                format!("{key}:observed-by-12"),
                Some(true),
                agent,
                &text,
                start + 3 * 60_000,
            )
            .unwrap();
        let notices = master_texts(&state);
        assert_eq!(
            notices.len(),
            1,
            "same reset must page once even if a suffix changes: {notices:?}"
        );
        assert!(notices[0].contains(key), "{notices:?}");
    }

    #[test]
    fn quota_new_reset_pages_again_while_the_old_reset_is_still_ahead() {
        let (mut state, agent) = quota_room();
        let start = 1_793_364_695_031;
        let first = "quota:claude:five_hour:1793368200";
        state
            .attention_episode(first.to_string(), Some(true), agent, "first", start)
            .unwrap();
        state
            .attention_episode(
                "quota:claude:five_hour:1793400000".to_string(),
                Some(true),
                agent,
                "next window",
                start + 60_000,
            )
            .unwrap();
        let notices = master_texts(&state);
        assert_eq!(
            notices,
            vec![
                "[needs_developer_attention] first".to_string(),
                "[needs_developer_attention] next window".to_string(),
            ],
            "a new reset timestamp must page once more"
        );
    }

    #[test]
    fn quota_recovery_rearms_and_another_provider_pages() {
        let (mut state, agent) = quota_room();
        let start = 1_793_364_695_031;
        let key = "quota:claude:five_hour:1793368200";
        state
            .attention_episode(key.to_string(), Some(true), agent, "limited", start)
            .unwrap();
        state
            .attention_episode(key.to_string(), Some(false), agent, "", start + 60_000)
            .unwrap();
        state
            .attention_episode(
                key.to_string(),
                Some(true),
                agent,
                "limited again",
                start + 120_000,
            )
            .unwrap();
        state
            .attention_episode(
                "quota:codex:weekly:1793774141".to_string(),
                Some(true),
                agent,
                "Provider usage limit reached (quota:codex:weekly:1793774141).",
                start + 180_000,
            )
            .unwrap();
        let notices = master_texts(&state);
        assert_eq!(notices.len(), 3, "{notices:?}");
        assert!(
            notices[2].contains("quota:codex:weekly:1793774141"),
            "{notices:?}"
        );
    }
}
