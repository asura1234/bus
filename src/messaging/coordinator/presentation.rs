use std::sync::atomic::{AtomicU64, Ordering};

static FORM_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn current_form_generation() -> u64 {
    FORM_GENERATION.load(Ordering::Relaxed)
}

pub(crate) fn publish_form_generation(generation: u64) {
    FORM_GENERATION.store(generation, Ordering::Relaxed);
}
