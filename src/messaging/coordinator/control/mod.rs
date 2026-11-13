//! Control receipts, tiers, validation and routing on the existing single-writer coordinator.
mod agents;
mod driver;
mod feishu;
mod inspect;
mod messages;
mod rooms;
mod selectors;
mod sender;
#[cfg(feature = "dev-tools")]
pub(crate) mod ui_hits;

use super::{
    launch, mpsc, schema, AddAgent, AgentId, AgentRecipients, Author, BTreeMap, BusCommand,
    BusEvent, Draft, Duration, Method, ModelError, PathBuf, PromptId, Provider, Request, RequestId,
    RequestPhase, ResponseResult, RoomId, RuntimeStatus, Worker, MASTER_AGENT_NEEDS_ROOM,
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
pub(super) const CONTROL_RECEIPT_RETENTION: Duration = Duration::from_secs(10 * 60);
/// At most this many receipts are kept inside the retention window.
const CONTROL_RECEIPT_LIMIT: usize = 1024;

/// A committed mutation's response, replayed for a retry with the same ID.
pub(super) struct ControlReceipt {
    request: ControlRequest,
    response: Response,
    inferred_sender: Option<Author>,
    at: std::time::Instant,
    bytes: usize,
}

impl Worker {
    #[cfg(test)]
    fn control_response(&mut self, request: &ControlRequest) -> Response {
        self.control_response_with_events(request, None)
    }

    pub(super) fn control_response_with_events(
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
        if self.control_receipts.contains_key(&request.id) {
            return self.replay_control_receipt(request);
        }
        // Only a published, process-bound restart may request normal quit in
        // a non-dev session. Ordinary `quit` retains its dev-tier contract.
        if request.method == "bus.quit" && request.params.get("restart").is_some() {
            return self.restart_quit_response(request, events);
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
        let reserve = match self.reserve_control_receipt(request, mutation) {
            Ok(reserve) => reserve,
            Err(response) => return response,
        };
        let _span = tracing::info_span!("bus.control.command", control_request_id = %request.id, method = %request.method).entered();
        let started = std::time::Instant::now();
        let response = match self.control_execute(&request.method, &request.params, events) {
            Ok(result) => Response::success(&request.id, result),
            Err(error) => {
                // Detailed domain diagnostics remain available via state and log IDs.
                self.master_reserved_response(request)
                    .or_else(|| self.reply_conflict_response(request))
                    .or_else(|| self.recipient_deleted_response(request))
                    .unwrap_or_else(|| {
                        Response::failure(
                            &request.id,
                            "command_failed",
                            error.chars().take(700).collect::<String>(),
                        )
                    })
            }
        };
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if response
            .error
            .as_ref()
            .is_some_and(|error| error.code == "recipient_deleted")
        {
            // A refused deleted recipient is not a failure record. Debug only.
            tracing::debug!(
                event = "bus.control.command.result",
                ok = false,
                elapsed_ms,
                "Bus control command finished"
            );
        } else {
            tracing::info!(
                event = "bus.control.command.result",
                ok = response.ok,
                elapsed_ms,
                "Bus control command finished"
            );
        }
        if mutation {
            self.control_receipt_bytes = self.control_receipt_bytes.saturating_add(reserve);
            self.control_receipt_order.push_back(request.id.clone());
            let inferred_sender = self.receipt_sender(request, &response);
            self.control_receipts.insert(
                request.id.clone(),
                ControlReceipt {
                    request: request.clone(),
                    response: response.clone(),
                    inferred_sender,
                    at: std::time::Instant::now(),
                    bytes: reserve,
                },
            );
        }
        response
    }

    fn reserve_control_receipt(
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
            self.evict_expired_control_receipts();
        }
        if mutation
            && (self.control_receipts.len() >= CONTROL_RECEIPT_LIMIT
                || self.control_receipt_bytes.saturating_add(reserve) > 8 * 1024 * 1024)
        {
            return Err(Response::failure(
                &request.id,
                "receipt_capacity",
                "Control mutation receipt limit reached; restart when safe",
            ));
        }
        Ok(reserve)
    }

    fn evict_expired_control_receipts(&mut self) {
        while let Some(id) = self.control_receipt_order.front() {
            let Some(receipt) = self.control_receipts.get(id) else {
                self.control_receipt_order.pop_front();
                continue;
            };
            if receipt.at.elapsed() < self.control_receipt_retention {
                break;
            }
            self.control_receipt_bytes = self.control_receipt_bytes.saturating_sub(receipt.bytes);
            self.control_receipts.remove(id);
            self.control_receipt_order.pop_front();
        }
    }

    fn control_command(&mut self, command: BusCommand) -> Result<Value, String> {
        // Do not send navigation side effects to the developer's TUI event queue.
        // CLI ids are 0 so a leaked event cannot match a UI origin (those start at 1).
        let (tx, rx) = mpsc::channel();
        self.active_command_id = 0;
        self.command(command, &tx)?;
        for event in rx.try_iter() {
            match event {
                BusEvent::RoomCreated { room: id, .. } => return Ok(json!({"room_id":id})),
                BusEvent::AgentAdded { agent: id, .. } => {
                    return Ok(json!({"agent_id":id,"stage":"launching"}))
                }
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

    fn control_execute(
        &mut self,
        method: &str,
        p: &Value,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "feishu.ingest" => self.control_feishu_ingest(p),
            "agent.focus" | "room.focus" | "room.create" | "room.rename" | "room.seen"
            | "room.sound" | "room.delete" | "room.history" => {
                self.execute_rooms(method, p, events)
            }
            "state"
            | "diagnostics"
            | "sounds"
            | "settings.color_blind"
            | "settings"
            | "settings.error_orchestrator"
            | "settings.room_sound"
            | "settings.feishu_notifications"
            | "bus.quit"
            | "agent.read" => self.execute_inspect(method, p, events),
            "agent.list"
            | "agent.details"
            | "agent.rename"
            | "agent.delete"
            | "agent.setup-confirm"
            | "agent.add"
            | "agent.resume"
            | "agent.clear"
            | "agent.interrupt" => self.execute_agents(method, p, events),
            "agent.observe" | "agent.press" => self.execute_driver(method, p),
            "message.send" | "message.status" | "message.seen" | "request.recover" => {
                self.execute_messages(method, p, events)
            }
            #[cfg(feature = "dev-tools")]
            "ui.hits" => ui_hits::hits_json(),
            #[cfg(feature = "dev-tools")]
            "ui.hold-acks" => ui_hits::set_ack_hold(true),
            #[cfg(feature = "dev-tools")]
            "ui.release-acks" => ui_hits::set_ack_hold(false),
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
    spec(
        "feishu.ingest",
        "feishu ingest",
        &["app_id", "event"],
        true,
        Tier::Agent,
    ),
    spec("state", "state", &[], false, Tier::Agent),
    spec("agent.list", "agent list", &["room"], false, Tier::Agent),
    spec("settings", "settings", &[], false, Tier::Agent),
    spec(
        "settings.feishu_notifications",
        "settings feishu-notifications",
        &["on"],
        true,
        Tier::Agent,
    ),
    spec("room.create", "room create", &["name"], true, Tier::Agent),
    spec(
        "room.rename",
        "room rename",
        &["room", "name"],
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
    spec(
        "room.history",
        "history",
        &["room", "limit", "before"],
        false,
        Tier::Agent,
    ),
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
        &["agent", "source", "lines", "room", "as"],
        false,
        Tier::Agent,
    ),
    spec(
        "agent.observe",
        "agent observe",
        &["agent", "pane", "cells", "room", "as"],
        false,
        Tier::Agent,
    ),
    spec(
        "agent.press",
        "agent press",
        &[
            "agent",
            "pane",
            "terminal_id",
            "select",
            "text",
            "keys",
            "submit",
            "room",
            "as",
        ],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.clear",
        "agent clear",
        &["agent", "room", "as"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.resume",
        "agent resume --room",
        &["room"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.interrupt",
        "agent interrupt",
        &["agent", "room", "as"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.rename",
        "agent rename",
        &["agent", "name", "room", "as"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.setup-confirm",
        "agent setup-confirm",
        &["agent", "confirm", "room", "as"],
        true,
        Tier::Agent,
    ),
    spec(
        "agent.delete",
        "agent delete",
        &["agent", "confirm", "room", "as"],
        true,
        Tier::Agent,
    ),
    spec(
        "message.send",
        "send",
        &[
            "room", "to", "text", "files", "as", "queue", "reply_to", "caller",
        ],
        true,
        Tier::Agent,
    ),
    spec(
        "message.status",
        "message status",
        &["message", "as"],
        false,
        Tier::Agent,
    ),
    spec(
        "message.seen",
        "message seen",
        &["request", "as", "events", "reply"],
        true,
        Tier::Agent,
    ),
    spec(
        "request.recover",
        "request recover",
        &["request", "confirm", "as"],
        true,
        Tier::Agent,
    ),
    // Dev tier: the developer's view and preferences, and Bus's own internals.
    spec("room.focus", "room focus", &["room"], true, Tier::Dev),
    spec("room.seen", "room seen", &["room"], true, Tier::Dev),
    spec(
        "room.sound",
        "room sound",
        &["room", "on", "sound"],
        true,
        Tier::Dev,
    ),
    spec(
        "agent.focus",
        "agent focus",
        &["agent", "room", "as"],
        true,
        Tier::Dev,
    ),
    spec(
        "agent.details",
        "agent details",
        &["agent", "on", "room", "as"],
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
    spec(
        "settings.error_orchestrator",
        "settings --error-orchestrator",
        &["name"],
        true,
        Tier::Dev,
    ),
    spec("sounds", "sounds", &[], false, Tier::Dev),
    spec("bus.quit", "quit", &[], true, Tier::Dev),
    spec("diagnostics", "diagnostics", &[], false, Tier::Dev),
    // The TUI driver's named click targets and deterministic acknowledgement race.
    #[cfg(feature = "dev-tools")]
    spec("ui.hits", "ui hits", &[], false, Tier::Dev),
    #[cfg(feature = "dev-tools")]
    spec("ui.hold-acks", "ui hold-acks", &[], true, Tier::Dev),
    #[cfg(feature = "dev-tools")]
    spec("ui.release-acks", "ui release-acks", &[], true, Tier::Dev),
];

/// Every place `text` names a dev-tier command the way docs and help do
/// (`bus room focus`, `` `room focus` ``, or a help line). Ordinary prose
/// about diagnostics does not name the dev-tier command.
#[cfg(test)]
pub(crate) fn dev_tier_mentions(text: &str) -> Vec<String> {
    METHODS
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
        .collect()
}

pub(crate) fn method_spec(method: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|spec| spec.method == method)
}
