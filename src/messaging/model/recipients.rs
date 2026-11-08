//! Recipient set kept in the order the sender selected.
use super::types::AgentId;
use serde::{Deserialize, Serialize};

/// Unique recipients in the order the sender selected them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct AgentRecipients(Vec<AgentId>);

impl AgentRecipients {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &AgentId> {
        self.0.iter()
    }

    pub(crate) fn contains(&self, id: &AgentId) -> bool {
        self.0.contains(id)
    }

    pub(crate) fn insert(&mut self, id: AgentId) -> bool {
        if self.contains(&id) {
            false
        } else {
            self.0.push(id);
            true
        }
    }

    pub(crate) fn remove(&mut self, id: &AgentId) -> bool {
        let Some(index) = self.0.iter().position(|candidate| candidate == id) else {
            return false;
        };
        self.0.remove(index);
        true
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&AgentId) -> bool) {
        self.0.retain(|id| keep(id));
    }
}

impl<'de> Deserialize<'de> for AgentRecipients {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Vec::<AgentId>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

impl FromIterator<AgentId> for AgentRecipients {
    fn from_iter<T: IntoIterator<Item = AgentId>>(iter: T) -> Self {
        let mut recipients = Self::default();
        for id in iter {
            recipients.insert(id);
        }
        recipients
    }
}

impl<const N: usize> From<[AgentId; N]> for AgentRecipients {
    fn from(ids: [AgentId; N]) -> Self {
        ids.into_iter().collect()
    }
}

impl<'a> IntoIterator for &'a AgentRecipients {
    type Item = &'a AgentId;
    type IntoIter = std::slice::Iter<'a, AgentId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for AgentRecipients {
    type Item = AgentId;
    type IntoIter = std::vec::IntoIter<AgentId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
