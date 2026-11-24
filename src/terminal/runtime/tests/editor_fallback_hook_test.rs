//! Pause a single exchange at the optional editor fallback decision.
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
};
use tokio::sync::oneshot;

type Pause = (PathBuf, oneshot::Sender<()>, mpsc::Receiver<()>);
static PAUSES: Mutex<Vec<Pause>> = Mutex::new(Vec::new());

pub(crate) fn arm(directory: PathBuf) -> (oneshot::Receiver<()>, mpsc::Sender<()>) {
    let (arrived, arrival) = oneshot::channel();
    let (resume, resumed) = mpsc::channel();
    PAUSES.lock().unwrap().push((directory, arrived, resumed));
    (arrival, resume)
}

pub(super) fn pause(directory: &Path) {
    let pause = {
        let mut pauses = PAUSES.lock().unwrap();
        pauses
            .iter()
            .position(|pause| pause.0 == directory)
            .map(|index| pauses.swap_remove(index))
    };
    if let Some((_, arrived, resumed)) = pause {
        arrived.send(()).unwrap();
        resumed
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap();
    }
}
