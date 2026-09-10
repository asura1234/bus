//! Observation-only provider hooks. Raw records survive client detach and restart.
use super::model::{AgentId, CallbackEventKind, Provider, ProviderCallback};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

const MAX_CALLBACK_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Manifest {
    pub(crate) agent_id: AgentId,
    pub(crate) provider: Provider,
    pub(crate) launch_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Record {
    pub(crate) id: String,
    pub(crate) sequence: u64,
    pub(crate) at_ms: u64,
    pub(crate) manifest: Manifest,
    pub(crate) value: Value,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Parsed {
    Session(String),
    Started {
        session: String,
        turn: String,
        prompt: String,
    },
    Final {
        session: String,
        turn: String,
        text: String,
    },
    CursorResponse {
        session: String,
        turn: String,
        text: String,
    },
    CursorStop {
        session: String,
        turn: String,
    },
    Failure {
        session: String,
        turn: String,
        message: String,
    },
    Ignore,
}

fn field(value: &Value, key: &str) -> Result<String, String> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned)
        .ok_or_else(|| format!("Hook missing {key}; update the CLI and verify Bus hooks. No prompt will be retried."))
}

#[cfg(test)]
#[test]
fn codex_background_sessions_without_transcripts_are_not_terminal_callbacks() {
    for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
        let value = serde_json::json!({"hook_event_name":event,"session_id":"background-title-or-memory","transcript_path":null,"turn_id":"background-turn","prompt":"Generate title","last_assistant_message":"Title"});
        assert_eq!(parse(Provider::Codex, &value).unwrap(), Parsed::Ignore);
    }
}

pub(crate) fn parse(provider: Provider, value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hook_event_name")?;
    // Codex's title/memory background sessions inherit hooks but have no
    // transcript. Only the persisted interactive session can own a room turn.
    // Live 0.153.4 evidence includes all three events from both session types.
    if provider == Provider::Codex
        && value
            .get("transcript_path")
            .and_then(Value::as_str)
            .is_none_or(|path| path.is_empty())
    {
        return Ok(Parsed::Ignore);
    }
    if value.get("agent_id").is_some_and(|v| !v.is_null()) {
        return Ok(Parsed::Ignore);
    }
    if event.starts_with("Subagent") {
        return Ok(Parsed::Ignore);
    }
    let cursor = provider == Provider::Cursor;
    let session = field(
        value,
        if cursor {
            "conversation_id"
        } else {
            "session_id"
        },
    )?;
    if (!cursor && event == "SessionStart") || (cursor && event == "sessionStart") {
        return Ok(Parsed::Session(session));
    }
    let turn = field(
        value,
        match provider {
            Provider::Codex => "turn_id",
            Provider::ClaudeCode => "prompt_id",
            Provider::Cursor => "generation_id",
        },
    )?;
    match event.as_str() {
        "UserPromptSubmit" if !cursor => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "beforeSubmitPrompt" if cursor => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "Stop" if !cursor => Ok(Parsed::Final {
            session,
            turn,
            text: field(value, "last_assistant_message")?,
        }),
        "StopFailure" if !cursor => Ok(Parsed::Failure {
            session,
            turn,
            message: "Provider turn failed; inspect its terminal. Request remains owned.".into(),
        }),
        "afterAgentResponse" if cursor => Ok(Parsed::CursorResponse {
            session,
            turn,
            text: field(value, "text")?,
        }),
        "stop" if cursor => match field(value, "status")?.as_str() {
            "completed" => Ok(Parsed::CursorStop { session, turn }),
            status => Ok(Parsed::Failure {
                session,
                turn,
                message: format!(
                    "Cursor turn {status}; inspect its terminal. Request remains owned."
                ),
            }),
        },
        _ => Err(format!("Unsupported Bus hook event {event}")),
    }
}

#[cfg(test)]
fn validate_provider_event(provider: Provider, value: &Value) -> Result<(), String> {
    parse(provider, value).map(|_| ())
}

pub(crate) fn initialize(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    super::io::private_dir(dir)?;
    super::io::atomic_write(&dir.join("manifest.json"), &serde_json::to_vec(manifest)?)
}

pub(crate) fn boundary(dir: &Path) -> io::Result<u64> {
    let _lease = super::io::append_lock(&dir.join("append.lock"))?;
    read_counter(dir)
}

fn read_counter(dir: &Path) -> io::Result<u64> {
    match std::fs::read_to_string(dir.join("sequence")) {
        Ok(value) => value.parse().map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

pub(crate) fn append(dir: &Path, launch: &str, provider: Provider, value: Value) -> io::Result<()> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
    if manifest.launch_id != launch || manifest.provider != provider {
        return Err(io::Error::other("Bus callback launch identity mismatch"));
    }
    let _lease = super::io::append_lock(&dir.join("append.lock"))?;
    let id = super::io::digest(&serde_json::to_vec(&(launch, &value))?);
    let path = dir.join(format!("event-{id}.json"));
    if path.exists() {
        return Ok(());
    }
    let sequence = read_counter(dir)?
        .checked_add(1)
        .ok_or_else(|| io::Error::other("Bus callback sequence overflow"))?;
    // Reserve durably before publishing. Gaps after a crash are harmless.
    super::io::atomic_write(&dir.join("sequence"), sequence.to_string().as_bytes())?;
    let record = Record {
        id,
        sequence,
        at_ms: super::io::now_ms(),
        manifest,
        value,
    };
    super::io::atomic_write(&path, &serde_json::to_vec(&record)?)
}

pub(crate) fn records(dir: &Path) -> io::Result<Vec<(PathBuf, Record)>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("event-")
            || entry.path().extension().is_none_or(|e| e != "json")
        {
            continue;
        }
        if !entry.file_type()?.is_file() || entry.metadata()?.len() > MAX_CALLBACK_BYTES + 4096 {
            return Err(io::Error::other("Invalid Bus callback file"));
        }
        result.push((
            entry.path(),
            serde_json::from_slice::<Record>(&std::fs::read(entry.path())?)?,
        ));
    }
    result.sort_by_key(|(_, record)| record.sequence);
    Ok(result)
}

