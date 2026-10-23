//! Dialogs development commands on the single coordinator.
use super::{
    mpsc, optional_bool, optional_text, required, schema, AgentId, BusEvent, Duration, Method,
    ResponseResult, RoomId, Worker,
};
use crate::messaging::native::TransportError;
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
    /// The highest option number; a scrolled list can hide some below it.
    options: u32,
    /// Claude's free-text row: answered with text rather than chosen, unless
    /// it holds a draft (Enter submits it) or is a checkbox (Enter toggles it).
    free_text: Option<schema::AgentDialogFreeText>,
    /// Makes each observation's fingerprint distinct, so spending one never
    /// blocks answering the same dialog after observing it again.
    observed_at_ns: u128,
}

/// The native answer only moved to a hidden free-text row (see
/// `fill_focused_free_text`).
const FREE_TEXT_FOCUSED: &str = "free_text_focused";
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
                let agent = self.control_agent(required(p, "agent")?, None)?;
                self.observe_dialog(agent)
            }
            "agent.dialog.choose" => {
                let agent = self.control_agent(required(p, "agent")?, None)?;
                let option = required(p, "option")?
                    .parse::<u32>()
                    .map_err(|_| "Option must be a number")?;
                self.choose_dialog_option(agent, option, required(p, "fingerprint")?)
            }
            "agent.dialog.answer" => {
                let agent = self.control_agent(required(p, "agent")?, None)?;
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
        open_pending_question: bool,
    ) -> Result<schema::AgentDialogObservation, String> {
        self.native_dialog_checked(pane_id, terminal_id, session_id, open_pending_question)
            .map_err(|error| error.message)
    }

    fn native_dialog_checked(
        &mut self,
        pane_id: &str,
        terminal_id: &str,
        session_id: Option<&str>,
        open_pending_question: bool,
    ) -> Result<schema::AgentDialogObservation, TransportError> {
        let invalid = |message: &str| TransportError {
            code: None,
            message: message.into(),
            definitely_rejected: true,
        };
        let response = self.transport.request(Method::AgentDialogObserve(
            schema::AgentDialogObserveParams {
                target: pane_id.into(),
                open_pending_question,
            },
        ))?;
        let ResponseResult::AgentDialog { observation } = response else {
            return Err(invalid("Unexpected native dialog response"));
        };
        if observation.terminal_id != terminal_id
            || observation.pane_id != pane_id
            || session_id.is_some_and(|session| observation.session_id.as_deref() != Some(session))
        {
            return Err(invalid(
                "Agent terminal or session changed; observe the dialog again",
            ));
        }
        Ok(observation)
    }

    pub(in crate::messaging::coordinator) fn observe_dialog(
        &mut self,
        id: AgentId,
    ) -> Result<Value, String> {
        let (room_id, launch_id, terminal_id, pane_id, session_id) = self.dialog_identity(id)?;
        let mut observation = None;
        let mut awaiting_question = self.state.agent(id).is_some_and(|agent| {
            agent.dialog_notice.as_deref() == Some(crate::agents::dialog::CODEX_PENDING_QUESTION_ID)
        });
        // Shift+Left is queued once. A redraw can temporarily prevent a coherent
        // snapshot, including on that first read. Retry only that read error,
        // within the same bounded wait, without sending navigation again.
        for poll in 0..=DIALOG_SETTLE_POLLS {
            if poll > 0 {
                std::thread::sleep(DIALOG_SETTLE_INTERVAL);
            }
            match self.native_dialog_checked(
                &pane_id,
                &terminal_id,
                session_id.as_deref(),
                poll == 0,
            ) {
                Ok(current) => {
                    awaiting_question |= current.pending_question;
                    let settled = current.dialog.is_some() || !awaiting_question;
                    observation = Some(current);
                    // Removing the collapsed queue can precede drawing its
                    // question. An empty intermediate frame is not settlement.
                    if settled {
                        break;
                    }
                }
                Err(error)
                    if error.code.as_deref() == Some("dialog_observation_unavailable")
                        && poll < DIALOG_SETTLE_POLLS => {}
                Err(error) => return Err(error.message),
            }
        }
        let observation = observation.ok_or("Agent dialog observation unavailable")?;
        // Inspection resolves an already-notified collapsed episode into its
        // concrete shape; opening the panel does not ask for help a second time.
        if self.state.agent(id).is_some_and(|agent| {
            agent.dialog_notice.as_deref() == Some(crate::agents::dialog::CODEX_PENDING_QUESTION_ID)
        }) {
            if let Some(dialog) = &observation.dialog {
                let mut state = self.state.clone();
                state
                    .set_dialog_notice(id, Some(dialog.id.clone()))
                    .map_err(|error| error.to_string())?;
                self.save(state)?;
                self.dialog_seen.remove(&id);
            }
        }
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
                options: dialog.options.last().map_or(0, |option| option.number),
                free_text: dialog.free_text,
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

    /// Only a fresh complete observation of the same known UI shape can
    /// authorise an automatic choice. Never infer it from the returned text.
    pub(in crate::messaging::coordinator) fn observe_allowlisted_fingerprint(
        &mut self,
        id: AgentId,
        expected_shape: &str,
    ) -> Result<Option<(String, u32)>, String> {
        let observation = self.observe_dialog(id)?;
        let Some(fingerprint) = observation["fingerprint"].as_str() else {
            return Ok(None);
        };
        let claims = decode_dialog_fingerprint(fingerprint)?;
        let dialog = &observation["dialog"];
        let options = dialog["options"].as_array();
        let actionable = options.is_some_and(|options| {
            options.len() == 3
                && options.iter().enumerate().all(|(index, row)| {
                    row["number"].as_u64() == Some(index as u64 + 1)
                        && !row["free_text"].as_bool().unwrap_or(false)
                })
                && options.iter().filter(|row| row["selected"] == true).count() == 1
        });
        if claims.session_id.is_none()
            || claims.dialog_shape != expected_shape
            || claims.options != 3
            || claims.free_text.is_some()
            || dialog["kind"] != "choice"
            || !actionable
            || dialog["tabs"]
                .as_array()
                .is_some_and(|tabs| !tabs.is_empty())
        {
            return Ok(None);
        }
        let selected = options
            .and_then(|rows| rows.iter().find(|row| row["selected"] == true))
            .and_then(|row| row["number"].as_u64())
            .ok_or("Missing selection")? as u32;
        Ok(Some((fingerprint.to_owned(), selected)))
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

    /// Automatic UI replies are settled by later ordinary polls, so one
    /// slow redraw cannot hold the entire coordinator for the CLI wait loop.
    pub(in crate::messaging::coordinator) fn choose_automatic_dialog_option(
        &mut self,
        id: AgentId,
        option: u32,
        fingerprint: &str,
        action: schema::AgentDialogChoiceAction,
    ) -> Result<Value, String> {
        self.write_dialog_answer(id, Some(option), None, false, fingerprint, Some(action))
    }

    pub(in crate::messaging::coordinator) fn send_dialog_answer(
        &mut self,
        id: AgentId,
        option: Option<u32>,
        text: Option<&str>,
        skip: bool,
        fingerprint: &str,
    ) -> Result<Value, String> {
        self.write_dialog_answer(id, option, text, skip, fingerprint, None)
    }

    fn write_dialog_answer(
        &mut self,
        id: AgentId,
        option: Option<u32>,
        text: Option<&str>,
        skip: bool,
        fingerprint: &str,
        automatic: Option<schema::AgentDialogChoiceAction>,
    ) -> Result<Value, String> {
        let claims = decode_dialog_fingerprint(fingerprint)?;
        let age_ms =
            crate::messaging::storage::io::now_ms().saturating_sub(observed_at_ms(&claims));
        if age_ms > crate::messaging::model::DIALOG_ANSWER_TTL_MS {
            return Err("Dialog fingerprint expired; observe the dialog again".into());
        }
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
        check_free_text(&claims, option, skip)?;
        let digest = fingerprint_digest(fingerprint);
        if self.state.dialog_answer_spent(digest) {
            return Err("Dialog fingerprint was already used; observe the dialog again".into());
        }
        let mut consumed = self.state.clone();
        consumed.spend_dialog_answer(digest.to_owned(), observed_at_ms(&claims));
        self.save(consumed)?;
        let method = match option {
            Some(option) => Method::AgentDialogChoose(schema::AgentDialogChooseParams {
                target: pane_id.clone(),
                expected_terminal_id: terminal_id.clone(),
                expected_pane_id: pane_id.clone(),
                expected_session_id: session_id.clone(),
                expected_dialog_digest: claims.dialog_digest.clone(),
                option,
                action: automatic.unwrap_or_default(),
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
            Ok(ResponseResult::AgentDialogChosen { choice })
                if choice.written && choice.reason.as_deref() == Some(FREE_TEXT_FOCUSED) =>
            {
                let mut keys = choice.keys;
                keys.extend(self.fill_focused_free_text(
                    &pane_id,
                    &terminal_id,
                    session_id.as_deref(),
                    text.unwrap_or_default(),
                )?);
                keys
            }
            Ok(ResponseResult::AgentDialogChosen { choice }) if choice.written => choice.keys,
            Ok(ResponseResult::AgentDialogChosen { choice }) => {
                return Err(format!(
                    "No keys were sent ({}); observe the dialog again",
                    choice.reason.as_deref().unwrap_or("dialog changed")
                ))
            }
            Ok(_) => return Err("Unexpected native dialog response; outcome is uncertain".into()),
            Err(error) if error.code.as_deref() == Some("dialog_redrawing") => {
                return Err(
                    "No keys were sent (dialog_redrawing); observe the dialog again".into(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "Dialog answer outcome is uncertain: {}",
                    error.message
                ))
            }
        };
        if automatic != Some(schema::AgentDialogChoiceAction::Focus) {
            self.record_dialog_answer(id, option)?;
        }
        if automatic.is_some() {
            return Ok(json!({"agent_id": id, "option": option, "keys": keys, "outcome": "sent"}));
        }
        self.settle_written_dialog_answer(id, option, skip, keys, &claims)
    }

    fn settle_written_dialog_answer(
        &mut self,
        id: AgentId,
        option: Option<u32>,
        skip: bool,
        keys: Vec<String>,
        claims: &DialogFingerprintClaims,
    ) -> Result<Value, String> {
        let (outcome, after) = self.settle_dialog_answer(
            &claims.pane_id,
            &claims.terminal_id,
            claims.session_id.as_deref(),
            claims,
            option.is_some(),
        );
        if outcome == "closed" {
            self.finish_closed_dialog(id)?;
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

    /// The second step of answering a free-text row scrolled out of view:
    /// once the moves redraw it as the focused field, type only if it is empty.
    fn fill_focused_free_text(
        &mut self,
        pane_id: &str,
        terminal_id: &str,
        session_id: Option<&str>,
        text: &str,
    ) -> Result<Vec<String>, String> {
        let mut field = None;
        for poll in 0..DIALOG_SETTLE_POLLS {
            if poll > 0 {
                std::thread::sleep(DIALOG_SETTLE_INTERVAL);
            }
            if let Ok(observation) =
                self.native_dialog_checked(pane_id, terminal_id, session_id, false)
            {
                field = observation.dialog.filter(|dialog| {
                    dialog.kind == schema::AgentDialogKind::Question
                        || dialog.free_text.is_some_and(|field| {
                            field.checkbox
                                && dialog
                                    .options
                                    .iter()
                                    .any(|row| row.number == field.option && row.selected)
                        })
                });
                if field.is_some() {
                    break;
                }
            }
        }
        let field = field.ok_or(
            "Moved to the free-text row, but it never showed as a focused field; no text was typed",
        )?;
        if field.free_text.is_some_and(|free_text| free_text.draft) {
            return Err(
                "Moved to the free-text row; no text was typed (free_text_holds_draft): edit the draft in the agent's pane or answer it there".into(),
            );
        }
        let answer =
            self.transport
                .request(Method::AgentDialogAnswer(schema::AgentDialogAnswerParams {
                    target: pane_id.into(),
                    expected_terminal_id: terminal_id.into(),
                    expected_pane_id: pane_id.into(),
                    expected_session_id: session_id.map(str::to_owned),
                    expected_dialog_digest: field.digest,
                    text: Some(text.into()),
                    skip: false,
                }));
        match answer {
            Ok(ResponseResult::AgentDialogChosen { choice }) if choice.written => Ok(choice.keys),
            Ok(ResponseResult::AgentDialogChosen { choice }) => Err(format!(
                "Moved to the free-text row; no text was typed ({})",
                choice.reason.as_deref().unwrap_or("dialog changed")
            )),
            Ok(_) => Err("Unexpected native dialog response; outcome is uncertain".into()),
            Err(error) => Err(format!(
                "Dialog answer outcome is uncertain: {}",
                error.message
            )),
        }
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
            let observation =
                match self.native_dialog_checked(pane_id, terminal_id, session_id, false) {
                    Ok(observation) => observation,
                    Err(error) => {
                        tracing::debug!(event = "bus.dialog.settle_observation_failed", pane_id,
                        poll, code = ?error.code, message = %error.message,
                        "Post-answer observation failed; keys will not be resent");
                        outcome = "unknown";
                        if error.code.as_deref() == Some("dialog_observation_unavailable") {
                            continue;
                        }
                        break;
                    }
                };
            outcome = match &observation.dialog {
                None => "closed",
                Some(dialog) if dialog.digest == claims.dialog_digest => "unchanged",
                // Text typed into a choice list's free-text row shows as its
                // focused field until Claude takes the delayed Enter.
                Some(dialog)
                    if !choice
                        && claims.free_text.is_some()
                        && dialog.kind == schema::AgentDialogKind::Question =>
                {
                    "input_changed"
                }
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
        // Record the answer before verification; retire its notice only once
        // the dialog closes, so help is not resolved on an uncertain outcome.
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

/// Claude's free-text row takes text, not Enter, unless Enter is harmless
/// there: on a draft it submits the draft, on a checkbox it toggles it.
fn check_free_text(
    claims: &DialogFingerprintClaims,
    option: Option<u32>,
    skip: bool,
) -> Result<(), String> {
    match (option, claims.free_text) {
        (Some(option), Some(field)) if option == field.option && !field.draft && !field.checkbox => {
            Err(format!(
                "Option {option} is a free-text field, and Enter on it without text declines the question; use agent answer --text"
            ))
        }
        (Some(_), _) => Ok(()),
        (None, Some(field)) if field.draft && !skip => {
            let action = if field.option == 0 {
                "edit or submit it in the agent's pane".into()
            } else {
                format!("agent choose {} submits it, or edit it in the agent's pane", field.option)
            };
            Err(format!("The free-text field already holds a draft (free_text_holds_draft); {action}"))
        }
        _ if claims.options == 0 => Ok(()),
        (None, Some(_)) if !skip => Ok(()),
        _ => Err("This is a choice dialog; use agent choose".into()),
    }
}

fn dialog_json(dialog: &schema::AgentDialog) -> Value {
    let mut value = json!({
        "kind": dialog.kind,
        "text": dialog.text,
        "options": dialog.options,
        "hint": dialog.hint,
    });
    if !dialog.tabs.is_empty() {
        value["tabs"] = json!(dialog.tabs);
    }
    if let Some(field) = dialog.free_text {
        value["free_text"] = json!(field);
    }
    value
}

fn encode_dialog_fingerprint(claims: &DialogFingerprintClaims) -> Result<String, String> {
    use base64::Engine;
    let payload = serde_json::to_vec(claims).map_err(|error| error.to_string())?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload);
    let digest = format!("{:x}", Sha256::digest(&payload));
    Ok(format!("d1.{encoded}.{digest}"))
}

pub(in crate::messaging::coordinator) use crate::messaging::model::fingerprint_digest;

fn observed_at_ms(claims: &DialogFingerprintClaims) -> u64 {
    u64::try_from(claims.observed_at_ns / 1_000_000).unwrap_or(u64::MAX)
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

#[cfg(test)]
#[path = "../tests/control/fingerprint_test.rs"]
mod fingerprint_tests;
