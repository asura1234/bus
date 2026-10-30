//! Dev-only, provider-neutral fake-agent callback protocol.
use super::spool::{field, optional_text, Parsed};
use serde_json::Value;

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "event")?;
    let session = field(value, "session")?;
    if event == "session" {
        return Ok(Parsed::Session {
            session,
            source: Some("startup".into()),
        });
    }
    let turn = field(value, "turn")?;
    match event.as_str() {
        "started" => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "final" => Ok(Parsed::Final {
            session,
            turn,
            text: optional_text(value, "text")?,
        }),
        "failure" => Ok(Parsed::Failure {
            session,
            turn,
            message: "Fake turn failed.".into(),
            error: None,
        }),
        _ => Err(format!("Unsupported fake callback event {event}")),
    }
}
