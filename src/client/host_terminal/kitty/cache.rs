use crate::protocol::kitty::apc::{
    encode_delete_image, encode_delete_placement, encode_display_placement, encode_kitty_data,
    KITTY_CHUNK_BYTES,
};
use crate::protocol::kitty::placement::{
    clipped_placement, host_image_id, host_placement_id, image_signature, kitty_format_code,
    placement_signature, HostPlacement, HostSourceKey, ImageSignature, PlacementSignature,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
const MAX_OVERSIZED_SOURCES: usize = 256;

#[derive(Debug, Default, Clone)]
pub(crate) struct HostGraphicsCache {
    pub(crate) images: HashMap<u32, ImageSignature>,
    pub(crate) placements: HashMap<(u32, u32), PlacementSignature>,
    /// Host image currently backing each (pane, source image id) pair.
    pub(crate) sources: HashMap<HostSourceKey, u32>,
    pub(crate) oversized: HashMap<HostSourceKey, ImageSignature>,
    continuation: Option<(HostSourceKey, u32, usize)>,
    replay_placements: bool,
    pub(crate) replayed_placements: HashSet<(u32, u32)>,
}

static KITTY_GRAPHICS_ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_enabled(enabled: bool) {
    KITTY_GRAPHICS_ENABLED.store(enabled, Ordering::Release);
}

pub(crate) fn is_enabled() -> bool {
    KITTY_GRAPHICS_ENABLED.load(Ordering::Acquire)
}

pub(crate) struct EncodedGraphics {
    pub(crate) bytes: Vec<u8>,
    pub(crate) incomplete: bool,
}

/// Whether appending `additional` bytes to the `current_len` bytes already
/// assembled keeps the transaction inside the caller's budget. Without a
/// budget the incremental path intentionally stays one transaction per call.
fn coalesced_transaction_fits(
    current_len: usize,
    additional: usize,
    transaction_budget: Option<usize>,
) -> bool {
    let Some(budget) = transaction_budget else {
        return false;
    };
    current_len.saturating_add(additional) <= budget
}

fn image_transaction_fits(placement: &HostPlacement, budget: Option<usize>) -> bool {
    let Some(budget) = budget else {
        return true;
    };
    image_transfer_estimated_size(placement.placement.data_len) <= budget
}

pub(crate) fn image_transfer_estimated_size(data_len: usize) -> usize {
    let encoded = data_len.div_ceil(3).saturating_mul(4);
    let command_overhead = data_len.div_ceil(KITTY_CHUNK_BYTES).saturating_mul(16) + 1024;
    encoded.saturating_add(command_overhead)
}

fn placement_identity(placement: &HostPlacement) -> (HostSourceKey, u32) {
    (
        placement.source_key.clone(),
        host_placement_id(&placement.source_key, &placement.placement),
    )
}

fn encode_placement_update(
    cache: &mut HostGraphicsCache,
    placement: &HostPlacement,
) -> Option<Vec<u8>> {
    let (clipped, format_code) = clipped_placement(placement)?;
    let host_id = placement
        .host_image_id
        .unwrap_or_else(|| host_image_id(placement.pane_id, &placement.placement));
    let placement_id = host_placement_id(&placement.source_key, &placement.placement);
    let key = (host_id, placement_id);
    let image_signature = image_signature(placement, format_code);
    let placement_signature =
        placement_signature(clipped, placement.placement.z, placement.scrollback_offset);
    let image_current = cache.images.get(&host_id) == Some(&image_signature);
    let placement_current = cache.placements.get(&key) == Some(&placement_signature)
        && (!cache.replay_placements || cache.replayed_placements.contains(&key));
    if image_current
        && placement_current
        && cache.sources.get(&placement.source_key) == Some(&host_id)
    {
        return None;
    }

    let mut bytes = Vec::new();
    if !image_current {
        if cache.images.contains_key(&host_id) {
            encode_delete_image(&mut bytes, host_id);
            cache.placements.retain(|(id, _), _| *id != host_id);
            cache.replayed_placements.retain(|(id, _)| *id != host_id);
        }
        if !encode_upload_image(&mut bytes, placement, format_code, host_id) {
            return None;
        }
        cache.images.insert(host_id, image_signature);
    }

    release_superseded_source_image(&mut bytes, cache, placement.source_key.clone(), host_id);
    if !placement_current {
        encode_display_placement(
            &mut bytes,
            clipped,
            host_id,
            placement_id,
            placement.placement.z,
        );
    }
    cache.placements.insert(key, placement_signature);
    if cache.replay_placements {
        cache.replayed_placements.insert(key);
    }
    Some(bytes)
}

fn release_superseded_source_image(
    bytes: &mut Vec<u8>,
    cache: &mut HostGraphicsCache,
    source: HostSourceKey,
    host_id: u32,
) {
    let Some(previous) = cache.sources.insert(source, host_id) else {
        return;
    };
    if previous == host_id || cache.sources.values().any(|id| *id == previous) {
        return;
    }
    encode_delete_image(bytes, previous);
    cache.images.remove(&previous);
    cache.placements.retain(|(id, _), _| *id != previous);
    cache.replayed_placements.retain(|(id, _)| *id != previous);
}

pub(crate) fn encode_graphics_update_incremental(
    cache: &mut HostGraphicsCache,
    placements: &[HostPlacement],
    transaction_budget: Option<usize>,
    coalesce_placements: bool,
) -> EncodedGraphics {
    let desired_sources = placements
        .iter()
        .map(|placement| placement.source_key.clone())
        .collect::<HashSet<_>>();
    let desired_placements = placements
        .iter()
        .filter_map(|placement| {
            clipped_placement(placement).map(|_| {
                let host_id = placement
                    .host_image_id
                    .unwrap_or_else(|| host_image_id(placement.pane_id, &placement.placement));
                (
                    host_id,
                    host_placement_id(&placement.source_key, &placement.placement),
                )
            })
        })
        .collect::<HashSet<_>>();
    let start = cache
        .continuation
        .as_ref()
        .and_then(|(source, id, _)| {
            placements
                .iter()
                .position(|placement| placement_identity(placement) == (source.clone(), *id))
        })
        .map(|index| index + 1)
        .or_else(|| cache.continuation.as_ref().map(|cursor| cursor.2))
        .map_or(0, |index| index % placements.len().max(1));
    let mut bytes = Vec::new();
    let mut emitted = false;

    cache
        .sources
        .retain(|source, _| desired_sources.contains(source));
    cache.oversized.retain(|source, _| {
        matches!(source, HostSourceKey::Terminal { .. }) || desired_sources.contains(source)
    });

    let mut stale = cache
        .placements
        .keys()
        .filter(|key| !desired_placements.contains(key))
        .copied()
        .collect::<Vec<_>>();
    stale.sort_unstable();
    let mut stale_image = None;
    for key @ (host_id, placement_id) in stale {
        let mut transaction = Vec::new();
        encode_delete_placement(&mut transaction, host_id, placement_id);
        let same_image = stale_image == Some(host_id);
        if emitted
            && !(coalesce_placements
                && same_image
                && coalesced_transaction_fits(bytes.len(), transaction.len(), transaction_budget))
        {
            return EncodedGraphics {
                bytes,
                incomplete: true,
            };
        }
        bytes.extend(transaction);
        cache.placements.remove(&key);
        cache.replayed_placements.remove(&key);
        emitted = true;
        stale_image = Some(host_id);
    }

    // Keep unrelated images isolated, but treat every row of one logical image
    // as part of its upload or replacement transaction. Sending only the first
    // row exposes the blank placeholder cells until later frames catch up.
    let coalesce_pass = coalesce_placements && !emitted;
    let mut coalesce_target = None;
    for offset in 0..placements.len() {
        let index = (start + offset) % placements.len();
        let placement = &placements[index];
        let signature = image_signature(placement, kitty_format_code(placement.placement.format));
        if transaction_budget.is_some()
            && cache.oversized.get(&placement.source_key) == Some(&signature)
        {
            continue;
        }
        cache.oversized.remove(&placement.source_key);
        let host_id = placement
            .host_image_id
            .unwrap_or_else(|| host_image_id(placement.pane_id, &placement.placement));
        let image_cached = cache.images.get(&host_id) == Some(&signature);
        // With the image uploaded and the source already bound to it, the
        // transaction is a re-display only: no upload and no superseded-image
        // delete from `release_superseded_source_image`.
        let pure_redisplay =
            image_cached && cache.sources.get(&placement.source_key) == Some(&host_id);
        if !image_cached && !image_transaction_fits(placement, transaction_budget) {
            cache.quarantine_oversized(placement.source_key.clone(), signature);
            continue;
        }
        let mut candidate = cache.clone();
        let Some(transaction) = encode_placement_update(&mut candidate, placement) else {
            continue;
        };
        if transaction.is_empty() {
            *cache = candidate;
            continue;
        }
        let same_logical_image = coalesce_target.as_ref().is_none_or(|(source, target_id)| {
            source == &placement.source_key && *target_id == host_id
        });
        if emitted
            && !(coalesce_pass
                && pure_redisplay
                && same_logical_image
                && coalesced_transaction_fits(bytes.len(), transaction.len(), transaction_budget))
        {
            return EncodedGraphics {
                bytes,
                incomplete: true,
            };
        }
        *cache = candidate;
        let (source, id) = placement_identity(placement);
        cache.continuation = Some((source, id, (index + 1) % placements.len()));
        bytes.extend(transaction);
        emitted = true;
        if coalesce_pass && !pure_redisplay {
            coalesce_target = Some((placement.source_key.clone(), host_id));
        }
    }

    cache.replay_placements = false;
    cache.replayed_placements.clear();
    EncodedGraphics {
        bytes,
        incomplete: false,
    }
}

#[cfg(test)]
pub(crate) fn drain_graphics_updates(
    cache: &mut HostGraphicsCache,
    placements: &[HostPlacement],
) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let encoded = encode_graphics_update_incremental(cache, placements, None, false);
        bytes.extend(encoded.bytes);
        if !encoded.incomplete {
            return bytes;
        }
    }
}

