//! Durable provider callback consumption and request correlation.
use super::*;

impl Worker {
    pub(super) fn consume_callbacks(&mut self, id: AgentId, dir: &Path) -> Result<(), String> {
        let mut records = callbacks::records(dir).map_err(|e| e.to_string())?;
        // Companion hooks can reach the spool in either order, including after reconnect.
        records.sort_by_key(|(_, r)| {
            (
                match callbacks::parse(r.manifest.provider, &r.value) {
                    Ok(Parsed::Session(_)) => 0,
                    Ok(Parsed::Started { .. }) => 1,
                    _ => 2,
                },
                r.sequence,
            )
        });
        for (path, record) in &records {
            let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
            if record.manifest.agent_id != id
                || record.manifest.provider != agent.provider
                || Some(record.manifest.launch_id.as_str())
                    != agent.runtime_identity.launch_id.as_deref()
            {
                return self.agent_error(id, "Callback launch identity mismatch".into());
            }
            if agent.session_binding_invalidated {
                return Ok(());
            }
            let parsed = match callbacks::parse(agent.provider, &record.value) {
                Ok(parsed) => parsed,
                Err(message) => {
                    let mut state = self.state.clone();
                    state
                        .set_agent_error(id, Some(message))
                        .map_err(|e| e.to_string())?;
                    self.save(state)?;
                    continue;
                }
            };
            let mut state = self.state.clone();
            let callback_session = match &parsed {
                Parsed::Session(session)
                | Parsed::Started { session, .. }
                | Parsed::Final { session, .. }
                | Parsed::Failure { session, .. }
                | Parsed::CursorStop { session, .. }
                | Parsed::CursorResponse { session, .. } => Some(session),
                Parsed::Ignore => None,
            };
            if let Some(session) = callback_session {
                if agent
                    .runtime_identity
                    .session_id
                    .as_ref()
                    .is_some_and(|known| known != session)
                {
                    // Retain the attested identity even on uncertain sends. Clearing
                    // it would let a repeated foreign SessionStart silently rebind.
                    if matches!(parsed, Parsed::Session(_)) {
                        state
                            .invalidate_agent_session(id)
                            .map_err(|e| e.to_string())?;
                    }
                    state
                        .observe_status(id, RuntimeStatus::Unavailable, crate::bus::io::now_ms())
                        .map_err(|e| e.to_string())?;
                    state.set_agent_error(id, Some("Provider callback session differs from this Bus launch; request ownership is preserved. Inspect the terminal and create a new Bus agent for a new session.".into())).map_err(|e| e.to_string())?;
                    self.save(state)?;
                    std::fs::remove_file(path).map_err(|e| e.to_string())?;
                    crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())?;
                    continue;
                }
                if agent.runtime_identity.session_id.is_none()
                    && !matches!(parsed, Parsed::Session(_))
                {
                    // Do not bind a turn before this launch's SessionStart attests
                    // which provider session owns the terminal.
                    continue;
                }
            }
            let mut remove = vec![path.clone()];
            let callback = match parsed {
                Parsed::Session(session) => {
                    let mut identity = agent.runtime_identity.clone();
                    let pane = identity
                        .pane_id
                        .clone()
                        .ok_or("Callback arrived before pane creation was recorded")?;
                    self.transport
                        .request(Method::PaneReportAgentSession(
                            schema::PaneReportAgentSessionParams {
                                pane_id: pane,
                                // Use the existing provider adapter namespace:
                                // a generic source is deliberately rejected by
                                // the native session identity consumer.
                                source: format!("herdr:{}", launch::provider_kind(agent.provider)),
                                agent: launch::provider_kind(agent.provider).into(),
                                seq: Some(record.sequence),
                                agent_session_id: Some(session.clone()),
                                agent_session_path: None,
                                session_start_source: Some("startup".into()),
                            },
                        ))
                        .map_err(|e| e.message)?;
                    identity.session_id = Some(session);
                    state
                        .set_agent_runtime_identity(id, identity)
                        .map_err(|e| e.to_string())?;
                    if agent.current_request.is_none() {
                        state.set_agent_error(id, if agent.hook_setup_confirmed {None} else {Some("Finish trusting all Bus hook entries and confirm hook setup in Bus; room prompts remain queued.".into())}).map_err(|e|e.to_string())?;
                    }
                    None
                }
                Parsed::Started {
                    session,
                    turn,
                    prompt,
                } => Some(record.callback(
                    session,
                    turn,
                    Some(prompt),
                    CallbackEventKind::PromptStarted,
                )),
                Parsed::Final {
                    session,
                    turn,
                    text,
                } => Some(record.callback(session, turn, None, CallbackEventKind::Final { text })),
                Parsed::Failure {
                    session,
                    turn,
                    message,
                } => {
                    Some(record.callback(session, turn, None, CallbackEventKind::Error { message }))
                }
                Parsed::CursorStop { session, turn } => {
                    let found =
                        records.iter().find_map(|(other_path, other)| {
                            match callbacks::parse(agent.provider, &other.value) {
                                Ok(Parsed::CursorResponse {
                                    session: s,
                                    turn: t,
                                    text,
                                }) if s == session
                                    && t == turn
                                    && other.manifest.launch_id == record.manifest.launch_id =>
                                {
                                    Some((other_path, text))
                                }
                                _ => None,
                            }
                        });
                    let Some((other_path, text)) = found else {
                        continue;
                    };
                    remove.push(other_path.clone());
                    Some(record.callback(session, turn, None, CallbackEventKind::Final { text }))
                }
                Parsed::CursorResponse { .. } => continue,
                Parsed::Ignore => None,
            };
            if let Some(callback) = callback {
                if matches!(callback.kind, CallbackEventKind::Final { .. })
                    && state
                        .agent(id)
                        .and_then(|a| a.current_request)
                        .and_then(|r| state.request(r))
                        .is_some_and(|r| {
                            !r.trusted_start_bound
                                && callback.sequence > r.submission_boundary.unwrap_or(u64::MAX)
                        })
                {
                    state.set_agent_error(id,Some("Awaiting trusted submit-hook binding; final callback retained and no retry will occur".into())).map_err(|e|e.to_string())?;
                    self.save(state)?;
                    continue;
                }
                let disposition = state.accept_callback(callback);
                if matches!(
                    disposition,
                    CallbackDisposition::Rejected(
                        CallbackRejection::UnboundFinal | CallbackRejection::WrongPrompt
                    )
                ) {
                    state.set_agent_error(id,Some("Unbound provider callback: verify trusted submit/stop hooks. The active request will not be retried.".into())).map_err(|e|e.to_string())?;
                }
            }
            self.save(state)?;
            for path in remove {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.to_string()),
                }
            }
            crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
