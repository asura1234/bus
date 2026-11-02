//! Assemble failed terminal records whose append order differs from native time.
use super::{guidance, normalized_prompt, owned_prompt, CommittedTurn, Outcome};
use serde_json::Value;

pub(super) struct Evidence {
    turn_id: u64,
    start: Option<Start>,
    end: Option<End>,
    completed: Option<u64>,
    next_start: Option<u64>,
    agents: Vec<AgentEnd>,
    guidance: Vec<(u64, String, Vec<String>)>,
    invalid: bool,
}

struct Start {
    time: u64,
    prompt_id: String,
    text: String,
}

struct End {
    time: u64,
    error: Value,
}

struct AgentEnd {
    time: u64,
    outcome: String,
    error: Option<String>,
}

impl Evidence {
    pub(super) fn new(turn_id: u64) -> Self {
        Self {
            turn_id,
            start: None,
            end: None,
            completed: None,
            next_start: None,
            agents: Vec::new(),
            guidance: Vec::new(),
            invalid: false,
        }
    }

    pub(super) fn observe(&mut self, value: &Value) {
        let kind = value.get("type").and_then(Value::as_str);
        if kind == Some("turn.prompt") && main(value, false) {
            if value.get("turnId").and_then(Value::as_u64) == Some(self.turn_id) {
                if let (Some(time), Some((prompt_id, text))) = (time(value), owned_prompt(value)) {
                    self.invalid |= self.start.is_some();
                    self.start = Some(Start {
                        time,
                        prompt_id,
                        text,
                    });
                } else {
                    self.invalid = true;
                }
            } else if let (Some(start), Some(time)) = (&self.start, time(value)) {
                if time >= start.time {
                    self.next_start = Some(self.next_start.map_or(time, |old| old.min(time)));
                }
            } else if self.start.is_some() {
                self.invalid = true;
            }
            return;
        }
        let Some(start) = &self.start else { return };
        match kind {
            Some("agent.turn.ended") if main(value, true) => {
                if let (Some(time), Some(outcome), Some(_)) = (
                    time(value),
                    value.get("outcome").and_then(Value::as_str),
                    value.get("turnId").and_then(Value::as_u64),
                ) {
                    self.agents.push(AgentEnd {
                        time,
                        outcome: outcome.into(),
                        error: value
                            .get("errorMessage")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
                } else {
                    self.invalid = true;
                }
            }
            Some("turn.ended")
                if main(value, false)
                    && value.get("turnId").and_then(Value::as_u64) == Some(self.turn_id) =>
            {
                if let Some(time) = time(value)
                    .filter(|_| value.get("reason").and_then(Value::as_str) == Some("failed"))
                {
                    self.invalid |= self.end.is_some();
                    self.end = Some(End {
                        time,
                        error: value.get("error").cloned().unwrap_or(Value::Null),
                    });
                } else {
                    self.invalid = true;
                }
            }
            Some("prompt.completed")
                if main(value, false)
                    && value.get("promptId").and_then(Value::as_str) == Some(&start.prompt_id) =>
            {
                if let Some(time) = time(value)
                    .filter(|_| value.get("reason").and_then(Value::as_str) == Some("failed"))
                {
                    self.invalid |= self.completed.is_some();
                    self.completed = Some(time);
                } else {
                    self.invalid = true;
                }
            }
            Some("prompt.steered") => {
                if let Some((id, texts)) = guidance(value, &start.prompt_id) {
                    if let Some(time) = time(value) {
                        self.guidance.push((time, id, texts));
                    } else {
                        self.invalid = true;
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn committed(
        self,
        prompt: &str,
        message: impl FnOnce(Option<&str>, &Value) -> String,
    ) -> Option<CommittedTurn> {
        if self.invalid {
            return None;
        }
        let start = self.start?;
        let end = self.end?;
        if end.time < start.time || self.completed? < end.time {
            return None;
        }
        // Internal agent counters are not outer turn IDs. Native times bind
        // the delayed agent end without stealing the next main turn's end.
        let eligible = |agent: &&AgentEnd| {
            agent.time >= start.time
                && agent.time <= end.time
                && self.next_start.is_none_or(|next| agent.time < next)
        };
        let agent = self
            .agents
            .iter()
            .filter(eligible)
            .max_by_key(|agent| agent.time);
        // #321 treats a structured owned root failure and prompt commit as
        // authoritative even before the inner event flushes. Keep that rule.
        let root_failure = end
            .error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.trim().is_empty());
        if !root_failure
            && (agent.is_none_or(|agent| agent.outcome != "failed")
                || self.agents.iter().filter(eligible).any(|other| {
                    agent.is_some_and(|agent| {
                        other.time == agent.time
                            && (other.outcome != agent.outcome || other.error != agent.error)
                    })
                }))
        {
            return None;
        }
        let mut prompts = vec![start.text];
        let mut ids = std::collections::HashSet::new();
        for (at, id, texts) in self.guidance {
            if at >= start.time && at <= end.time && ids.insert(id) {
                prompts.extend(texts);
            }
        }
        if !prompts
            .iter()
            .any(|text| normalized_prompt(text) == normalized_prompt(prompt))
        {
            return None;
        }
        Some(CommittedTurn {
            outcome: Outcome::Failed(message(
                agent
                    .filter(|agent| agent.outcome == "failed")
                    .and_then(|agent| agent.error.as_deref()),
                &end.error,
            )),
            prompts,
        })
    }
}

fn time(value: &Value) -> Option<u64> {
    value
        .get("time")
        .and_then(Value::as_u64)
        .filter(|time| *time > 0)
}

fn main(value: &Value, absent: bool) -> bool {
    match value.get("agentId") {
        None => absent,
        Some(Value::String(id)) => id == "main",
        _ => false,
    }
}
