mod auth;
mod process;

use crate::messaging::model::AttentionOrigin;
pub(crate) use process::Cancellation;
use process::Process;
use sha2::{Digest, Sha256};

#[derive(Clone, Default)]
pub(crate) struct Client {
    process: Process,
}

#[derive(Clone)]
pub(crate) struct Authenticated {
    process: Process,
    identity: auth::SelfIdentity,
    cancel: Cancellation,
}

#[derive(Clone, Debug)]
pub(crate) struct SendFailure {
    pub(crate) message: String,
    pub(crate) disable: bool,
}

#[derive(Clone)]
pub(crate) struct Message {
    pub(crate) source: String,
    pub(crate) signature: String,
    body: String,
    key: String,
}

impl Message {
    pub(crate) fn new(
        machine: &str,
        session: &str,
        source: String,
        origin: &AttentionOrigin,
        text: &str,
    ) -> Self {
        let text = crate::messaging::attention::text(text)
            .expect("Feishu source must have an attention tag");
        let body = format!("[Bus] Developer attention\nMachine: {machine}\nSession: {session}\nRoom: {} ({})\nAgent: {} ({})\n\n{text}",
            origin.room_name, origin.room_id.0, origin.agent_name, origin.agent_id.0);
        let key = format!("bus-{}", &digest(&[machine, session, &source])[..40]);
        let signature = digest(&[body.trim()]);
        Self {
            source,
            signature,
            body,
            key,
        }
    }
}

fn digest(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

impl Client {
    pub(crate) fn authenticate(&self, cancel: Cancellation) -> Result<Authenticated, String> {
        let value = self
            .process
            .run(&["auth", "status", "--json", "--verify"], "", &cancel)?;
        let identity = auth::parse(&value)?;
        Ok(Authenticated {
            process: self.process.clone(),
            identity,
            cancel,
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn at(program: impl Into<std::ffi::OsString>, timeout: std::time::Duration) -> Self {
        Self {
            process: Process::at(program, timeout),
        }
    }
}

impl Authenticated {
    pub(crate) fn send(&self, message: &Message) -> Result<(), SendFailure> {
        let value = self
            .process
            .run(&["auth", "status", "--json", "--verify"], "", &self.cancel)
            .map_err(|message| SendFailure {
                message,
                disable: true,
            })?;
        let current = auth::parse(&value).map_err(|message| SendFailure {
            message,
            disable: true,
        })?;
        if current.open_id() != self.identity.open_id() {
            return Err(SendFailure {
                message: "lark-cli authenticated user changed; enable Feishu notifications again"
                    .into(),
                disable: true,
            });
        }
        let result = self
            .process
            .run(
                &[
                    "im",
                    "+messages-send",
                    "--as",
                    "user",
                    "--user-id",
                    self.identity.open_id(),
                    "--text",
                    "-",
                    "--idempotency-key",
                    &message.key,
                    "--format",
                    "json",
                ],
                &message.body,
                &self.cancel,
            )
            .map_err(|message| SendFailure {
                message,
                disable: false,
            })?;
        if result["ok"] != true {
            return Err(SendFailure {
                message: "lark-cli did not confirm Feishu message delivery".into(),
                disable: false,
            });
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "tests/send_test.rs"]
mod tests;

#[cfg(all(test, unix))]
pub(crate) fn fixture_lease() -> std::fs::File {
    process::fixture_lease()
}
