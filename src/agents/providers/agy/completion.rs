//! Deterministic completion reader over AGY's `transcript_full.jsonl`.
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const TAIL_FAST_PATH_BYTES: u64 = 8 * 1024 * 1024;
const SOURCE_LOOKUP_TAIL_BYTES: u64 = 512 * 1024;
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TurnResolution {
    Pending,
    Final {
        turn_id: String,
        text: String,
        prompts: Vec<String>,
    },
}

/// Prefer `transcript_full.jsonl` when `transcriptPath` points to compact `transcript.jsonl`.
pub(crate) fn resolve_transcript_path(raw: &str, session: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(raw);
    if !candidate.is_absolute() {
        return Err("AGY transcriptPath must be an absolute path".into());
    }
    if !candidate
        .components()
        .any(|component| component.as_os_str().to_str() == Some(session))
    {
        return Err("AGY transcriptPath does not match callback conversationId".into());
    }
    if candidate.file_name().and_then(|n| n.to_str()) == Some("transcript.jsonl") {
        let full = candidate.with_file_name("transcript_full.jsonl");
        if full.is_file() {
            return Ok(full);
        }
    }
    Ok(candidate.to_path_buf())
}

/// Strip AGY's `<USER_REQUEST>\n...\n</USER_REQUEST>` envelope around raw prompt text.
fn unwrap_user_request(content: &str) -> String {
    if let Some(after_open) = content.strip_prefix("<USER_REQUEST>") {
        let after_open = after_open
            .strip_prefix("\r\n")
            .or_else(|| after_open.strip_prefix('\n'))
            .unwrap_or(after_open);
        if let Some((body, _tail)) = after_open.split_once("</USER_REQUEST>") {
            let body = body
                .strip_suffix("\r\n")
                .or_else(|| body.strip_suffix('\n'))
                .unwrap_or(body);
            return body.to_owned();
        }
    }
    content.to_owned()
}

fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    buf.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(buf.len());
        }
        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let take = newline_pos + 1;
            if buf.len().saturating_add(take) > MAX_LINE_BYTES {
                return Err(std::io::Error::other("AGY transcript line exceeds 16 MiB"));
            }
            buf.extend_from_slice(&available[..take]);
            reader.consume(take);
            return Ok(buf.len());
        }
        let len = available.len();
        if buf.len().saturating_add(len) > MAX_LINE_BYTES {
            return Err(std::io::Error::other("AGY transcript line exceeds 16 MiB"));
        }
        buf.extend_from_slice(available);
        reader.consume(len);
    }
}

