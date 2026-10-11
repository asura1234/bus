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
use crate::messaging::control::{Request as ControlRequest, Response};
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
        let Some(&MethodSpec {
            command,
            fields,
            mutation,
            tier,
            ..
        }) = method_spec(request.method.as_str())
        else {
            return Response::failure(&request.id, "unknown_method", "Unknown Bus control method");
        };
        if tier == Tier::Dev && !self.dev_enabled {
            return Response::failure(
                &request.id,
                "dev_tools_disabled",
                format!("`{command}` is a dev tool; this session was not started with --dev"),
            );
        }
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
        if mutation && self.storage_pause.is_some() {
            return Err(Response::failure(
                &request.id,
                "storage_unavailable",
                self.storage_notice(),
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

pub(super) fn is_mutation(method: &str) -> bool {
    method_spec(method).is_some_and(|spec| spec.mutation)
}

/// Who a control method answers. Agent-tier methods answer in every session;
/// dev-tier methods inspect or drive Bus's own UI and internals, so they
/// answer only in a session started with `--dev`. The gate keeps them out of
/// normal sessions' agents and help text; it is not a security boundary, since
/// any process of the same user can reach the private socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    Agent,
    Dev,
}

/// One control method: its CLI command words, allowed parameters, whether a
/// retry replays a receipt, and its tier.
pub(crate) struct MethodSpec {
    pub(crate) method: &'static str,
    pub(crate) command: &'static str,
    pub(crate) fields: &'static [&'static str],
    pub(crate) mutation: bool,
    pub(crate) tier: Tier,
}

const fn spec(
    method: &'static str,
    command: &'static str,
    fields: &'static [&'static str],
    mutation: bool,
    tier: Tier,
) -> MethodSpec {
    MethodSpec {
        method,
        command,
        fields,
        mutation,
        tier,
    }
}

/// Every control method. A method missing here is refused as unknown, so a new
/// one cannot ship without a tier. Dev-tier methods from `src/devtools` (the
/// TUI driver's `ui hits` / `ui screen`) are registered in this table too.
pub(crate) const METHODS: &[MethodSpec] = &[
    spec("state", "state", &[], false, Tier::Agent),
    spec("settings", "settings", &[], false, Tier::Agent),
    spec("room.create", "room create", &["name"], true, Tier::Agent),
    spec(
        "room.rename",
        "room rename",
        &["room", "name"],
        true,
        Tier::Agent,
    ),
    spec(
        "room.notes",
        "room notes",
        &["room", "text"],
        true,
        Tier::Agent,
    ),
    spec(
        "room.delete",
        "room delete",
        &["room", "confirm"],
        true,
        Tier::Agent,
    ),
    spec("room.history", "history", &["room"], false, Tier::Agent),
    spec(
        "agent.add",
        "agent add",
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
        Tier::Agent,
    ),
    spec(
        "agent.read",
        "agent read",
        &["agent", "source", "lines"],
        false,
        Tier::Agent,
    ),
    spec(
        "agent.dialog.observe",
        "agent dialog",
        &["agent"],
        false,
        Tier::Agent,
    ),
    spec(
        "agent.dialog.choose",
        "agent choose",
        &["agent", "option", "fingerprint"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.dialog.answer",
        "agent answer",
        &["agent", "text", "skip", "fingerprint"],
        true,
        Tier::Agent,
    ),
    spec("agent.clear", "agent clear", &["agent"], true, Tier::Agent),
    spec(
        "agent.rename",
        "agent rename",
        &["agent", "name"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.setup-confirm",
        "agent setup-confirm",
        &["agent", "confirm"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.delete",
        "agent delete",
        &["agent", "confirm"],
        true,
        Tier::Agent,
    ),
    spec(
        "message.send",
        "send",
        &["room", "to", "text", "files", "as", "queue"],
        true,
        Tier::Agent,
    ),
    spec(
        "message.status",
        "message status",
        &["message"],
        false,
        Tier::Agent,
    ),
    spec(
        "request.recover",
        "request recover",
        &["request", "confirm"],
        true,
        Tier::Agent,
    ),
    // Dev tier: the human's view and preferences, and Bus's own internals.
    spec("room.focus", "room focus", &["room"], true, Tier::Dev),
    spec("room.seen", "room seen", &["room"], true, Tier::Dev),
    spec(
        "room.sound",
        "room sound",
        &["room", "on", "sound"],
        true,
        Tier::Dev,
    ),
    spec("agent.focus", "agent focus", &["agent"], true, Tier::Dev),
    spec(
        "agent.details",
        "agent details",
        &["agent", "on"],
        true,
        Tier::Dev,
    ),
    spec(
        "settings.color_blind",
        "settings color-blind",
        &["on"],
        true,
        Tier::Dev,
    ),
    spec(
        "settings.room_sound",
        "settings room-sound",
        &["on", "sound"],
        true,
        Tier::Dev,
    ),
    spec("sounds", "sounds", &[], false, Tier::Dev),
    spec("bus.quit", "quit", &[], true, Tier::Dev),
    spec("diagnostics", "diagnostics", &[], false, Tier::Dev),
];

/// Every place `text` names a dev-tier command the way docs and help do
/// (`bus room focus`, `` `room focus` ``, or a help line), plus any
/// `diagnostics` at all, since that command is dev tier as a whole.
#[cfg(test)]
pub(crate) fn dev_tier_mentions(text: &str) -> Vec<String> {
    let mut found: Vec<String> = METHODS
        .iter()
        .filter(|spec| spec.tier == Tier::Dev)
        .flat_map(|spec| {
            [
                format!("bus {}", spec.command),
                format!("`{}`", spec.command),
                format!("\n  {}", spec.command),
            ]
        })
        .filter(|pattern| text.contains(pattern.as_str()))
        .collect();
    if text.contains("diagnostics") {
        found.push("diagnostics".into());
    }
    found
}

pub(crate) fn method_spec(method: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|spec| spec.method == method)
}
