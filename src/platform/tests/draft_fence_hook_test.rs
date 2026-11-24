//! Pause a specific fence immediately before its parent-directory sync.
use crate::platform::fs::AtomicWriteStage;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
    time::Duration,
};

type Pause = (PathBuf, mpsc::Sender<()>, mpsc::Receiver<()>);
static PAUSES: Mutex<Vec<Pause>> = Mutex::new(Vec::new());

pub(crate) fn stall(dir: PathBuf) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (reached, ready) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    PAUSES.lock().unwrap().push((dir, reached, release));
    (ready, resume)
}

pub(super) fn take(dir: &Path) -> Option<impl FnMut(AtomicWriteStage) -> io::Result<()>> {
    let (_, reached, release) = {
        let mut pauses = PAUSES.lock().unwrap();
        let index = pauses.iter().position(|pause| pause.0 == dir)?;
        pauses.swap_remove(index)
    };
    Some(move |stage| {
        if stage == AtomicWriteStage::ParentSync {
            reached.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(12)).unwrap();
        }
        Ok(())
    })
}
