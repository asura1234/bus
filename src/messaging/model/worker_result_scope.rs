//! Which worker copy is this request's own result.
use super::reply_routing::{reply_key, ReplyRoute};
use super::worker_events::{WorkerEvent, WorkerEventKind};
use super::{AgentId, AgentRecipients, Author, BusState, PromptId, Request, RequestId, RoomId};

impl BusState {
    /// This request's own result landed within a minute of `at_ms`: a final
    /// grouped to it, or the worker copy assigned to its captured reply.
    pub(super) fn result_in_same_minute(&self, request: &Request, at_ms: u64) -> bool {
        let start = at_ms.saturating_sub(60_000);
        let end = at_ms.saturating_add(60_000);
        let group = reply_key(request);
        if self.worker_events.values().any(|event| {
            event.kind == WorkerEventKind::Final
                && event.group == Some(group)
                && (start..=end).contains(&event.at_ms)
        }) {
            return true;
        }
        self.assigned_copy_at(request, None, None)
            .is_some_and(|sent_at| (start..=end).contains(&sent_at))
    }

    /// The worker already sent this request's captured reply, at or after the
    /// request was submitted. An earlier request's identical text does not count.
    pub(super) fn captured_reply_already_sent(&self, request: &Request) -> bool {
        request
            .reply()
            .is_some_and(|reply| !reply.text.trim().is_empty())
            && self
                .assigned_copy_at(
                    request,
                    None,
                    self.reply_recipient(request)
                        .filter(|_| self.reply_orchestrator(request).is_none()),
                )
                .is_some()
    }

    /// A restart line covers only requests that already existed when it was posted.
    pub(super) fn restart_covers(&self, request: &Request, now_ms: u64) -> bool {
        self.latest_restart_ms(request.room_id, now_ms)
            .is_some_and(|restart_at| request.prompt.submitted_at_ms <= restart_at)
    }

    fn latest_restart_ms(&self, room: RoomId, now_ms: u64) -> Option<u64> {
        const WINDOW_MS: u64 = 15 * 60 * 1000;
        let mut latest = None;
        let mut note = |author: &Author, text: &str, at: u64| {
            if author == &Author::Bus
                && text.starts_with("Restarted ")
                && now_ms.saturating_sub(at) <= WINDOW_MS
            {
                latest = Some(latest.map_or(at, |prior: u64| prior.max(at)));
            }
        };
        for prompt in self.notices(room) {
            note(&prompt.author, &prompt.text, prompt.submitted_at_ms);
        }
        for other in self.requests().filter(|other| other.room_id == room) {
            note(
                &other.prompt.author,
                &other.prompt.text,
                other.prompt.submitted_at_ms,
            );
        }
        latest
    }

    /// When the worker's copy of `request`'s result was sent, if one is assigned
    /// to this request. `exclude` is a posted final's own message, which is not
    /// a second copy. Copies are claimed oldest-request first.
    fn assigned_copy_at(
        &self,
        request: &Request,
        exclude: Option<PromptId>,
        recipient: Option<AgentId>,
    ) -> Option<u64> {
        let text = request
            .reply()
            .map(|reply| reply.text.trim().to_owned())
            .filter(|text| !text.is_empty())?;
        self.assigned_result_at(
            request.room_id,
            request.agent_id,
            request.prompt.submitted_at_ms,
            request.id,
            &text,
            exclude,
            recipient,
        )
    }

    /// A grouped late final is a second copy only when this request already has
    /// its own worker message. An ungrouped final still yields to any earlier
    /// copy of the same text in the room.
    pub(super) fn final_text_already_posted(
        &self,
        late: Option<&Request>,
        room: RoomId,
        worker: AgentId,
        text: &str,
    ) -> bool {
        let Some(request) = late else {
            return self.result_already_in_room(room, worker, text);
        };
        self.assigned_result_at(
            request.room_id,
            request.agent_id,
            request.prompt.submitted_at_ms,
            request.id,
            text.trim(),
            None,
            None,
        )
        .is_some()
    }

    /// A grouped final is withdrawn when this request still receives a worker
    /// copy of its body after earlier same-text replies have taken theirs.
    /// An ungrouped final is withdrawn when any other worker copy exists.
    /// Matching uses the posted message, because the stored summary is only
    /// a preview once the body is long.
    pub(super) fn final_has_explicit_copy(&self, event: &WorkerEvent) -> bool {
        if event.kind != WorkerEventKind::Final {
            return false;
        }
        let body = self.final_source_text(event).trim().to_owned();
        if body.is_empty() {
            return false;
        }
        let Some(group) = event.group else {
            return self.other_worker_copy(event.room, event.agent, &body, Some(event.message));
        };
        let Some(request) = self.request(group) else {
            return self.other_worker_copy(event.room, event.agent, &body, Some(event.message));
        };
        let (room, worker, submit_at, id) = (
            request.room_id,
            request.agent_id,
            request.prompt.submitted_at_ms,
            request.id,
        );
        self.assigned_result_at(
            room,
            worker,
            submit_at,
            id,
            &body,
            Some(event.message),
            None,
        )
        .is_some()
    }

    /// The posted final keeps the full body. The event summary keeps only a
    /// preview once that body is longer than the notice cap.
    fn final_source_text<'a>(&'a self, event: &'a WorkerEvent) -> &'a str {
        self.prompt_text(event.room, event.message)
            .unwrap_or_else(|| final_body(&event.text))
    }

    fn prompt_text(&self, room: RoomId, id: PromptId) -> Option<&str> {
        self.notices(room)
            .find(|prompt| prompt.id == id)
            .map(|prompt| prompt.text.as_str())
            .or_else(|| {
                self.requests()
                    .find(|request| request.room_id == room && request.prompt.id == id)
                    .map(|request| request.prompt.text.as_str())
            })
    }

