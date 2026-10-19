mod process;
pub(crate) mod waiting;

pub(crate) use process::Cancellation;
use process::Process;

#[derive(Clone, Default)]
pub(crate) struct Client {
    process: Process,
}

pub(crate) fn validate_recipient(recipient: &str) -> Result<(), String> {
    if recipient.strip_prefix("ou_").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }) {
        Ok(())
    } else {
        Err("Feishu recipient must be a user open_id (ou_...)".into())
    }
}

impl Client {
    pub(crate) fn send(
        &self,
        recipient: &str,
        program: Option<&str>,
        text: &str,
        key: &str,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        validate_recipient(recipient)?;
        let mut process = self.process.clone();
        if let Some(program) = program.filter(|p| !p.is_empty()) {
            process.set_program(program);
        }
        let value = process.run(
            &[
                "im",
                "+messages-send",
                "--as",
                "bot",
                "--user-id",
                recipient,
                "--text",
                text,
                "--idempotency-key",
                key,
                "--format",
                "json",
            ],
            "",
            cancel,
        )?;
        if value["ok"] == true {
            Ok(())
        } else {
            Err("lark-cli did not confirm Feishu message delivery".into())
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn on_path(path: &std::path::Path, timeout: std::time::Duration) -> Self {
        let mut process = Process::at("lark-cli", timeout);
        process.path = Some(format!("{}:/usr/bin:/bin", path.display()).into());
        Self { process }
    }
}
