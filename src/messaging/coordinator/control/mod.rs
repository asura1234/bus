//! Dev receipts, validation and routing on the existing single-writer coordinator.
mod agents;
mod dialogs;
mod inspect;
mod messages;
mod rooms;

use super::{
    launch, mpsc, schema, AddAgent, AgentId, AgentRecipients, Author, BTreeMap, BusCommand,
    BusEvent, Draft, Duration, Method, ModelError, PathBuf, PromptId, Provider, Request, RequestId,
    RequestPhase, ResponseResult, RoomId, RoomKind, RuntimeStatus, Worker, MASTER_AGENT_NEEDS_ROOM,
};
#[cfg(test)]
use super::{
    AgentRuntimeIdentity, BTreeSet, BusState, CallbackDisposition, CallbackEventKind, JsonStore,
    ProviderCallback, SubmissionOutcome, Transport,
};
use crate::bus::control::{Request as ControlRequest, Response};
#[cfg(test)]
use crate::messaging::model::MASTER_ROOM_NAME;
use serde_json::{json, Value};

/// Receipts younger than this are never evicted, so a retry within it replays.
pub(super) const DEV_RECEIPT_RETENTION: Duration = Duration::from_secs(10 * 60);
/// At most this many receipts are kept inside the retention window.
const DEV_RECEIPT_LIMIT: usize = 1024;

/// A committed mutation's response, replayed for a retry with the same ID.
pub(super) struct DevReceipt {
    request: ControlRequest,
    response: Response,
    at: std::time::Instant,
    bytes: usize,
}

impl Worker {
    #[cfg(test)]
    fn dev_response(&mut self, request: &ControlRequest) -> Response {
        self.dev_response_with_events(request, None)
    }

    pub(super) fn dev_response_with_events(
        &mut self,
        request: &ControlRequest,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Response {
        if !self.dev_enabled {
            return Response::failure(
                &request.id,
                "dev_disabled",
                "Start Bus with --dev to enable control",
            );
        }
        if request.id.is_empty() || request.id.len() > 128 {
            return Response::failure(
                &request.id,
                "invalid_request",
                "Request ID must contain 1–128 bytes",
            );
        }
        if let Some(receipt) = self.dev_receipts.get(&request.id) {
            return if receipt.request.method == request.method
                && receipt.request.params == request.params
            {
                receipt.response.clone()
            } else {
                Response::failure(
                    &request.id,
                    "id_conflict",
                    "Request ID already used with different parameters",
                )
            };
        }
        let Some((fields, mutation)) = dev_method_fields(request.method.as_str()) else {
            return Response::failure(&request.id, "unknown_method", "Unknown Bus control method");
        };
        let Some(params) = request.params.as_object() else {
            return Response::failure(
                &request.id,
                "invalid_params",
                "Parameters must be an object",
            );
        };
        if params.keys().any(|key| !fields.contains(&key.as_str())) {
            return Response::failure(&request.id, "invalid_params", "Unknown parameter");
        }
        if (request.method.ends_with(".delete")
            || request.method == "agent.setup-confirm"
            || request.method == "request.recover")
            && request.params.get("confirm") != Some(&Value::Bool(true))
        {
            return Response::failure(
                &request.id,
                "confirmation_required",
                "Explicit --confirm is required",
            );
        }
        let reserve = match self.reserve_dev_receipt(request, mutation) {
            Ok(reserve) => reserve,
            Err(response) => return response,
        };
        let _span = tracing::info_span!("bus.dev.command", control_request_id = %request.id, method = %request.method).entered();
        let started = std::time::Instant::now();
        let response = match self.dev_execute(&request.method, &request.params, events) {
            Ok(result) => Response::success(&request.id, result),
            Err(error) => {
                // Detailed domain diagnostics remain available via state and log IDs.
                Response::failure(
                    &request.id,
                    "command_failed",
                    error.chars().take(700).collect::<String>(),
                )
            }
        };
        tracing::info!(
            event = "bus.dev.command.result",
            ok = response.ok,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Bus dev command finished"
        );
        if mutation {
            self.dev_receipt_bytes = self.dev_receipt_bytes.saturating_add(reserve);
            self.dev_receipt_order.push_back(request.id.clone());
            self.dev_receipts.insert(
                request.id.clone(),
                DevReceipt {
                    request: request.clone(),
                    response: response.clone(),
                    at: std::time::Instant::now(),
                    bytes: reserve,
                },
            );
        }
        response
    }

    fn reserve_dev_receipt(
        &mut self,
        request: &ControlRequest,
        mutation: bool,
    ) -> Result<usize, Response> {
        if mutation && self.storage_failed {
            return Err(Response::failure(
                &request.id,
                "storage_unavailable",
                "Fix Bus storage and restart before making changes",
            ));
        }
        // A receipt lets a retry replay its response instead of launching or
        // sending twice. Retries come within seconds, so receipts older than
        // the retention window make room; younger ones are never evicted.
        let reserve = serde_json::to_vec(request)
            .map_or(usize::MAX, |v| v.len())
            .saturating_add(4096);
        if mutation {
            self.evict_expired_dev_receipts();
        }
        if mutation
            && (self.dev_receipts.len() >= DEV_RECEIPT_LIMIT
                || self.dev_receipt_bytes.saturating_add(reserve) > 8 * 1024 * 1024)
        {
            return Err(Response::failure(
                &request.id,
                "receipt_capacity",
                "Dev mutation receipt limit reached; restart when safe",
            ));
        }
        Ok(reserve)
    }

    fn evict_expired_dev_receipts(&mut self) {
        while let Some(id) = self.dev_receipt_order.front() {
            let Some(receipt) = self.dev_receipts.get(id) else {
                self.dev_receipt_order.pop_front();
                continue;
            };
            if receipt.at.elapsed() < self.dev_receipt_retention {
                break;
            }
            self.dev_receipt_bytes = self.dev_receipt_bytes.saturating_sub(receipt.bytes);
            self.dev_receipts.remove(id);
            self.dev_receipt_order.pop_front();
        }
    }

    fn dev_command(&mut self, command: BusCommand) -> Result<Value, String> {
        // Do not send navigation side effects to the human's TUI event queue.
        let (tx, rx) = mpsc::channel();
        self.command(command, &tx)?;
        for event in rx.try_iter() {
            match event {
                BusEvent::RoomCreated(id) => return Ok(json!({"room_id":id})),
                BusEvent::AgentAdded(id) => return Ok(json!({"agent_id":id,"stage":"launching"})),
                BusEvent::SetupRequired { notice, .. } => {
                    return Err(format!(
                        "Project hook setup requires --consent-hooks: {} ({})",
                        notice.message,
                        notice.path.display()
                    ))
                }
                BusEvent::TerminalsLeftOpen(terminals) => {
                    return Ok(json!({"updated":true,"terminals_left_open":terminals}))
                }
                _ => {}
            }
        }
        Ok(json!({"updated":true}))
    }

    fn dev_execute(
        &mut self,
        method: &str,
        p: &Value,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "agent.focus" | "room.focus" | "room.create" | "room.rename" | "room.notes"
            | "room.seen" | "room.sound" | "room.delete" | "room.history" => {
                self.execute_rooms(method, p, events)
            }
            "state"
            | "diagnostics"
            | "sounds"
            | "settings.color_blind"
            | "settings"
            | "settings.room_sound"
            | "bus.quit"
            | "agent.read" => self.execute_inspect(method, p, events),
            "agent.details"
            | "agent.rename"
            | "agent.delete"
            | "agent.setup-confirm"
            | "agent.add"
            | "agent.clear" => self.execute_agents(method, p, events),
            "agent.dialog.observe" | "agent.dialog.choose" | "agent.dialog.answer" => {
                self.execute_dialogs(method, p, events)
            }
            "message.send" | "message.status" | "request.recover" => {
                self.execute_messages(method, p, events)
            }
            _ => Err("Unknown method".into()),
        }
    }
}

