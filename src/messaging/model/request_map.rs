//! Live records retain their JSON map shape; derived indexes never become durable state.
use super::{history_index::HistoryIndex, Request, RequestId, RoomId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
    sync::{Arc, Mutex},
};

type Rooms = BTreeMap<RoomId, BTreeSet<RequestId>>;

#[derive(Debug, Default, Serialize)]
#[serde(transparent)]
pub(super) struct RequestMap {
    records: BTreeMap<RequestId, Request>,
    // Request IDs and room IDs are immutable across model transitions.
    #[serde(skip)]
    rooms: Arc<Rooms>,
    #[serde(skip)]
    groups: Mutex<GroupCache>,
}

#[derive(Debug, Default)]
struct GroupCache {
    index: Arc<HistoryIndex>,
    dirty: BTreeSet<RequestId>,
}

impl RequestMap {
    pub(super) fn insert(&mut self, id: RequestId, request: Request) -> Option<Request> {
        let previous = self.remove(&id);
        Arc::make_mut(&mut self.rooms)
            .entry(request.room_id)
            .or_default()
            .insert(id);
        let cache = self.groups.get_mut().unwrap_or_else(|e| e.into_inner());
        Arc::make_mut(&mut cache.index).insert(&request);
        self.records.insert(id, request);
        previous
    }

    pub(super) fn remove(&mut self, id: &RequestId) -> Option<Request> {
        let previous = self.records.remove(id)?;
        let rooms = Arc::make_mut(&mut self.rooms);
        if let Some(ids) = rooms.get_mut(&previous.room_id) {
            ids.remove(id);
            if ids.is_empty() {
                rooms.remove(&previous.room_id);
            }
        }
        let cache = self.groups.get_mut().unwrap_or_else(|e| e.into_inner());
        cache.dirty.remove(id);
        Arc::make_mut(&mut cache.index).remove(*id);
        Some(previous)
    }

    pub(super) fn get_mut(&mut self, id: &RequestId) -> Option<&mut Request> {
        self.groups
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .dirty
            .insert(*id);
        self.records.get_mut(id)
    }

    pub(super) fn values_mut(&mut self) -> impl Iterator<Item = &mut Request> {
        self.groups
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .dirty
            .extend(self.records.keys());
        self.records.values_mut()
    }

    pub(super) fn recent_room(&self, room: RoomId, before: u64) -> impl Iterator<Item = &Request> {
        self.rooms
            .get(&room)
            .into_iter()
            .flat_map(move |ids| ids.range(..RequestId(before)).rev())
            .filter_map(|id| self.records.get(id))
    }

    pub(super) fn history_index(&self) -> Arc<HistoryIndex> {
        let mut cache = self.groups.lock().unwrap_or_else(|e| e.into_inner());
        if !cache.dirty.is_empty() {
            let dirty = std::mem::take(&mut cache.dirty);
            let index = Arc::make_mut(&mut cache.index);
            for id in dirty {
                match self.records.get(&id) {
                    Some(request) => index.insert(request),
                    None => index.remove(id),
                }
            }
        }
        Arc::clone(&cache.index)
    }
}

impl Deref for RequestMap {
    type Target = BTreeMap<RequestId, Request>;
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

impl PartialEq for RequestMap {
    fn eq(&self, other: &Self) -> bool {
        self.records == other.records
    }
}
impl Eq for RequestMap {}

impl Clone for RequestMap {
    fn clone(&self) -> Self {
        // Snapshots publish complete metadata once; a page never rescans all requests.
        let index = self.history_index();
        Self {
            records: self.records.clone(),
            rooms: Arc::clone(&self.rooms),
            groups: Mutex::new(GroupCache {
                index,
                dirty: BTreeSet::new(),
            }),
        }
    }
}

impl<'de> Deserialize<'de> for RequestMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let records = BTreeMap::<RequestId, Request>::deserialize(deserializer)?;
        let mut map = Self::default();
        for (id, request) in records {
            map.insert(id, request);
        }
        Ok(map)
    }
}

#[cfg(test)]
#[path = "tests/request_map_test.rs"]
mod tests;
