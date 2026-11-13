mod process;

pub(crate) use process::Cancellation;
use process::Process;
use std::path::{Path, PathBuf};

#[cfg(all(test, unix))]
#[path = "tests/bridge_test.rs"]
mod bridge_tests;

#[derive(Clone, Default)]
pub(crate) struct Client {
    process: Process,
    #[cfg(test)]
    search_path: Option<std::ffi::OsString>,
}

#[derive(Debug, serde::Deserialize, Eq, PartialEq)]
pub(crate) struct SentMessage {
    pub(crate) message_id: String,
    pub(crate) chat_id: String,
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

pub(crate) fn validate_recipient(recipient: &str) -> Result<(), String> {
    if valid_id(recipient, "ou_") {
        Ok(())
    } else {
        Err("Feishu recipient must be a user open_id (ou_...)".into())
    }
}

impl Client {
    /// Resolve once before admitting a send so PATH or settings changes cannot
    /// switch the executable between outbox persistence and process launch.
    pub(crate) fn program(&self, configured: Option<&str>) -> Option<PathBuf> {
        if let Some(program) = configured.filter(|program| !program.is_empty()) {
            let path = Path::new(program);
            return path.is_absolute().then(|| executable(path)).flatten();
        }
        #[cfg(test)]
        let path = self.search_path.clone()?;
        #[cfg(not(test))]
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .find_map(|directory| executable(&directory.join("feishu-bridge")))
    }

    pub(crate) fn send(
        &self,
        recipient: &str,
        program: Option<&str>,
        text: &str,
        key: &str,
        cancel: &Cancellation,
    ) -> Result<SentMessage, String> {
        validate_recipient(recipient)?;
        if key.is_empty() || key.len() > 50 || !key.is_ascii() {
            return Err("Feishu idempotency key must be 1-50 ASCII characters".into());
        }
        let program = self
            .program(program)
            .ok_or("feishu-bridge is not installed or not on PATH")?;
        let mut process = self.process.clone();
        process.set_program(program.into_os_string());
        let value = process.run(
            &[
                "send",
                "--open-id",
                recipient,
                "--text",
                text,
                "--idempotency-key",
                key,
            ],
            "",
            cancel,
        )?;
        let sent: SentMessage = serde_json::from_value(value)
            .map_err(|_| "feishu-bridge returned invalid message identifiers")?;
        if !valid_id(&sent.message_id, "om_") || !valid_id(&sent.chat_id, "oc_") {
            return Err("feishu-bridge returned invalid message identifiers".into());
        }
        Ok(sent)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn on_path(path: &std::path::Path, timeout: std::time::Duration) -> Self {
        let mut process = Process::at("feishu-bridge", timeout);
        process.path = Some(format!("{}:/usr/bin:/bin", path.display()).into());
        Self {
            process,
            search_path: Some(path.as_os_str().to_owned()),
        }
    }
}

fn executable(path: &Path) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    std::fs::canonicalize(path).ok()
}
