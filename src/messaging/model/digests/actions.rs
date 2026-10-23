use super::{AgentId, AgentRecipients, Author, BusState, RoomFact, RoomId, TOMBSTONE_RETENTION_MS};

impl BusState {
    /// Accepted, direct dispatches count as action; broadcasts and transport reads do not.
    pub(in crate::messaging::model) fn ack_digest_action(
        &mut self,
        room: RoomId,
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
                // Directly assigning the worker is explicit action even from MASTER.
                // A peer reference needs the fact's own conversation context.
                fact.agent.is_some_and(|id| recipients.contains(&id))
                    || (fact.room == room && references_fact(text, fact))
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
    let body = fact.text.to_lowercase();
    let tag = super::status_tag(&body);
    // A worker name alone cannot identify which PR is ready. Keep existing
    // name-based handling for other kinds of facts, outside readiness handoffs.
    if !matches!(tag, Some("READY FOR VERIFY" | "REVIEW READY"))
        && !fact.agent_name.is_empty()
        && contains_name(&text, &fact.agent_name.to_lowercase())
    {
        return true;
    }
    let Some(tag) = tag else {
        return false;
    };
    let prs = pr_references(&text);
    let heads = heads(&text);
    let first = body.lines().next().unwrap_or_default().trim_start();
    let first = &first[tag.len()..];
    pr_references(first)
        .first()
        .is_some_and(|number| prs.contains(number))
        || first_line_head_matches(first, &heads)
        || body.lines().skip(1).any(|line| {
            let Some((label, value)) = identity_field(line) else {
                return false;
            };
            match label {
                "pr" => value.strip_prefix('#').is_some_and(|number| {
                    !number.is_empty()
                        && number.chars().all(|c| c.is_ascii_digit())
                        && prs.contains(&number)
                }),
                "head" | "sha" => {
                    self_contained_head(value).is_some_and(|head| matching_head(head, &heads))
                }
                _ => false,
            }
        })
}

fn pr_references(text: &str) -> Vec<&str> {
    text.split('#')
        .skip(1)
        .filter_map(|part| {
            let end = part
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(part.len());
            let (digits, rest) = part.split_at(end);
            (!digits.is_empty()
                && !rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_'))
            .then_some(digits)
        })
        .collect()
}

fn first_line_head_matches(line: &str, references: &[&str]) -> bool {
    let words: Vec<_> = line
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(2).any(|pair| {
        matches!(pair[0], "head" | "sha")
            && self_contained_head(pair[1]).is_some_and(|head| matching_head(head, references))
    }) || line.split('@').skip(1).any(|suffix| {
        let head = suffix
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .next()
            .unwrap_or_default();
        self_contained_head(head).is_some_and(|head| matching_head(head, references))
    })
}

fn matching_head(head: &str, references: &[&str]) -> bool {
    references
        .iter()
        .any(|prefix| head.starts_with(prefix) || prefix.starts_with(head))
}

fn self_contained_head(value: &str) -> Option<&str> {
    heads(value).into_iter().find(|head| *head == value)
}

fn identity_field(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line);
    let separator = line.find(|c: char| c == ':' || c.is_whitespace())?;
    let (label, value) = line.split_at(separator);
    let value = value.trim_start();
    let value = value.strip_prefix(':').unwrap_or(value).trim_start();
    let label = label.trim();
    let label = label
        .strip_prefix("**")
        .and_then(|s| s.strip_suffix("**"))
        .unwrap_or(label);
    Some((label, identity_value(value.trim())))
}

fn identity_value(value: &str) -> &str {
    let value = value
        .strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .unwrap_or(value);
    value
        .strip_prefix('[')
        .and_then(|s| s.split_once("]("))
        .and_then(|(label, target)| target.strip_suffix(')').map(|target| (label, target)))
        .filter(|(_, target)| {
            !target.is_empty() && !target.contains(')') && !target.chars().any(char::is_whitespace)
        })
        .map_or(value, |(label, _)| label)
}

fn contains_name(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-');
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(word)
            && !text[at + name.len()..].chars().next().is_some_and(word)
    })
}

fn heads(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| {
            (7..=40).contains(&word.len()) && word.chars().all(|c| c.is_ascii_hexdigit())
        })
        .collect()
}
