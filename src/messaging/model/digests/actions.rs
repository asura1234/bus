use super::{AgentId, AgentRecipients, Author, BusState, RoomFact, TOMBSTONE_RETENTION_MS};

impl BusState {
    /// Accepted, direct dispatches count as action; broadcasts and transport reads do not.
    pub(in crate::messaging::model) fn ack_digest_action(
        &mut self,
        author: &Author,
        recipients: &AgentRecipients,
        text: &str,
        broadcast: bool,
        now: u64,
    ) {
        if broadcast || recipients.len() != 1 {
            return;
        }
        let Author::Agent(orchestrator) = author else {
            return;
        };
        let Some(record) = self.room_digests.get_mut(orchestrator) else {
            return;
        };
        let keys: Vec<_> = record
            .facts
            .iter()
            .filter(|(_, fact)| {
                fact.agent.is_some_and(|id| recipients.contains(&id)) || references_fact(text, fact)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            record.acknowledge(&key, now);
        }
    }

    pub(in crate::messaging::model) fn ack_digest_deletion(&mut self, agent: AgentId, now: u64) {
        for record in self.room_digests.values_mut() {
            let keys: Vec<_> = record
                .facts
                .iter()
                .filter(|(_, f)| f.agent == Some(agent))
                .map(|(key, _)| key.clone())
                .collect();
            for key in keys {
                record.acknowledge(&key, now);
            }
        }
    }

    pub(crate) fn ack_digest_fact(&mut self, id: u64, now: u64) -> Result<(), String> {
        for record in self.room_digests.values_mut() {
            if let Some(key) = record
                .facts
                .iter()
                .find(|(_, f)| f.id == id)
                .map(|(key, _)| key.clone())
            {
                record.acknowledge(&key, now);
                return Ok(());
            }
            if record.tombstones.values().any(|f| f.id == id) {
                return Ok(());
            }
        }
        Err(format!("Unknown digest fact {id}"))
    }

    /// Reuse history housekeeping; expiry never arms a digest or adds a timer.
    pub(in crate::messaging::model) fn prune_digest_tombstones(&mut self, now: u64) -> bool {
        let mut changed = false;
        for record in self.room_digests.values_mut() {
            let before = record.tombstones.len();
            record.tombstones.retain(|_, t| {
                if now.saturating_sub(t.acknowledged_at_ms) >= TOMBSTONE_RETENTION_MS {
                    record.pruned_through = record.pruned_through.max(t.id);
                    false
                } else {
                    true
                }
            });
            changed |= before != record.tombstones.len();
        }
        changed
    }
}

pub(super) fn references_fact(text: &str, fact: &RoomFact) -> bool {
    let text = text.to_lowercase();
    if !fact.agent_name.is_empty() && contains_name(&text, &fact.agent_name.to_lowercase()) {
        return true;
    }
    let body = fact.text.to_lowercase();
    if super::status_tag(&body).is_none() {
        return false;
    }
    let first = body.lines().next().unwrap_or_default();
    let prs = |s: &str| -> Vec<String> {
        s.split('#')
            .skip(1)
            .filter_map(|part| {
                let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
                (!digits.is_empty()).then_some(digits)
            })
            .collect()
    };
    let references = prs(&text);
    if prs(first).iter().any(|number| references.contains(number)) {
        return true;
    }
    let matches = |head: &str, explicit: bool| {
        heads(&text, explicit)
            .iter()
            .any(|prefix| head.starts_with(prefix) || prefix.starts_with(head))
    };
    heads(first, false).iter().any(|head| matches(head, false))
        || body.lines().enumerate().any(|(index, line)| {
            // Outside the tag line, the entire value must be a standalone head/SHA field.
            if index != 0 {
                let Some((label, value)) = line
                    .split_once(':')
                    .or_else(|| line.trim().split_once(char::is_whitespace))
                else {
                    return false;
                };
                let value = value.trim();
                return matches!(label.trim(), "head" | "sha")
                    && heads(value, true)
                        .iter()
                        .any(|head| *head == value && matches(head, true));
            }
            let words: Vec<_> = line
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .collect();
            words.windows(2).any(|pair| {
                matches!(pair[0], "head" | "sha")
                    && heads(pair[1], true).iter().any(|head| matches(head, true))
            })
        })
}

fn contains_name(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-');
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(word)
            && !text[at + name.len()..].chars().next().is_some_and(word)
    })
}

fn heads(s: &str, explicit: bool) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|word| {
            (7..=40).contains(&word.len())
                && word.chars().all(|c| c.is_ascii_hexdigit())
                && (explicit || word.chars().any(|c| c.is_ascii_alphabetic()))
        })
        .collect()
}
