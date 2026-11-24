//! Hold the callback's real exchange lock after it publishes a restore ACK.
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
    time::Duration,
};

type Pause = (PathBuf, mpsc::Sender<()>, mpsc::Receiver<()>);
static PAUSES: Mutex<Vec<Pause>> = Mutex::new(Vec::new());

pub(crate) fn hold_after_ack(dir: PathBuf) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (arrived, arrival) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    PAUSES.lock().unwrap().push((dir, arrived, release));
    (arrival, resume)
}

pub(super) fn after_ack(dir: &Path) {
    let pause = {
        let mut pauses = PAUSES.lock().unwrap();
        pauses
            .iter()
            .position(|pause| pause.0 == dir)
            .map(|index| pauses.swap_remove(index))
    };
    if let Some((_, arrived, release)) = pause {
        arrived.send(()).unwrap();
        release.recv_timeout(Duration::from_secs(12)).unwrap();
    }
}