impl HostGraphicsCache {
    fn reset_incremental_progress(&mut self) {
        self.continuation = None;
        self.replay_placements = false;
        self.replayed_placements.clear();
    }

    fn quarantine_oversized(&mut self, source: HostSourceKey, signature: ImageSignature) {
        if !self.oversized.contains_key(&source) && self.oversized.len() >= MAX_OVERSIZED_SOURCES {
            if let Some(evicted) = self.oversized.keys().next().cloned() {
                self.oversized.remove(&evicted);
            }
        }
        self.oversized.insert(source, signature);
    }

    pub(crate) fn request_placement_replay(&mut self) {
        if !self.replay_placements {
            self.replay_placements = true;
            self.replayed_placements.clear();
        }
    }

    pub(crate) fn clear_bytes(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for id in self.images.keys().copied().collect::<Vec<_>>() {
            encode_delete_image(&mut bytes, id);
        }
        self.images.clear();
        self.placements.clear();
        self.sources.clear();
        self.oversized.clear();
        self.reset_incremental_progress();
        bytes
    }
}

fn encode_upload_image(
    out: &mut Vec<u8>,
    placement: &HostPlacement,
    format_code: u32,
    host_id: u32,
) -> bool {
    if placement.placement.data.is_empty() {
        return false;
    }

    let control = format!(
        "a=t,t=d,f={format_code},s={},v={},i={host_id},q=2",
        placement.placement.image_width, placement.placement.image_height,
    );
    encode_kitty_data(out, &control, &placement.placement.data);
    true
}