    fn other_worker_copy(
        &self,
        room: RoomId,
        worker: AgentId,
        text: &str,
        exclude: Option<PromptId>,
    ) -> bool {
        !self
            .worker_copies(room, worker, text, exclude, None)
            .is_empty()
    }

    fn assigned_result_at(
        &self,
        room: RoomId,
        worker: AgentId,
        submit_at: u64,
        request_id: RequestId,
        text: &str,
        exclude: Option<PromptId>,
        recipient: Option<AgentId>,
    ) -> Option<u64> {
        let copies = self.worker_copies(room, worker, text, exclude, recipient);
        if copies.is_empty() {
            return None;
        }
        let mut claimants = self.reply_claimants(room, worker, text, recipient);
        if !claimants.iter().any(|(_, id)| *id == request_id) {
            claimants.push((submit_at, request_id));
            claimants.sort_unstable();
        }
        let mut taken = vec![false; claimants.len()];
        for (copy_at, _) in copies {
            let Some(index) = claimants
                .iter()
                .enumerate()
                .find(|(index, (submit, id))| {
                    !taken[*index] && *submit <= copy_at && !self.reply_wake_delivered(*id)
                })
                .map(|(index, _)| index)
            else {
                continue;
            };
            if claimants[index].1 == request_id {
                return Some(copy_at);
            }
            taken[index] = true;
        }
        None
    }

    /// A request whose reply wake was already delivered does not take a later
    /// request's explicit copy.
    fn reply_wake_delivered(&self, request_id: RequestId) -> bool {
        let Some(request) = self.request(request_id) else {
            return false;
        };
        match self.reply_routes.get(&request.id).copied() {
            Some(ReplyRoute::Seen) => true,
            Some(ReplyRoute::Routed(notice)) => {
                self.request(notice).is_some_and(|queued| queued.settled())
            }
            Some(ReplyRoute::Pending) | None => false,
        }
    }

    fn worker_copies(
        &self,
        room: RoomId,
        worker: AgentId,
        text: &str,
        exclude: Option<PromptId>,
        recipient: Option<AgentId>,
    ) -> Vec<(u64, PromptId)> {
        let mut found = std::collections::BTreeMap::<PromptId, u64>::new();
        let mut keep =
            |id: PromptId, at: u64, author: &Author, stored: &str, recipients: &AgentRecipients| {
                if exclude == Some(id)
                    || author != &Author::Agent(worker)
                    || recipient.is_some_and(|target| !recipients.contains(&target))
                    || is_wrapper(stored)
                    || !same_result_text(stored, text)
                {
                    return;
                }
                found.entry(id).or_insert(at);
            };
        for prompt in self.notices(room) {
            keep(
                prompt.id,
                prompt.submitted_at_ms,
                &prompt.author,
                &prompt.text,
                &prompt.recipient_ids,
            );
        }
        for request in self.requests().filter(|request| request.room_id == room) {
            keep(
                request.prompt.id,
                request.prompt.submitted_at_ms,
                &request.prompt.author,
                &request.prompt.text,
                &request.prompt.recipient_ids,
            );
        }
        let mut copies: Vec<_> = found.into_iter().map(|(id, at)| (at, id)).collect();
        copies.sort_unstable();
        copies
    }

    fn reply_claimants(
        &self,
        room: RoomId,
        worker: AgentId,
        text: &str,
        recipient: Option<AgentId>,
    ) -> Vec<(u64, RequestId)> {
        let mut claimants: Vec<_> = self
            .requests()
            .filter(|request| {
                request.room_id == room
                    && request.agent_id == worker
                    && recipient.is_none_or(|target| request.prompt.author == Author::Agent(target))
                    && request
                        .reply()
                        .is_some_and(|reply| same_result_text(&reply.text, text))
            })
            .map(|request| (request.prompt.submitted_at_ms, request.id))
            .collect();
        claimants.sort_unstable();
        claimants
    }

    /// The worker's own earlier message already carries `text`. A task prompt
    /// is not that copy, even when the task and the reply use the same words.
    pub(super) fn result_already_in_room(&self, room: RoomId, worker: AgentId, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let matches = |stored: &str| same_result_text(stored, text);
        let from_worker = |author: &Author| author == &Author::Agent(worker);
        self.notices(room)
            .any(|prompt| from_worker(&prompt.author) && matches(&prompt.text))
            || self.requests().any(|request| {
                request.room_id == room
                    && from_worker(&request.prompt.author)
                    && matches(&request.prompt.text)
            })
    }

    /// The worker message that already carries `text`, when one is assigned.
    pub(super) fn explicit_copy_message(
        &self,
        room: RoomId,
        worker: AgentId,
        text: &str,
    ) -> Option<PromptId> {
        self.worker_copies(room, worker, text.trim(), None, None)
            .into_iter()
            .next()
            .map(|(_, id)| id)
    }
}

fn final_body(text: &str) -> &str {
    text.split_once('\n').map_or(text, |(_, body)| body).trim()
}

fn is_wrapper(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("No reply")
        || text.starts_with("Late reply")
        || text.starts_with("Turn final")
        || text.starts_with("Reply to message")
        || text.starts_with("Restarted ")
}

fn same_result_text(stored: &str, text: &str) -> bool {
    let stored = stored.trim();
    if stored == text {
        return true;
    }
    stored
        .strip_prefix("[Bus message ")
        .and_then(|rest| rest.split_once('\n'))
        .is_some_and(|(_, body)| body.trim() == text)
}