fn required<'a>(p: &'a Value, field: &str) -> Result<&'a str, String> {
    p.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Missing or invalid {field}"))
}

fn optional_text<'a>(p: &'a Value, field: &str) -> Result<Option<&'a str>, String> {
    p.get(field)
        .map(|v| v.as_str().ok_or_else(|| format!("Invalid {field}")))
        .transpose()
}

fn optional_bool(p: &Value, field: &str) -> Result<bool, String> {
    p.get(field).map_or(Ok(false), |v| {
        v.as_bool().ok_or_else(|| format!("Invalid {field}"))
    })
}

fn optional_u32(p: &Value, field: &str) -> Result<Option<u32>, String> {
    p.get(field)
        .map(|value| {
            value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("Invalid {field}"))
        })
        .transpose()
}

fn unique<T>(mut items: impl Iterator<Item = T>) -> Result<T, String> {
    let first = items.next().ok_or("No matching room or agent")?;
    if items.next().is_some() {
        Err("Ambiguous name; use a numeric ID".into())
    } else {
        Ok(first)
    }
}

#[cfg(test)]
#[path = "../tests/control/support_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/focus_test.rs"]
mod focus_tests;

fn dev_method_fields(method: &str) -> Option<(&'static [&'static str], bool)> {
    Some(match method {
        "state" | "diagnostics" | "sounds" | "settings" => (&[], false),
        "room.create" => (&["name"], true),
        "room.rename" => (&["room", "name"], true),
        "room.notes" => (&["room", "text"], true),
        "room.delete" => (&["room", "confirm"], true),
        "room.focus" => (&["room"], true),
        "room.seen" => (&["room"], true),
        "room.sound" => (&["room", "on", "sound"], true),
        "agent.rename" => (&["agent", "name"], true),
        "agent.details" => (&["agent", "on"], true),
        "settings.color_blind" => (&["on"], true),
        "settings.room_sound" => (&["on", "sound"], true),
        "bus.quit" => (&[], true),
        "agent.add" => (
            &[
                "room",
                "name",
                "provider",
                "cwd",
                "extra_args",
                "consent_project_hooks",
                "orchestrates",
                "system_prompt",
            ],
            true,
        ),
        "agent.delete" | "agent.setup-confirm" => (&["agent", "confirm"], true),
        "agent.read" => (&["agent", "source", "lines"], false),
        "agent.dialog.observe" => (&["agent"], false),
        "agent.dialog.choose" => (&["agent", "option", "fingerprint"], true),
        "agent.dialog.answer" => (&["agent", "text", "skip", "fingerprint"], true),
        "agent.focus" => (&["agent"], true),
        "agent.clear" => (&["agent"], true),
        "message.send" => (&["room", "to", "text", "files", "as", "queue"], true),
        "message.status" => (&["message"], false),
        "request.recover" => (&["request", "confirm"], true),
        "room.history" => (&["room"], false),
        _ => return None,
    })
}
