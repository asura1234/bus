use super::*;
use std::io;

type Action = Box<dyn FnOnce() -> io::Result<()> + Send>;
static WRITES: OnceLock<Mutex<HashMap<PathBuf, Action>>> = OnceLock::new();

type Effect = Box<dyn FnOnce() + Send>;
static EFFECTS: OnceLock<Mutex<HashMap<PathBuf, Effect>>> = OnceLock::new();

pub(super) fn install_effect(path: PathBuf, effect: impl FnOnce() + Send + 'static) {
    let mut effects = EFFECTS.get_or_init(Default::default).lock().unwrap();
    assert!(effects.insert(path, Box::new(effect)).is_none());
}

pub(super) fn before_effect(path: &Path) {
    let effect = EFFECTS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(path);
    if let Some(effect) = effect {
        effect();
    }
}

pub(crate) fn install(
    directory: PathBuf,
    action: impl FnOnce() -> io::Result<()> + Send + 'static,
) {
    let previous = WRITES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(directory, Box::new(action));
    assert!(previous.is_none());
}

pub(super) fn before_write(directory: &Path) -> io::Result<()> {
    let action = WRITES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(directory);
    action.map_or(Ok(()), |action| action())
}

pub(crate) fn retain(directory: &Path, text: &str) {
    let diagnostics = CallerDiagnostics::default();
    if let Some(attempt) = start(directory, text, &diagnostics) {
        attempt.send(Outcome::Retain {
            reason: "test_retain",
            error: String::new(),
        });
    }
    diagnostics.emit();
}

type ReadAction = Box<dyn FnMut() -> io::Result<()> + Send>;
static READS: OnceLock<Mutex<HashMap<PathBuf, ReadAction>>> = OnceLock::new();

pub(super) fn install_read(
    directory: PathBuf,
    action: impl FnMut() -> io::Result<()> + Send + 'static,
) {
    let old = READS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(directory, Box::new(action));
    assert!(old.is_none());
}

pub(super) fn before_read(directory: &Path) -> io::Result<()> {
    let mut reads = READS.get_or_init(Default::default).lock().unwrap();
    reads.get_mut(directory).map_or(Ok(()), |action| action())
}

pub(super) fn remove_read(directory: &Path) {
    READS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(directory);
}