/// Return the `source` of the first recorded step (`step_index == 0`), if readable.
pub(in crate::agents::providers) fn first_step_source(transcript_path: &Path) -> Option<String> {
    let file = File::open(transcript_path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    for _ in 0..4 {
        let n = read_bounded_line(&mut reader, &mut buf).ok()?;
        if n == 0 {
            return None;
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            return None;
        };
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(line).ok()?;
        return row.get("source").and_then(Value::as_str).map(str::to_owned);
    }
    None
}

/// Return the `source` field of `target_step` (e.g. `"USER_EXPLICIT"` vs `"SYSTEM"`).
pub(in crate::agents::providers) fn step_source_at(
    transcript_path: &Path,
    target_step: u64,
) -> Option<String> {
    let mut file = File::open(transcript_path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > SOURCE_LOOKUP_TAIL_BYTES {
        file.seek(SeekFrom::Start(len - SOURCE_LOOKUP_TAIL_BYTES))
            .ok()?;
        let mut reader = BufReader::new(&mut file);
        let mut discard = Vec::new();
        let _ = read_bounded_line(&mut reader, &mut discard);
        if let Some(source) = find_step_source(&mut reader, target_step) {
            return Some(source);
        }
        file.seek(SeekFrom::Start(0)).ok()?;
    }
    let mut reader = BufReader::new(file);
    find_step_source(&mut reader, target_step)
}

fn find_step_source(reader: &mut impl BufRead, target_step: u64) -> Option<String> {
    let mut buf = Vec::new();
    loop {
        let n = read_bounded_line(reader, &mut buf).ok()?;
        if n == 0 {
            return None;
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if row.get("step_index").and_then(Value::as_u64) == Some(target_step) {
            return row.get("source").and_then(Value::as_str).map(str::to_owned);
        }
    }
}

pub(crate) fn read_turn_completion(
    transcript_path: &Path,
    target_turn: &str,
) -> Result<TurnResolution, String> {
    let target_step: u64 = target_turn
        .parse()
        .map_err(|_| format!("Invalid AGY numeric turn id {target_turn:?}"))?;
    let metadata = match std::fs::metadata(transcript_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TurnResolution::Pending)
        }
        Err(error) => return Err(format!("Cannot stat AGY transcript: {error}")),
    };
    if !metadata.is_file() {
        return Err("AGY transcript path is not a regular file".into());
    }
    let mut file =
        File::open(transcript_path).map_err(|e| format!("Cannot open AGY transcript: {e}"))?;
    if metadata.len() > TAIL_FAST_PATH_BYTES {
        file.seek(SeekFrom::Start(metadata.len() - TAIL_FAST_PATH_BYTES))
            .map_err(|e| format!("Cannot seek AGY transcript: {e}"))?;
        let mut tail_reader = BufReader::new(&mut file);
        let mut partial_prefix = Vec::new();
        let _ = read_bounded_line(&mut tail_reader, &mut partial_prefix);
        let (found_target, resolution) =
            scan_byte_reader(&mut tail_reader, target_step, target_turn)?;
        if found_target {
            return Ok(resolution);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("Cannot rewind AGY transcript: {e}"))?;
    }
    let mut reader = BufReader::new(file);
    let (_found, resolution) = scan_byte_reader(&mut reader, target_step, target_turn)?;
    Ok(resolution)
}

/// Find the latest unconsumed `USER_EXPLICIT`/`USER_INPUT` step whose unwrapped
/// prompt matches `matches`. Used to reconcile an AGY request whose native
/// paste+Enter succeeded when no `PreInvocation(invocationNum == 0)` bound it.
pub(crate) fn find_turn_for_prompt(
    transcript_path: &Path,
    matches: impl Fn(&str) -> bool,
    is_consumed: impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    let metadata = match std::fs::metadata(transcript_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot stat AGY transcript: {error}")),
    };
    if !metadata.is_file() {
        return Err("AGY transcript path is not a regular file".into());
    }
    let mut file =
        File::open(transcript_path).map_err(|e| format!("Cannot open AGY transcript: {e}"))?;
    if metadata.len() > TAIL_FAST_PATH_BYTES {
        file.seek(SeekFrom::Start(metadata.len() - TAIL_FAST_PATH_BYTES))
            .map_err(|e| format!("Cannot seek AGY transcript: {e}"))?;
        let mut tail_reader = BufReader::new(&mut file);
        let mut partial_prefix = Vec::new();
        let _ = read_bounded_line(&mut tail_reader, &mut partial_prefix);
        if let Some(found) = scan_prompt_turn(&mut tail_reader, &matches, &is_consumed)? {
            return Ok(Some(found));
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("Cannot rewind AGY transcript: {e}"))?;
    }
    let mut reader = BufReader::new(file);
    scan_prompt_turn(&mut reader, &matches, &is_consumed)
}

fn scan_prompt_turn(
    reader: &mut impl BufRead,
    matches: &impl Fn(&str) -> bool,
    is_consumed: &impl Fn(&str) -> bool,
) -> Result<Option<String>, String> {
    let mut buf = Vec::new();
    let mut found = None;
    loop {
        let n = read_bounded_line(reader, &mut buf)
            .map_err(|e| format!("Cannot read AGY transcript bytes: {e}"))?;
        if n == 0 {
            break;
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let source = row
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = row.get("type").and_then(Value::as_str).unwrap_or_default();
        if source != "USER_EXPLICIT" || kind != "USER_INPUT" {
            continue;
        }
        let Some(step_index) = row.get("step_index").and_then(Value::as_u64) else {
            continue;
        };
        let turn_id = step_index.to_string();
        if is_consumed(&turn_id) {
            continue;
        }
        let raw = row
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let unwrapped = unwrap_user_request(raw);
        if matches(&unwrapped) {
            found = Some(turn_id);
        }
    }
    Ok(found)
}

fn scan_byte_reader(
    reader: &mut impl BufRead,
    target_step: u64,
    target_turn: &str,
) -> Result<(bool, TurnResolution), String> {
    let mut state = ScanState {
        target_step,
        ..ScanState::default()
    };
    let mut buf = Vec::new();
    loop {
        let n = read_bounded_line(reader, &mut buf)
            .map_err(|e| format!("Cannot read AGY transcript bytes: {e}"))?;
        if n == 0 {
            break;
        }
        let has_newline = buf.last() == Some(&b'\n');
        let Ok(line) = std::str::from_utf8(&buf) else {
            let next_available = reader.fill_buf().map(|b| !b.is_empty()).unwrap_or(false);
            if !has_newline && !next_available {
                return Ok((state.in_turn, TurnResolution::Pending));
            }
            return Err("AGY transcript contains invalid UTF-8 in a completed record".into());
        };
        if state.feed_line(line)? == LineOutcome::Break {
            break;
        }
    }
    Ok((state.in_turn, state.finish(target_turn)))
}

#[cfg(test)]
pub(in crate::agents::providers) fn scan_lines<I, E>(
    lines: I,
    target_step: u64,
    target_turn: &str,
) -> Result<TurnResolution, String>
where
    I: IntoIterator<Item = Result<String, E>>,
    E: std::fmt::Display,
{
    let mut state = ScanState {
        target_step,
        ..ScanState::default()
    };
    for line in lines {
        let line = line.map_err(|e| format!("Cannot read AGY transcript line: {e}"))?;
        if state.feed_line(&line)? == LineOutcome::Break {
            break;
        }
    }
    Ok(state.finish(target_turn))
}

#[derive(Default)]
struct ScanState {
    target_step: u64,
    in_turn: bool,
    prompts: Vec<String>,
    latest_terminal_reply: Option<String>,
    terminal_response_seen: bool,
    pending_tail: bool,
}

#[derive(Eq, PartialEq)]
enum LineOutcome {
    Continue,
    Break,
}

impl ScanState {
    fn feed_line(&mut self, line: &str) -> Result<LineOutcome, String> {
        if line.trim().is_empty() {
            return Ok(LineOutcome::Continue);
        }
        // A torn trailing JSONL line while AGY flushes stays Pending for retry.
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            self.pending_tail = true;
            return Ok(LineOutcome::Break);
        };
        let Some(step_index) = row.get("step_index").and_then(Value::as_u64) else {
            return Ok(LineOutcome::Continue);
        };
        let source = row
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = row.get("type").and_then(Value::as_str).unwrap_or_default();
        let status = row
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if source == "USER_EXPLICIT" && kind == "USER_INPUT" {
            if step_index == self.target_step {
                self.in_turn = true;
                let raw = row
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.prompts.push(unwrap_user_request(raw));
                return Ok(LineOutcome::Continue);
            }
            if self.in_turn && step_index > self.target_step {
                if self.terminal_response_seen {
                    return Ok(LineOutcome::Break);
                }
                let raw = row
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.prompts.push(unwrap_user_request(raw));
            }
            return Ok(LineOutcome::Continue);
        }

        if !self.in_turn {
            return Ok(LineOutcome::Continue);
        }

        if source == "MODEL" && kind == "PLANNER_RESPONSE" {
            if status != "DONE" {
                self.pending_tail = true;
                return Ok(LineOutcome::Break);
            }
            let has_tool_calls = row
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty());
            if has_tool_calls {
                self.terminal_response_seen = false;
            } else {
                let content = row
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.latest_terminal_reply = Some(content);
                self.terminal_response_seen = true;
            }
        } else if self.terminal_response_seen {
            self.terminal_response_seen = false;
        }
        Ok(LineOutcome::Continue)
    }

    fn finish(self, target_turn: &str) -> TurnResolution {
        if self.pending_tail || !self.in_turn || !self.terminal_response_seen {
            return TurnResolution::Pending;
        }
        match self.latest_terminal_reply {
            Some(text) => TurnResolution::Final {
                turn_id: target_turn.to_owned(),
                text,
                prompts: self.prompts,
            },
            None => TurnResolution::Pending,
        }
    }
}
