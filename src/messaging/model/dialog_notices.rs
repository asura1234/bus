//! Provenance for blocked notices, independent of the current dialog episode.
use super::{AgentId, Author, BusState, DialogNoticeTo, Draft, ModelError, Prompt, RoomId};

pub(crate) const BLOCKED_MESSAGE: &str = "Blocked, needs help to continue.";

impl BusState {
    pub(crate) fn post_blocked_notice(
        &mut self,
        id: AgentId,
        now_ms: u64,
    ) -> Result<Option<DialogNoticeTo>, ModelError> {
        let agent = self.agent(id).ok_or(ModelError::UnknownAgent(id))?;
        let room = agent.room_id;
        if agent.deletion_pending || self.room(room).is_none() {
            return Ok(None);
        }
        let text = format!(
            "{}. {BLOCKED_MESSAGE}\nInspect: bus agent observe {}",
            self.agent_message_label(id),
            self.agent_cli_selector(id)
        );
        if self.is_developer_attention_author(room, &Author::Agent(id)) {
            self.post_developer_attention(id, &text, now_ms)?;
            return Ok(None);
        }
        if let Some(orchestrator) = self.notice_orchestrator(room, Some(id)) {
            let requests = self.submit_message_from(
                room,
                Draft {
                    text,
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Bus,
                now_ms,
            )?;
            let Some(&request) = requests.first() else {
                return Ok(None);
            };
            let prompt = &mut self
                .requests
                .get_mut(&request)
                .ok_or(ModelError::UnknownRequest(request))?
                .prompt;
            prompt.dialog_subject = Some(id);
            let prompt_id = prompt.id;
            self.tag_latest_dialog_notice(room, prompt_id, id);
            return Ok(Some(DialogNoticeTo::Orchestrator(request)));
        }
        self.post_dialog_notice_to_developer(room, id, text, now_ms)?;
        Ok(Some(DialogNoticeTo::Developer))
    }

    pub(super) fn post_dialog_notice_to_developer(
        &mut self,
        room: RoomId,
        subject: AgentId,
        text: String,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let id = self.post_bus_notice(room, None, text, now_ms)?;
        if let Some(notice) = self
            .rooms
            .get_mut(&room)
            .and_then(|room| room.notices.last_mut())
        {
            notice.dialog_subject = Some(subject);
        }
        self.tag_latest_dialog_notice(room, id, subject);
        Ok(())
    }

    fn tag_latest_dialog_notice(
        &mut self,
        room: RoomId,
        prompt: super::PromptId,
        subject: AgentId,
    ) {
        if let Some(latest) = self
            .rooms
            .get_mut(&room)
            .and_then(|room| room.latest_prompt.as_mut())
        {
            if latest.id == prompt {
                latest.dialog_subject = Some(subject);
            }
        }
    }

    /// Retained notices remain history. Pending delivery and client alerts
    /// require a live subject, including across rename and name reuse.
    pub(crate) fn stale_dialog_notice(&self, room: RoomId, prompt: &Prompt) -> bool {
        let subject = match prompt.dialog_subject {
            Some(subject) => Some(subject),
            None => {
                let Some((label, explicit_id)) = legacy_blocked_notice(prompt) else {
                    return false;
                };
                prompt
                    .attention_origin
                    .as_ref()
                    .map(|origin| origin.agent_id)
                    .or(explicit_id)
                    .or_else(|| self.legacy_dialog_subject(room, prompt, label))
            }
        };
        subject
            .and_then(|id| self.agent(id))
            .is_none_or(|agent| agent.deletion_pending)
    }

    fn legacy_dialog_subject(&self, room: RoomId, prompt: &Prompt, label: &str) -> Option<AgentId> {
        // A live legacy episode can attest its request even after a rename.
        if let Some(agent) = self.agents().find(|agent| {
            agent.room_id == room
                && matches!(agent.dialog_notice_to,
                    Some(DialogNoticeTo::Orchestrator(request))
                        if self.request(request).is_some_and(|r| r.prompt.id == prompt.id))
        }) {
            return Some(agent.id);
        }
        // IDs share one allocation sequence. A later agent with the same
        // name cannot own an older notice.
        self.agents()
            .filter(|agent| agent.room_id == room && agent.name == label)
            .map(|agent| agent.id)
            .chain(self.retired_agents.iter().filter_map(|(id, agent)| {
                (agent.room == room && agent.name == label).then_some(*id)
            }))
            .filter(|id| id.0 < prompt.id.0)
            .max()
    }
}

fn legacy_blocked_notice(prompt: &Prompt) -> Option<(&str, Option<AgentId>)> {
    if prompt.author != Author::Bus || !prompt.files.is_empty() {
        return None;
    }
    let text = crate::messaging::attention::text(&prompt.text).unwrap_or(&prompt.text);
    let (headline, selector) = text.split_once("\nInspect: bus agent observe ")?;
    let label = headline.strip_suffix(BLOCKED_MESSAGE)?.strip_suffix(". ")?;
    if let Ok(id) = selector.parse() {
        return Some((label, Some(AgentId(id))));
    }
    let matches_name = |name: &str| selector == format!("'{}'", name.replace('\'', "'\\''"));
    if matches_name(label) {
        return Some((label, None));
    }
    // Older sessions could add an explicit ID to a duplicate live name.
    let (name, suffix) = label.rsplit_once(" (agent ")?;
    let id = suffix.strip_suffix(')')?.parse().ok()?;
    matches_name(name).then_some((name, Some(AgentId(id))))
}