impl Record {
    pub(crate) fn callback(
        &self,
        session: String,
        turn: String,
        prompt: Option<String>,
        kind: CallbackEventKind,
    ) -> ProviderCallback {
        ProviderCallback {
            callback_id: self.id.clone(),
            sequence: self.sequence,
            occurred_at_ms: self.at_ms,
            agent_id: self.manifest.agent_id,
            launch_id: self.manifest.launch_id.clone(),
            provider_session_id: Some(session),
            provider_turn_id: Some(turn.clone()),
            provider_prompt_id: (self.manifest.provider == Provider::ClaudeCode).then_some(turn),
            prompt_payload: prompt,
            kind,
        }
    }
}

/// Called before CLI parsing, server startup and inherited HERDR_ENV checks.
pub(crate) fn dispatch(args: &[String]) -> Option<io::Result<()>> {
    if args.get(1).map(String::as_str) != Some("--bus-callback") {
        return None;
    }
    let result = capture(args, io::stdin().lock());
    // Observation hooks must never inject context or approve a permission prompt.
    println!("{{}}");
    if let Err(error) = result {
        eprintln!("Bus callback: {error}");
    }
    Some(Ok(()))
}

fn capture(args: &[String], input: impl Read) -> io::Result<()> {
    let (Some(dir), Some(launch)) = (
        std::env::var_os("BUS_CALLBACK_DIR"),
        std::env::var_os("BUS_LAUNCH_ID"),
    ) else {
        return Ok(());
    };
    if args.len() != 3 {
        return Err(io::Error::other("Invalid Bus callback arguments"));
    }
    let provider = match args[2].as_str() {
        "codex-hook" => Provider::Codex,
        "claude-hook" => Provider::ClaudeCode,
        "cursor-hook" => Provider::Cursor,
        _ => return Err(io::Error::other("Unknown Bus callback provider")),
    };
    let mut bytes = Vec::new();
    input.take(MAX_CALLBACK_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CALLBACK_BYTES {
        return Err(io::Error::other("Bus callback exceeds 2 MiB"));
    }
    let value = serde_json::from_slice(&bytes)?;
    if matches!(parse(provider, &value), Ok(Parsed::Ignore)) {
        return Ok(());
    }
    append(Path::new(&dir), &launch.to_string_lossy(), provider, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_requires_real_turn_binding_and_rejects_notify() {
        assert!(validate_provider_event(
            Provider::Codex,
            &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"same","transcript_path":"/tmp/interactive.jsonl"})
        )
        .is_err());
        assert!(validate_provider_event(
            Provider::Codex,
            &json!({"type":"agent-turn-complete","turn-id":"old","input-messages":["same"]})
        )
        .is_err());
        assert_eq!(parse(Provider::Codex, &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","turn_id":"t","prompt":"same","transcript_path":"/tmp/interactive.jsonl"})).unwrap(), Parsed::Started {session:"s".into(),turn:"t".into(),prompt:"same".into()});
    }

    #[test]
    fn claude_requires_prompt_id_and_cursor_generation() {
        assert!(validate_provider_event(
            Provider::ClaudeCode,
            &json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"p"})
        )
        .is_err());
        assert!(validate_provider_event(
            Provider::Cursor,
            &json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"s","prompt":"p"})
        )
        .is_err());
    }

    #[test]
    fn concurrent_hook_writers_publish_all_records_and_duplicates_are_stable() {
        let dir = std::env::temp_dir().join(format!("bus-callback-{}", super::super::io::now_ns()));
        initialize(
            &dir,
            &Manifest {
                agent_id: AgentId(1),
                provider: Provider::Cursor,
                launch_id: "launch".into(),
            },
        )
        .unwrap();
        let writers: Vec<_> = (0..12).map(|i| {
            let dir = dir.clone();
            std::thread::spawn(move || append(&dir,"launch",Provider::Cursor,json!({"hook_event_name":"afterAgentResponse","conversation_id":"s","generation_id":format!("t{i}"),"text":"final"})).unwrap())
        }).collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let captured = records(&dir).unwrap();
        assert_eq!(captured.len(), 12);
        assert_eq!(boundary(&dir).unwrap(), 12);
        append(
            &dir,
            "launch",
            Provider::Cursor,
            captured[0].1.value.clone(),
        )
        .unwrap();
        assert_eq!(records(&dir).unwrap().len(), 12);
        assert!(append(&dir, "wrong-launch", Provider::Cursor, json!({})).is_err());
        assert_eq!(
            parse(
                Provider::Cursor,
                &json!({"hook_event_name":"sessionStart","conversation_id":"s"})
            )
            .unwrap(),
            Parsed::Session("s".into())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
