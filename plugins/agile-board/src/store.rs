use crate::{
    error::{invalid, Error, Result},
    model::{render_sanitized_markdown, BoardState},
};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub fn resolve_data_dir(cli_dir: Option<PathBuf>) -> Result<PathBuf> {
    let raw = cli_dir
        .ok_or_else(|| invalid("missing data directory: pass --data-dir <ABSOLUTE_PATH>"))?;
    if !raw.is_absolute() {
        return Err(invalid(format!(
            "data directory must be an absolute path: '{}'",
            raw.display()
        )));
    }
    Ok(raw)
}

pub fn board_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("agile-board")
}

pub fn board_file(data_dir: &Path) -> PathBuf {
    board_dir(data_dir).join("board.json")
}

fn ensure_private_dir(p: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(p)?;
    Ok(())
}

struct DirLock {
    _file: File,
}

impl DirLock {
    fn acquire(dir: &Path) -> Result<Self> {
        ensure_private_dir(dir)?;
        let lock_path = dir.join("board.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(Error(4, "failed to acquire board lock".into()));
        }
        Ok(Self { _file: file })
    }
}

pub fn load(data_dir: &Path) -> Result<BoardState> {
    let p = board_file(data_dir);
    if !p.exists() {
        return Ok(BoardState::default());
    }
    let bytes = fs::read(&p)?;
    let mut state: BoardState = serde_json::from_slice(&bytes)
        .map_err(|e| Error(4, format!("corrupt board state at {}: {e}", p.display())))?;
    for t in &mut state.tickets {
        t.description_html = render_sanitized_markdown(&t.description);
    }
    Ok(state)
}

pub fn save_unlocked(data_dir: &Path, state: &BoardState) -> Result<PathBuf> {
    let dir = board_dir(data_dir);
    ensure_private_dir(&dir)?;
    let final_path = dir.join("board.json");
    let tmp_path = dir.join(format!("board.json.tmp.{}", std::process::id()));
    let mut bytes = serde_json::to_vec_pretty(state)?;
    bytes.push(b'\n');
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

pub fn mutate<F, T>(data_dir: &Path, f: F) -> Result<(T, BoardState)>
where
    F: FnOnce(&mut BoardState) -> Result<T>,
{
    let dir = board_dir(data_dir);
    let _lock = DirLock::acquire(&dir)?;
    let mut state = load(data_dir)?;
    let out = f(&mut state)?;
    save_unlocked(data_dir, &state)?;
    Ok((out, state))
}
