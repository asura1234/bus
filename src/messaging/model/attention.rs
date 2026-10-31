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
            idle_agent_ids: Default::default(),
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
            Some(true) if !self.attention_episodes.contains(&key) => {
                if !self.quota_episode_continues(&key, now_ms) {
                    self.post_developer_attention(agent, text, now_ms)?;
                }
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

    /// Corrections to a still-future reset belong to the reported episode.
    fn quota_episode_continues(&self, key: &str, now_ms: u64) -> bool {
        if !key.starts_with("quota:") {
            return false;
        }
        let Some((family, _)) = key.rsplit_once(':') else {
            return false;
        };
        self.attention_episodes.iter().any(|old| {
            old.rsplit_once(':').is_some_and(|(old_family, reset)| {
                old_family == family
                    && reset
                        .parse::<u64>()
                        .is_ok_and(|reset| reset > now_ms / 1000)
            })
        })
    }

    /// Keep one current reset record per provider/window, including corrections.
    fn clear_previous_quota_episode(&mut self, key: &str) {
        if key.starts_with("quota:") {
            if let Some((family, _)) = key.rsplit_once(':') {
                self.attention_episodes.retain(|old| {
                    old.rsplit_once(':')
                        .is_none_or(|(old_family, _)| old_family != family)
                });
            }
        }
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
