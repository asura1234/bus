//! Clickable URLs and file paths in room history rows. Rows are scanned as
//! displayed, so a path inside inline code or a fenced block counts too. A
//! path is only a link once a background check has found it on disk; the UI
//! thread never waits on the file system.
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

/// Blue and underlined. Inline code is cyan without an underline, so a path
/// inside code still reads as clickable.
pub(super) const LINK: ratatui::style::Style = ratatui::style::Style::new()
    .fg(ratatui::style::Color::Rgb(90, 150, 255))
    .add_modifier(ratatui::style::Modifier::UNDERLINED);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Url(String),
    Path(PathBuf),
}

/// One link-shaped token of a row: its bytes (the underlined part) and what a
/// click opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Candidate {
    pub range: Range<usize>,
    pub target: Target,
}

/// Quotes, brackets and backticks around a token are never part of it.
fn delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\''
                | '`'
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '「'
                | '」'
                | '（'
                | '）'
                | '，'
                | '。'
                | '：'
                | '；'
        )
}

/// URL- and path-shaped tokens of `text`. Relative paths resolve against
/// `cwd`; without one (the human, Bus) only absolute and `~/` paths count.
pub(super) fn candidates(text: &str, cwd: Option<&Path>, home: Option<&Path>) -> Vec<Candidate> {
    let mut found = Vec::new();
    let mut start = None;
    for (index, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (start, delimiter(c)) {
            (None, false) => start = Some(index),
            (Some(from), true) => {
                start = None;
                if let Some(candidate) = candidate(text, from..index, cwd, home) {
                    found.push(candidate);
                }
            }
            _ => {}
        }
    }
    found
}

fn candidate(
    text: &str,
    mut range: Range<usize>,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Option<Candidate> {
    // Sentence punctuation after a link is not part of it.
    while text[range.clone()].ends_with(['.', ',', ';', ':', '!', '?']) {
        range.end -= 1;
    }
    let token = &text[range.clone()];
    if token.starts_with("http://") || token.starts_with("https://") {
        return (token.len() > "https://".len()).then(|| Candidate {
            target: Target::Url(token.to_owned()),
            range,
        });
    }
    if token.contains("://") {
        return None;
    }
    let path = strip_location(token);
    let target = if let Some(rest) = path.strip_prefix("~/") {
        home?.join(rest)
    } else if path.starts_with('/') || windows_absolute(path) {
        if path.len() < 2 {
            return None;
        }
        PathBuf::from(path)
    } else if relative_path_like(path) {
        cwd?.join(path)
    } else {
        return None;
    };
    Some(Candidate {
        range,
        target: Target::Path(target),
    })
}

/// `src/main.rs:12` and `src/main.rs:12:5` open `src/main.rs`.
fn strip_location(token: &str) -> &str {
    let mut path = token;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((head, tail))
                if !head.is_empty()
                    && !tail.is_empty()
                    && tail.bytes().all(|b| b.is_ascii_digit()) =>
            {
                path = head;
            }
            _ => break,
        }
    }
    path
}

fn windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// Words with a slash or a file extension. Plain words are never probed, so a
/// reply's prose costs no file-system checks.
fn relative_path_like(path: &str) -> bool {
    if !path.chars().any(char::is_alphabetic) || path.starts_with('-') {
        return false;
    }
    if path.contains('/') {
        return true;
    }
    path.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && (1..=10).contains(&extension.len())
            && extension.chars().all(|c| c.is_ascii_alphanumeric())
            && extension.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// Paths to check, and their answers.
type Worker = (Sender<PathBuf>, Receiver<(PathBuf, bool)>);

/// Remembers which paths exist. Unknown paths are checked on a worker thread,
/// and their rows show no link until the answer arrives.
pub(super) struct PathProbe {
    known: HashMap<PathBuf, Option<bool>>,
    worker: Option<Worker>,
    pub home: Option<PathBuf>,
}

impl Default for PathProbe {
    fn default() -> Self {
        Self {
            known: HashMap::new(),
            worker: None,
            home: std::env::home_dir(),
        }
    }
}

/// Bounds the cache; a long session forgets old answers and asks again.
const MAX_KNOWN: usize = 8192;

impl PathProbe {
    /// Whether `path` exists, or `None` while it is being checked.
    pub fn exists(&mut self, path: &Path) -> Option<bool> {
        if let Some(known) = self.known.get(path) {
            return *known;
        }
        if self.known.len() >= MAX_KNOWN {
            self.known.retain(|_, known| known.is_none());
        }
        let sent = self
            .worker()
            .is_some_and(|requests| requests.send(path.to_path_buf()).is_ok());
        let answer = (!sent).then_some(false);
        self.known.insert(path.to_path_buf(), answer);
        answer
    }

    fn worker(&mut self) -> Option<&Sender<PathBuf>> {
        if self.worker.is_none() {
            let (requests, inbox) = std::sync::mpsc::channel::<PathBuf>();
            let (outbox, results) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("bus-link-probe".into())
                .spawn(move || {
                    for path in inbox {
                        let exists = std::fs::metadata(&path).is_ok();
                        if outbox.send((path, exists)).is_err() {
                            break;
                        }
                    }
                })
                .ok()?;
            self.worker = Some((requests, results));
        }
        self.worker.as_ref().map(|(requests, _)| requests)
    }

    /// Records finished checks. True when one arrived, so links may have
    /// appeared and the room needs a repaint.
    pub fn poll(&mut self) -> bool {
        let Some((_, results)) = &self.worker else {
            return false;
        };
        let mut changed = false;
        for (path, exists) in results.try_iter() {
            self.known.insert(path, Some(exists));
            changed = true;
        }
        changed
    }

    /// Blocks until every pending check has an answer.
    #[cfg(test)]
    pub fn settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.known.values().any(Option::is_none) && std::time::Instant::now() < deadline {
            if let Some((_, results)) = &self.worker {
                if let Ok((path, exists)) =
                    results.recv_timeout(std::time::Duration::from_millis(50))
                {
                    self.known.insert(path, Some(exists));
                }
            }
        }
    }
}

/// The links of one displayed row: URLs, and paths known to exist. Bytes
/// before `copy_from` are layout indent and never link.
pub(super) fn row_links(
    probe: &mut PathProbe,
    text: &str,
    copy_from: usize,
    cwd: Option<&Path>,
) -> Vec<Candidate> {
    let home = probe.home.clone();
    let body = text.get(copy_from..).unwrap_or_default();
    candidates(body, cwd, home.as_deref())
        .into_iter()
        .filter(|candidate| match &candidate.target {
            Target::Url(_) => true,
            Target::Path(path) => probe.exists(path) == Some(true),
        })
        .map(|candidate| Candidate {
            range: candidate.range.start + copy_from..candidate.range.end + copy_from,
            ..candidate
        })
        .collect()
}

/// The links of each displayed history row, in order.
pub(super) fn visible_links<'a>(
    probe: &mut PathProbe,
    rows: impl Iterator<Item = &'a super::history::Line>,
) -> Vec<Vec<Candidate>> {
    rows.map(|line| {
        line.links.as_ref().map_or_else(Vec::new, |scope| {
            row_links(probe, &line.text, line.copy_from, scope.cwd.as_deref())
        })
    })
    .collect()
}

#[cfg(test)]
#[path = "tests/links_test.rs"]
mod tests;
