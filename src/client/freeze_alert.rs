//! Recovered alerts share the host output owner's ordered effect lane.
use super::host_terminal::output;
use crate::utils::perf::recovery;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static AWAITING_ACK: AtomicBool = AtomicBool::new(false);

pub(super) fn flush_pending() -> io::Result<()> {
    if !AWAITING_ACK.load(Ordering::Acquire) && !recovery::has_pending() {
        return Ok(());
    }
    // The owner must have completed its write/flush before notification delivery
    // and cooldown acknowledgement; never stack alerts behind blocked output.
    if output::busy() {
        return Ok(());
    }
    if AWAITING_ACK.load(Ordering::Acquire) {
        if !recovery::acknowledged() {
            return Ok(());
        }
        AWAITING_ACK.store(false, Ordering::Release);
    }
    let Some(bytes) = recovery::take() else {
        return Ok(());
    };
    let mut writer = output::stdout();
    writer.write_all(&bytes)?;
    writer.flush()?;
    AWAITING_ACK.store(true, Ordering::Release);
    Ok(())
}
