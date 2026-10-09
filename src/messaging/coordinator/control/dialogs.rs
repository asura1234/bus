//! Dialogs development commands on the single coordinator.
use super::{
    mpsc, optional_bool, optional_text, required, schema, AgentId, BusEvent, Duration, Method,
    ResponseResult, RoomId, Worker,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// What a dialog fingerprint vouches for: this agent's launch identity and
/// the exact dialog it showed. A fingerprint answers at most one dialog.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct DialogFingerprintClaims {
    room_id: RoomId,
    agent_id: AgentId,
    launch_id: String,
    terminal_id: String,
    pane_id: String,
    /// `None` while the agent launches, before its session is bound.
    session_id: Option<String>,
    content_revision: u64,
    dialog_digest: String,
    /// The question and labels without the selection, to tell a moved
    /// selection from a different dialog.
    dialog_shape: String,
    options: u32,
    /// Makes each observation's fingerprint distinct, so spending one never
    /// blocks answering the same dialog after observing it again.
    observed_at_ns: u128,
}

/// How long `agent choose` watches the screen for the dialog to react.
const DIALOG_SETTLE_POLLS: u32 = 20;
const DIALOG_SETTLE_INTERVAL: Duration = Duration::from_millis(100);

impl Worker {
    pub(super) fn execute_dialogs(
        &mut self,
        method: &str,
        p: &Value,
        _events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "agent.dialog.observe" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                self.observe_dialog(agent)
            }
            "agent.dialog.choose" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                let option = required(p, "option")?
                    .parse::<u32>()
                    .map_err(|_| "Option must be a number")?;
                self.choose_dialog_option(agent, option, required(p, "fingerprint")?)
            }
            "agent.dialog.answer" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                let text = if p["text"].is_null() {
                    None
                } else {
                    optional_text(p, "text")?
                };
                let skip = optional_bool(p, "skip")?;
                self.answer_dialog(agent, text, skip, required(p, "fingerprint")?)
            }
            _ => Err("Unknown method".into()),
        }
    }

    /// The agent's launch identity: launch, terminal and pane are required;
    /// the session is `None` until the provider binds it.
    pub(in crate::messaging::coordinator) fn dialog_identity(
        &self,
        id: AgentId,
    ) -> Result<(RoomId, String, String, String, Option<String>), String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let identity = &agent.runtime_identity;
        let (Some(launch_id), Some(terminal_id), Some(pane_id)) = (
            identity.launch_id.clone(),
            identity.terminal_id.clone(),
            identity.pane_id.clone(),
        ) else {
            return Err("Agent has no terminal yet".into());
        };
        Ok((
            agent.room_id,
            launch_id,
            terminal_id,
            pane_id,
            identity.session_id.clone(),
        ))
    }

    pub(in crate::messaging::coordinator) fn native_dialog(
        &mut self,
        pane_id: &str,
        terminal_id: &str,
        session_id: Option<&str>,
    ) -> Result<schema::AgentDialogObservation, String> {
        let response = self
            .transport
            .request(Method::AgentDialogObserve(schema::AgentTarget {
                target: pane_id.into(),
            }))
            .map_err(|error| error.message)?;
        let ResponseResult::AgentDialog { observation } = response else {
            return Err("Unexpected native dialog response".into());
        };
        if observation.terminal_id != terminal_id
            || observation.pane_id != pane_id
            || session_id.is_some_and(|session| observation.session_id.as_deref() != Some(session))
        {
            return Err("Agent terminal or session changed; observe the dialog again".into());
        }
        Ok(observation)
    }

    pub(in crate::messaging::coordinator) fn observe_dialog(
        &mut self,
        id: AgentId,
    ) -> Result<Value, String> {
        let (room_id, launch_id, terminal_id, pane_id, session_id) = self.dialog_identity(id)?;
        let observation = self.native_dialog(&pane_id, &terminal_id, session_id.as_deref())?;
        let fingerprint = match &observation.dialog {
            Some(dialog) => Some(encode_dialog_fingerprint(&DialogFingerprintClaims {
                room_id,
                agent_id: id,
                launch_id,
                terminal_id,
                pane_id,
                session_id,
                content_revision: observation.content_revision,
                dialog_digest: dialog.digest.clone(),
                dialog_shape: dialog.id.clone(),
                options: dialog.options.len() as u32,
                observed_at_ns: crate::messaging::storage::io::now_ns(),
            })?),
            None => None,
        };
        Ok(json!({
            "agent_id": id,
            "dialog": observation.dialog.as_ref().map(dialog_json),
            "fingerprint": fingerprint,
            "content_revision": observation.content_revision,
            "observed_at_ms": crate::messaging::storage::io::now_ms(),
        }))
    }

    /// Answers the observed dialog once: the fingerprint is spent before any
    /// key is written, so a lost response can never answer a second dialog.
    pub(in crate::messaging::coordinator) fn choose_dialog_option(
        &mut self,
        id: AgentId,
        option: u32,
        fingerprint: &str,
    ) -> Result<Value, String> {
        self.send_dialog_answer(id, Some(option), None, false, fingerprint)
    }

    pub(in crate::messaging::coordinator) fn answer_dialog(
        &mut self,
        id: AgentId,
        text: Option<&str>,
        skip: bool,
        fingerprint: &str,
    ) -> Result<Value, String> {
        schema::AgentDialogAnswerParams::validate_answer(text, skip)?;
        self.send_dialog_answer(id, None, text, skip, fingerprint)
    }

    pub(in crate::messaging::coordinator) fn send_dialog_answer(
        &mut self,
        id: AgentId,
        option: Option<u32>,
        text: Option<&str>,
        skip: bool,
        fingerprint: &str,
    ) -> Result<Value, String> {
        let claims = decode_dialog_fingerprint(fingerprint)?;
        if claims.agent_id != id {
            return Err("Dialog fingerprint belongs to another agent".into());
        }
        let (room_id, launch_id, terminal_id, pane_id, session_id) = self.dialog_identity(id)?;
        if claims.room_id != room_id
            || claims.launch_id != launch_id
            || claims.terminal_id != terminal_id
            || claims.pane_id != pane_id
            || claims.session_id != session_id
        {
            return Err("Agent launch or session changed; observe the dialog again".into());
        }
        if option.is_some_and(|option| option == 0 || option > claims.options) {
            return Err(format!("Option must be between 1 and {}", claims.options));
        }
        if option.is_none() && claims.options != 0 {
            return Err("This is a choice dialog; use agent choose".into());
        }
        if self.state.dialog_fingerprint_consumed(fingerprint) {
            return Err("Dialog fingerprint was already used; observe the dialog again".into());
        }
        let mut consumed = self.state.clone();
        consumed.consume_dialog_fingerprint(fingerprint.to_owned());
        self.save(consumed)?;
        let method = match option {
            Some(option) => Method::AgentDialogChoose(schema::AgentDialogChooseParams {
                target: pane_id.clone(),
                expected_terminal_id: terminal_id.clone(),
                expected_pane_id: pane_id.clone(),
                expected_session_id: session_id.clone(),
                expected_dialog_digest: claims.dialog_digest.clone(),
                option,
            }),
            None => Method::AgentDialogAnswer(schema::AgentDialogAnswerParams {
                target: pane_id.clone(),
                expected_terminal_id: terminal_id.clone(),
                expected_pane_id: pane_id.clone(),
                expected_session_id: session_id.clone(),
                expected_dialog_digest: claims.dialog_digest.clone(),
                text: text.map(str::to_owned),
                skip,
            }),
        };
        let native = self.transport.request(method);
        let keys = match native {
            Ok(ResponseResult::AgentDialogChosen { choice }) if choice.written => choice.keys,
            Ok(ResponseResult::AgentDialogChosen { choice }) => {
                return Err(format!(
                    "No keys were sent ({}); observe the dialog again",
                    choice.reason.as_deref().unwrap_or("dialog changed")
                ))
            }
            Ok(_) => return Err("Unexpected native dialog response; outcome is uncertain".into()),
            Err(error) => {
                return Err(format!(
                    "Dialog answer outcome is uncertain: {}",
                    error.message
                ))
            }
        };
        self.record_dialog_answer(id, option)?;
        let (outcome, after) = self.settle_dialog_answer(
            &pane_id,
            &terminal_id,
            session_id.as_deref(),
            &claims,
            option.is_some(),
        );
        if outcome == "closed" {
            self.finish_answered_dialog(id)?;
        }
        let mut result = json!({
            "agent_id": id,
            "keys": keys,
            "outcome": outcome,
            "dialog": after.as_ref().map(dialog_json),
        });
        if let Some(option) = option {
            result["option"] = json!(option);
        } else {
            result["skipped"] = json!(skip);
        }
        Ok(result)
    }

    fn settle_dialog_answer(
        &mut self,
        pane_id: &str,
        terminal_id: &str,
        session_id: Option<&str>,
        claims: &DialogFingerprintClaims,
        choice: bool,
    ) -> (&'static str, Option<schema::AgentDialog>) {
        // Moves are confirmed after a short delay, so watch until the dialog
        // closes or another one replaces it.
        let mut outcome = "unchanged";
        let mut after = None;
        for poll in 0..DIALOG_SETTLE_POLLS {
            if poll > 0 {
                std::thread::sleep(DIALOG_SETTLE_INTERVAL);
            }
            let Ok(observation) = self.native_dialog(pane_id, terminal_id, session_id) else {
                outcome = "unknown";
                break;
            };
            outcome = match &observation.dialog {
                None => "closed",
                Some(dialog) if dialog.digest == claims.dialog_digest => "unchanged",
                Some(dialog) if dialog.id == claims.dialog_shape => {
                    if choice {
                        "selection_moved"
                    } else {
                        "input_changed"
                    }
                }
                Some(_) => "replaced",
            };
            after = observation.dialog;
            if matches!(outcome, "closed" | "replaced") {
                break;
            }
        }
        (outcome, after)
    }

    fn record_dialog_answer(&mut self, id: AgentId, option: Option<u32>) -> Result<(), String> {
        // Its closing is expected now, so no "closed on its own" follow-up.
        if let Some(option) = option {
            let mut answered = self.state.clone();
            answered
                .mark_dialog_answered(id, option)
                .map_err(|error| error.to_string())?;
            self.save(answered)?;
        } else {
            let mut answered = self.state.clone();
            let notice = answered
                .agent(id)
                .and_then(|agent| agent.dialog_notice.clone());
            // A text answer has no numbered choice, including if it replaces
            // an earlier choice before the notice poll catches up.
            answered
                .set_dialog_notice(id, notice)
                .map_err(|error| error.to_string())?;
            self.save(answered)?;
        }
        Ok(())
    }
}

fn dialog_json(dialog: &schema::AgentDialog) -> Value {
    json!({
        "kind": dialog.kind,
        "text": dialog.text,
        "options": dialog.options,
        "hint": dialog.hint,
    })
}

fn encode_dialog_fingerprint(claims: &DialogFingerprintClaims) -> Result<String, String> {
    use base64::Engine;
    let payload = serde_json::to_vec(claims).map_err(|error| error.to_string())?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload);
    let digest = format!("{:x}", Sha256::digest(&payload));
    Ok(format!("d1.{encoded}.{digest}"))
}

fn decode_dialog_fingerprint(value: &str) -> Result<DialogFingerprintClaims, String> {
    use base64::Engine;
    let malformed = || "Dialog fingerprint is malformed; observe the dialog again".to_owned();
    let mut parts = value.split('.');
    let (Some("d1"), Some(encoded), Some(expected), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(malformed());
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| malformed())?;
    if format!("{:x}", Sha256::digest(&payload)) != expected {
        return Err(malformed());
    }
    serde_json::from_slice(&payload).map_err(|_| malformed())
}
