//! Long help attached to the Clap grammar; no separate CLI reference to maintain.
use clap::{Arg, ArgAction};

pub(super) const CONTROL: &str = r#"JSON and common errors:
  Control commands print exactly one JSON object, including failures:
  {"id":"request identity","ok":true,"result":{...},"error":null}
  Failure: {"id":"...","ok":false,"result":null,
            "error":{"code":"...","message":"actionable explanation"}}
  Some failures retain diagnostics or the last status in result. Require ok:true.
  Exit 0 means success, 1 failure.
  Help (-h or --help) prints text, exits 0, and never connects to a session.
  --request-id defaults to a generated unique ID (1-128 bytes). Reuse an ID with
  identical parameters for mutation retries while this process retains its receipt
  (10 minutes, bounded capacity); receipts do not survive server restart.
  ROOM accepts a name or ID, or master (case-insensitive). AGENT names/IDs resolve
  globally; an existing numeric ID wins over a numeric name. Optional --room/--as
  on agent selectors provide conversation context, never a name scope; an exact
  target ID does not need that optional author context resolved.

  invalid_arguments: correct the flags using this help; nothing was submitted.
  invalid_request / invalid_params: check request ID length or client/server version.
  id_conflict: reuse the original parameters, or use a new ID for a new operation.
  command_failed: inspect error.message and state; fix the named selector, provider,
    hook consent, identity, dialog or storage problem before retrying.
  recipient_deleted: that agent no longer exists and the message was not sent.
    The error names the recipient. Nothing was recorded. An agent deleted while
    its message is already queued still settles that request as agent_deleted.
  confirmation_required: inspect the target, then explicitly supply --confirm.
  control_unavailable / transport_error: check the running session and absolute
    BUS_DATA_DIR (otherwise the last opened session is targeted). Control never
    starts Bus. result.applied:false means not delivered; "unknown" may have run.
  response_incomplete / response_timeout / coordinator_timeout: inspect state and
    the retained outcome before retrying a mutation with the SAME request ID.
  server_busy / coordinator_busy: back off; server_busy is retried automatically
    up to five times for a one-shot command. Waits retry overload until their deadline.
  coordinator_unavailable: restore the intended running Bus before retrying.
  transport_error also wraps client-side request_too_large/request_timeout reasons
    in error.message: reduce payloads above 256 KiB or retry the same request ID
    after a send deadline. These prefixes are not separate CLI error codes.
  response_too_large: reduce history --limit (16 MiB response maximum); otherwise
    narrow the requested data. Inspect mutations before retrying.
  storage_unavailable: repair the reported storage problem before mutating again.
  receipt_capacity: wait for old receipts to expire or restart when safe, then
    inspect durable state before retrying because restart discards receipts.
  dev_tools_disabled: this tool needs a session launched with bus --dev.
  unknown_method: align the CLI and running server versions, restarting when safe."#;

pub(super) fn default_value(path: &str, id: &str, arg: &Arg) -> Option<&'static str> {
    if arg.is_required_set() {
        return Some("required; no default");
    }
    // Clap renders declared value defaults; only document implicit behavior here.
    if !matches!(arg.get_action(), ArgAction::SetTrue) && !arg.get_default_values().is_empty() {
        return None;
    }
    Some(match (path, id) {
        (_, "request-id") => "default: generated unique ID",
        ("agent list", "room") => "default: all rooms",
        ("agent interrupt", "room") => "default with AGENT: target's home room",
        (p, "room") if p.starts_with("agent ") && p != "agent add" && p != "agent resume" => {
            "default: none; optional conversation context"
        }
        ("send", "as") => "default: infer agent caller; developer outside agent panes",
        ("agent interrupt", "as") | ("request recover", "as") => "default: developer",
        (_, "as") => "default: no author context or waiter lease",
        (_, "args") => "default: empty string",
        (_, "orchestrates") => "default: none; required in MASTER",
        (_, "system-prompt") | (_, "system-prompt-file") => "default: built-in orchestrator prompt",
        (_, "source") => "default: recent; requires --lines",
        (_, "lines") => "no default; required for recent, forbidden for visible",
        (_, "limit") => "default: entire history",
        (_, "before") => "default: no cursor; requires --limit",
        (_, "file") => "default: no attachments",
        ("send", "room" | "to") => "required unless --reply-to; no default",
        ("send", "text") => "required; no default",
        (_, "reply-to") => "default: ordinary new message",
        ("agent interrupt", "agent") => "required unless --room; no default",
        ("agent interrupt", "text") => "default: no follow-up message",
        (_, "sound") => "default: keep current sound",
        _ if matches!(arg.get_action(), ArgAction::SetTrue) => "default: false",
        _ => "no default; see command requirements",
    })
}

pub(super) fn command(path: &str) -> String {
    let text = match path {
        "" => {
            "A plain bus launch starts a new interactive session. Control commands target an\nexisting session and return JSON. Use bus <command> --help for its contract.\n--paths returns a JSON object of data/log/config/state paths without starting Bus.\nUse bus sessions and bus resume --help before selecting a saved session."
        }
        "room" | "agent" | "message" | "request" | "ui" | "feishu" => {
            "Choose a subcommand below, then run bus <command> <subcommand> --help.\nA bare group prints help without contacting Bus; it has no JSON result."
        }
        "state" => STATE,
        "feishu ingest" => {
            "Read exactly one feishu-bridge v1 JSON message from stdin (maximum 128 KiB).\n--instance must be an absolute session root or registry base; --app-id must match\nthe configured bridge app. This sink bypasses inherited caller identity and the\nlast-opened session. Registry replies must identify exactly one saved session.\nResult: a control JSON acknowledgement after durable delivery or parking; exit 0\nacknowledges acceptance. Errors: invalid input, unknown/ambiguous registry mapping,\nan offline target or a failed save exit nonzero so the bridge retains its spool.\nNo listener, Feishu send, Bus launch or UI error is created by this command."
        }
        "room create" => ROOM_CREATE,
        "room rename" => ROOM_RENAME,
        "room delete" => ROOM_DELETE,
        "agent delete" => AGENT_DELETE,
        "agent add" => AGENT_ADD,
        "agent list" => AGENT_LIST,
        "agent rename" => AGENT_RENAME,
        "agent setup-confirm" => AGENT_SETUP_CONFIRM,
        "agent resume" => AGENT_RESUME,
        "agent read" => AGENT_READ,
        "agent observe" => {
            "Read the complete terminal viewport as text, focus, dimensions and cursor.\nAdd --cells to include the full styled cell grid.\nWithout --pane, TARGET is a managed agent name or ID, including during launch.\nWith --pane, TARGET is a terminal or pane ID; no agent process is required.\nResult: {terminal_id,pane_id,content_revision,text,focus,cols,rows,cursor,grid?}.\nErrors: command_failed for an unavailable target or a screen being redrawn;\nobserve again. No keys are sent. The terminal_id can bind a later press."
        }
        "agent press" => {
            "Drive the target's current terminal prompt, independently of provider or dialog parsers.\nBefore every action, run agent observe and decide from the fresh screen.\n--select LABEL matches a complete visible option row, moves to it, observes the\nfocus after each navigation key, then confirms with Enter. Choose one input form:\n--select, or any combination of --text, repeated --key and --submit. Text is typed\nfirst, then keys in their given order; only --submit appends Enter. --key enter\nis also an explicit Enter. Numbers, shell y/n answers and control chords work.\n--pane targets a terminal or pane ID, including a plain shell or pager.\nUse --pane on both observe and press: bus agent observe w1:p2 --pane, then\nbus agent press w1:p2 --pane --key n.\n--terminal-id binds input to the identity returned by agent observe.\nResult: {keys,text_written,selection_pending,observation,outcome,reason?}.\nThe observation contains text, focus, dimensions, cursor and terminal identity;\nuse agent observe --cells when styled cells are needed.\nOutcome sent means input was queued, selected means the label was focused and\nEnter was queued; neither proves what the terminal program subsequently did.\nObserve the resulting screen to verify the program's response.\nA selection_pending or unknown outcome preserves keys already sent and the\nlatest observation. Inspect it before continuing. There is no implicit resend.\nErrors: command_failed for invalid input, a missing target, mismatched terminal\nidentity, unknown key, or an unresolvable option/focus. Read the screen and use\nexplicit keys when its menu has no unique visible focus."
        }
        "agent clear" => AGENT_CLEAR,
        "agent interrupt" => AGENT_INTERRUPT,
        "send" => SEND,
        "message status" => STATUS,
        "wait" => WAIT,
        "message seen" => MESSAGE_SEEN,
        "request recover" => REQUEST_RECOVER,
        "history" => HISTORY,
        "settings" => SETTINGS,
        "settings feishu-notifications" => SETTINGS_FEISHU_NOTIFICATIONS,
        "settings color-blind" => {
            "Set the shared palette; exactly one of --on/--off is required.\nResult: {updated:true,color_blind_mode:BOOL}. Errors: common errors below."
        }
        "settings room-sound" => {
            "Set every work room's sound switch and optionally sound name, plus defaults\nfor new rooms. MASTER is separate. Exactly one of --on/--off is required.\nResult: {updated:true,room_sound:{...}}. Errors: command_failed for unknown sound;\nrun bus --dev sounds. Omitted --sound preserves the name."
        }
        "room sound" => {
            "Set one room's sound switch and optionally name; exactly one of --on/--off\nis required. Result: {updated:true}. Errors: command_failed for invalid room or\nunknown sound; inspect state and bus --dev sounds. Omitted --sound keeps its name."
        }
        "room seen" => {
            "Clear a room's unread markers. Result: {updated:true}.\nErrors: command_failed for invalid/ambiguous room; inspect state and use its ID."
        }
        "room focus" | "agent focus" => {
            "Enqueue navigation in the attached UI (not task completion).\nResult: {stage:queued,room_id,agent_id?}. Errors: command_failed for an unavailable\nUI event channel or agent terminal; inspect state and the attached UI."
        }
        "agent details" => {
            "Set sidebar details visibility; one of --on/--off is required.\nResult: {updated:true}. Errors: command_failed for invalid agent; use agent list."
        }
        "quit" => {
            "Enqueue quitting the attached UI. Result: {stage:queued}; this is not confirmation\nthat shutdown finished. Errors: command_failed if the UI channel is unavailable."
        }
        "sounds" => {
            "List Default and installed OS notification sounds.\nResult: {sounds:[{name:STRING,path:STRING|null},...]}; Default has null path.\nErrors: common errors below. Use names with room sound/settings room-sound."
        }
        "diagnostics" => {
            "Read internal session diagnostics. Result: {version,dev,storage_failed,\nstorage_repair_required,coordinator_error,data_dir,logs,callback_logs,\nagents:[{agent_id,name,status,reason,detail,identity,current_request}]}.\nErrors: common errors below; inspect storage/coordinator errors before mutations."
        }
        "ui hits" => {
            "Read named TUI click targets for the dev driver.\nResult: {size:[COLS,ROWS],hits:[{name,row,col,width,height},...],\nholding_acknowledgements,held_acknowledgements}; coordinates are terminal cells.\nSee bus --dev tui --help for the driver contract.\nErrors: command_failed when the UI snapshot is unavailable; common errors below."
        }
        "ui hold-acks" | "ui release-acks" => {
            "Hold or release RoomCreated, AgentAdded and DevFocusRequested before UI dispatch.\nFor deterministic scratch-TUI ordering tests only. `ui hits` reports the held count.\nResult: {holding:BOOL,held:NUMBER}. Errors: common errors below."
        }
        _ => "Missing command documentation",
    };
    if matches!(path, "room delete" | "agent delete") {
        format!("{text}\n\n{DELETION}")
    } else {
        text.into()
    }
}

const STATE: &str = r#"Read the current session snapshot without acknowledging replies.
Result: {revision,master_room,visible_room,rooms:[...],agents:[...],usage,settings,build,launch}.
  master_room is MASTER's ID; visible_room is null while a terminal/form is open.
  rooms: {id,name,kind,unread_count,unseen_to_developer,status,sound,sound_name,
    deletion_pending,orchestrator}. kind is master/work; orchestrator is ID/null.
    status is blocked (including unavailable/dialog), else working, else idle.
    unread_count counts hidden-room peer messages/replies/notices; the persisted
    unseen_to_developer dot tracks messages to You. Opening the room clears them.
  agents: stored identity/options plus room_id,orchestrates,status,wait_reason,
    actionable_error,runtime_identity,current_request,dialog,details_disclosed,
    deletion_pending,queue_depth,oldest_queued_age_ms,compactions:{count,last_at_ms}.
    wait_reason is null when delivery can proceed; otherwise session_hook_missing,
    prior_request_active,agent_unavailable, etc. dialog makes shown status blocked.
  usage: claude/codex/cursor, status observed or unknown (never assume unused).
    Observed five_hour/weekly windows: used_percent,resets_at,window_minutes,
    read_at_ms,observed_by_agent. Codex is read after turns; Claude's status-line
    tap runs at most once/second, preserves configured stdin/stdout (2s limit),
    reuses launch settings on resume, expires after 15m/reset and needs a fresh
    observation after restart. Managed settings can override it; Cursor is unknown.
  exit_snapshot on agents: path,tail,at_ms,launch_id,terminal_id,error,terminal_closed
    (true closed, false left open, null not attempted). Read that file after exit.
  build:{client:{profile:debug|release,binary,sha},server:{binary,sha,pid}|null}.
    client is the coordinator/UI executable; server is the native server recorded for this
    data directory. server is null when that record is missing, corrupt, or stale.
  launch:{dev,yolo} is how the client/coordinator process was started.
  settings is the shared settings snapshot.
Errors: common errors below; inspect actionable_error for queued/failed agents."#;

const ROOM_CREATE: &str = r#"Create a work room. MASTER already exists and its name is reserved.
Result: {room_id:NUMBER}. Capture the ID before adding workers or its orchestrator.
Errors: command_failed for invalid/reserved names; choose another name."#;

const ROOM_RENAME: &str = r#"Rename a work room; IDs and history remain stable. MASTER cannot be renamed.
Result: {updated:true}.
Errors: command_failed for unknown/ambiguous room or reserved name; inspect state,
use a numeric ID and choose a work-room name other than MASTER."#;

const ROOM_DELETE: &str =
    "Delete a work room and its MASTER orchestrator. MASTER cannot be deleted.
See the shared deletion contract below.";
const AGENT_DELETE: &str = "Delete an agent and close its still-owned terminal.
See the shared deletion contract below.";
const DELETION: &str = r#"Durably remove the selected resource and close terminals still attributed to it.
Messages/requests remain queryable with history and message status, without a
seven-day expiry. Resolve the target with state and prefer its numeric ID.
Result: {updated:true,terminals_left_open?:[{agent_id,agent_name,pane_id,
  terminal_id,reason:provider_exited|session_moved,message},...]}. message explains
  why it stayed open and the action to take.
  A terminal shared/reassigned or refused by the native server is left open and
  listed; inspect it before closing manually. A provider's leftover shell can
  be closed too, even if it is running a command. Removal is saved and published
  before success. A timeout does not cancel deletion.
Errors: command_failed for close/persistence failure or invalid target; inspect
state deletion_pending and actionable_error. Retry the SAME --request-id to
retrieve the retained outcome. --confirm is required (default false)."#;

const AGENT_ADD: &str = r#"Launch a worker in a work room, or an orchestrator in MASTER. Current names are
case-sensitive, trimmed and unique session-wide; a fully deleted name is reusable.
Providers: claude, claude-ds, codex, cursor, grok, kimi. --pwd is the project directory; --args is one
provider-argument string. Bus disables startup updates on launches/resumes;
explicit Claude updater environment settings take precedence.
Result: {agent_id:NUMBER,stage:"launching"}; launch success is not task completion.
Errors: command_failed carries setup/provider/validation diagnostics. Inspect the
reported project hook path before using --consent-hooks, or setup-confirm for an
existing pending agent. Missing provider binaries retain saved agents/tasks; fix
the installation and use agent resume --room ROOM. Never blindly duplicate add.
A missing provider command leaves the agent Unavailable and logs the binary
with room/provider context; affected agents and tasks stay saved.

MASTER requires --orchestrates ROOM (one orchestrator per work room, binding for
life); --orchestrates and custom system prompts are MASTER-only. Claude Code,
Codex, Cursor, Grok and Kimi can orchestrate. Codex cannot wake on background
command completion; use foreground bounded waits or ordinary worker messages.
Default prompt is built in (orchestration/prompt.md); --system-prompt TEXT or
--system-prompt-file PATH replaces it (mutually exclusive). Bus fills {{ROOM_NAME}},
{{ROOM_ID}}, {{AGENT_NAME}}, {{DOCS}} (the session's <BUS_DATA_DIR>/docs/) and
{{WORKFLOWS_DIR}} (the resolved workflow directory). The docs contain the guides,
workflow-create.md, templates and shipped workflows; see the named-workflows rule.
Bus saves system-prompt.md in the launch's callback folder and passes the prompt
again on resume. --args combines with Bus-owned prompt arguments.
Claude gets --append-system-prompt-file; saved Codex gets a developer_instructions
pointer via -c; Cursor and Grok receive a first message. Kimi uses an agent
file for new sessions and retains it on exact --session resume. Its hooks and
session files live in an owner-only per-agent home. New PWD trust prompts require
an explicit answer; Bus does not pre-trust folders.

Adopt an existing session with --args '--resume UUID' (Claude/Cursor) or
--args 'resume UUID' (Codex workers). Quit it elsewhere first, use its original
PWD, and supply exactly one UUID, never a picker/--continue/--last/--fork-session.
A session bound to another Bus agent is refused. Find IDs with provider /status
or cursor-agent ls. Hooks bind the adopted session; Cursor binds from launch
args because resume has no start hook. Claude resumed orchestrators also get
--system-prompt-snapshot off so the new orchestrator prompt applies."#;

const AGENT_LIST: &str = r#"Map current names to stable IDs and home rooms; optionally filter one room.
Result: {agents:[{name:STRING,id:NUMBER,room:NUMBER},...]} (empty array is valid).
Errors: command_failed for an unknown/ambiguous --room; use state and a numeric ID."#;

const AGENT_RENAME: &str = r#"Rename a current agent without changing its ID, provider session or requests.
Result: {updated:true}.
Errors: command_failed for a name already held by a current agent (diagnostic
names its ID/room), blank/reserved name or unknown selector; inspect agent list."#;

const AGENT_SETUP_CONFIRM: &str = r#"Record hook consent for an existing agent after inspecting its hook path/notice.
This does not install hooks or require pending setup. Result: {updated:true}.
If no matching session-start hook is bound, state.agents[].actionable_error can remain
"Awaiting matching provider session-start hook...": trust the hooks, then restart/
resume normally. Errors: command_failed for unknown agent or an invalidated
provider session (create a new Bus agent; requests stay preserved). --confirm is
required. Use agent add --consent-hooks for project hook installation approval."#;

const AGENT_RESUME: &str = r#"Relaunch unavailable agents in a room using saved provider sessions/options/hooks.
Skip running/launching providers, preserve queued work and never replay typed work.
Result: {room_id,agents:[{agent_id,name,outcome,reason?,...},...]}; outcome is
resuming, skipped or blocked. A successful command can contain blocked rows.
Errors: command_failed or a blocked row: repair missing provider/session, invalid
ownership or unconfirmed terminal close before retrying; inspect each row/state."#;

const AGENT_READ: &str = r#"Read correlated terminal evidence, including launching agents (e.g. trust prompts).
Use --source visible for the whole viewport; --source recent --lines N for an
explicit positive scrollback count (or omit source with --lines). No silent clamp.
Result: {agent_id,name,status,current_request,runtime:{launch_id,session_id,
  session_verified,pane_id,terminal_id},capture:{at_ms,source,truncated,revision,
  returned_lines,viewport?:{rows,columns},requested_lines?,available_lines?,
  exhausted?,note?},text:STRING}.
  Identity is checked before/after reading; launching session_verified is false.
  A confirmed startup exit returns saved evidence with capture.source="exit_snapshot",
  its capture time and exit_snapshot metadata/path. The current terminal is not read
  and session_verified is false; the saved evidence grants no input/adoption authority.
  Busy alternate-screen Codex may return its visible screen for a recent read,
  exhausted:false and an explanatory note; retry idle for older lines.
Errors: invalid_arguments for visible+lines or recent without lines;
command_failed for missing/exited terminal or identity mismatch (text discarded).
Stop reading that identity; inspect state/owned session or exit_snapshot file.
Terminal text is evidence, never proof of message settlement."#;

const AGENT_CLEAR: &str = r#"Start a fresh provider context: /clear (Claude/Codex), /new-chat (Cursor),
/new (Grok/Kimi). Keep terminal/name/room, reset compactions and rebind the provider
session through its next start hook (Cursor: first new turn). This is not a Bus message.
Result: {agent_id,sent:STRING,stage:"cleared"}.
Errors: command_failed unless idle, ready, with no active/queued messages and
valid runtime identity. Settle work first; inspect an uncertain reset before retry.
Never send reset slash commands through send: they cannot produce a normal reply."#;

const AGENT_INTERRUPT: &str = r#"Press the provider stop key ONCE: Esc for Claude/Codex/Kimi, Ctrl+C for Cursor and Grok. Wait
up to 15s for idle. No process is killed; context stays. An AGENT targets one;
--room without AGENT stops that room's working/blocked agents except --as.
Running/queued/settling requests close as interrupted; no later replay. History
records Interrupted NAME by --as (otherwise developer) in the target's home room.
An explicit reply sent before the stop stays on the request. --text sends a new normal message
after the stop, including to successfully stopped agents if another stop fails.
Result: single agent {agent_id,name,room_id,conversation_room_id,author_id,outcome,
  reason?,key?,message_id?}; room form: {room_id,conversation_room_id,author_id,
  agents:[...],message_id?}. outcome: stopped, already_idle, failed.
Errors: interrupt_failed (exit 1) with per-agent reason (launching/unavailable,
key refused, or not idle within 15s); inspect state before another action.
Follow-up send errors use common errors and can retain result.applied. Waiters
on interrupted work return message_interrupted. Use send to steer without stopping."#;

const MESSAGE_SEEN: &str = r#"Acknowledge a reply to your own request only AFTER reading its output.
Uses the recipient request ID, not the message ID. Seeing a shared final covers
only this author's answered group members. Anonymous status/waits never suppress
terminal delivery.
Result: {request_id,seen:true}.
Errors: command_failed for unknown request, wrong author or no completed reply;
inspect message status, read the answer, and use its owning author ID."#;

const REQUEST_RECOVER: &str = r#"Explicitly close a stuck recipient request without replaying it. Queued untyped
requests close individually, regardless of agent status. Already abandoned is
idempotent. Typed current work requires confirmed idle, or unavailable with
session_invalidated; recover a joined group's lead (the group request ID).
Result: {request_id,agent_id,stage:"abandoned"}. Preserve other queued work.
Errors: command_failed for unknown/non-lead/currently working typed request;
inspect message status and current identity/revision facts, then recover the lead
only when safe. Replace an invalidated agent before resending. Recovery does not
undo task effects. --confirm is required. Newly closed requests retain reason
manual_recovery (unless a prior cause exists) and manual_recovery:{actor,at_ms}.
--as NAME_OR_ID records an agent actor; omitted records the developer. Native
failure detail and earlier completed replies remain unchanged."#;

const HISTORY: &str = r#"Read durable room history, including deleted agents/rooms and context clears.
Default is the full history. --limit N takes newest N in chronological order;
--before ID is exclusive, requires --limit. Reduce N to fit the 16 MiB response.
Result: {room_id,room_name,deleted,messages:[{prompt,author_name,recipient_names,
  delivery}],has_more?,next_before?}. prompt holds id,author,text,files,
  recipient_ids,submitted_at_ms; delivery is null for notices. Otherwise it is
  message status with a history projection: each shared reply body appears once
  per room. Follower request rows set reply:null and
  answered_with_message:LEAD_MESSAGE_ID. Use message status to read every
  request's full reply.
  Pagination adds has_more/next_before; pass next_before for the older page.
  Text is raw Markdown; file/image paths are absolute. Deleted agent labels end
  in [deleted]; room_name stays plain and deleted:true marks a deleted room.
  Numeric deleted-room IDs work; live names win, deleted names are
  an unambiguous fallback. No seven-day expiry; see docs/message-history.md.
Errors: invalid_arguments for nonpositive limit/before or before without limit;
command_failed for unknown/ambiguous room (use ID); response_too_large: lower N."#;

const SETTINGS: &str = r#"Read settings shared across normal sessions, or select a setting subcommand.
Result: {settings:{color_blind_mode,master_sound,room_sound,developer_sound:
  {enabled,name},feishu,...}}. Sound choices store names, not paths; null developer
  name means System default. Read state for each running room's effective sound.
  Registry settings.json is beside the session registry. An isolated BUS_DATA_DIR
  without a registry session uses its own settings.json, never the shared file.
  MASTER takes master_sound on launch/resume; already-running instances keep it
  until restart. New work rooms take room_sound. master_sound is a non-null
  {enabled:true,name:null} by default (null name means Default); old settings load
  through the current defaults. There is no first-launch seeding step.
Errors: common errors; settings.error reports a settings-file read failure."#;

const SETTINGS_FEISHU_NOTIFICATIONS: &str = r#"Inspect mandatory Feishu delivery by requesting --on; --off is rejected because
developer blocker notifications remain mandatory even with a legacy off setting.
One of --on/--off is required. --on waits up to 15s for notification setup.
Result: {feishu:{enabled,pending,error,...}} (status from the shared settings).
Errors: feishu_unavailable: fix the reported bot/recipient/CLI setup;
feishu_timeout: inspect settings and retry after setup completes; command_failed
for --off. control_unavailable can occur while reading the completion status.
See docs/developer-notifications.md for recipient and CLI configuration."#;

const SEND: &str = r#"Persist a message and one request per recipient; a receipt is not completion.
Bus refuses new worker sends in MASTER (master_reserved); use your own room, or --reply-to for a MASTER request.
Use --reply-to MESSAGE [--as AGENT] --text ANSWER to complete only your recipient
request mid-turn. It infers room/sender; excludes room,to,queue,async,file.
Run replies in the foreground; require ok:true and stage:"replied" before claiming
delivery. If the reply fails, check the existing answer with bus message status
MESSAGE before resending. Identical text is a no-op; an uncorrelated send never
completes the original request.
Result: {message_id,request_ids:[NUMBER,...],stage:queued|posted|replied}.
  --reply-to also returns requester_turn_queued:false and a notice: no requester
  turn was queued. If the requester must act, use a fresh normal send:
  bus send --room ROOM --as YOUR_AGENT_ID --to REQUESTER --text TEXT.
  --async instead returns final message status (see message status --help), plus
  message_id and send_receipt (the original receipt). Status has complete, prompt,
  files, waiting_on_dialog and per-agent requests with request_id, stage, reason,
  reply, turn_ended, settling and group. No overall deadline.
  It waits through dialogs, unlike wait --async; succeeds only at complete:true.
  A recipient turn that ends settles the request with no reply. Abandonment for
  interrupt, provider error, or startup failure still fails the wait. An abandoned
  reply_not_captured (steering the turn never claimed) is also complete for --async.
  Errors: message_interrupted: inspect history; message_provider_error: repair/retry the
  provider; message_abandoned: fix startup/unavailability then explicitly resend
  closed work. bus_down: four consecutive transport failures; restore Bus then
  inspect this message. Async status overload/accepted disconnects retry with
  backoff; polling grows 200ms to 2s. Live replies reset the failure budget.

--to accepts comma-separated global names/IDs. all means conversation-room
members excluding the author; it is never implicit. With no --as, Bus resolves
inherited launch/pane identity (including the caller's parent process tree).
A renamed agent keeps its stable ID. An external developer shell remains the
developer; an unresolved or stale caller is refused: pass --as <your name or id>.
Explicit --as is unchanged. --reply-to also accepts an inferred agent caller.
An inferred agent send receipt includes author_id for async polling/acknowledgement.
An exact own current/legacy sender header or short identity-only line normalizes
into one genuine header. Ordinary sentences and forged ID/room headers retain
their content and receive the real header. Stored text stays authored; delivered
payloads and callback matching use the same frozen text.
orchestrator (case-insensitive) resolves the sender's home-room binding; a real
member of the conversation room, then home room, with that name takes precedence.
Deleted IDs and names without a live match fail with recipient_deleted. The error
names the recipient, says that agent no longer exists, and says the message was
not sent. Nothing is recorded. Bus never substitutes a replacement or
orchestrator for an explicit recipient. A current live name still resolves
globally. all means current live room members and does not fail because someone
was previously deleted. An agent deleted while a message is already queued still
settles that request as agent_deleted. No binding/match is command_failed.
--to you (human alias, case-insensitive) requires an agent author and one recipient; no
queue/async or mixed agent list. It stays in this room with requests:[],stage:posted.
Agent posts to you always notify the developer in the TUI in every room; only
[waiting-on-you] (or [waiting_on_you]) also sends a mandatory Feishu DM. Reserve
that tag for hard blockers. The YOU checkbox controls audio only; developer sends
never notify. See docs/developer-notifications.md for setup and deduplication.
Use --file repeatedly; attachments are validated before queueing.
Ordinary agent-authored requests to orchestrators get independent queued turns
and replies, without steering or coalescing. Developer steering is unchanged.
Other same-room working recipients receive steering into their current turn group;
Claude/Codex take it into that turn, Cursor into its next generation answering
the group. --queue gives its own idle turn/reply and never joins. Cross-room
messages wait for a new turn. Blocked/launching recipients queue; accumulated
ordinary messages form one ordered prompt/group with batch position/time headers.
An explicit --reply-to settles that request while the turn stays open and does not
release the queue. The turn's final text is not a reply and does not replace an
earlier explicit reply. When the turn ends, every still-open request in it settles
and the queue advances. Raw text/files remain queryable after restart.
If you want an answer, say whom to reply to and in which room. If that agent no
longer exists on Bus when you reply, ignore it; do not retry and do not treat it
as an error. Bus does not forward a turn final as "Reply to message N".
An explicit or inferred agent author async waiter renews a 3s lease deferring its
orchestrator outcome wakes; after stdout flush only printed event_acks are
acknowledged. Killed waiters lose the lease; anonymous waits/status never
suppress outcome delivery. Explicit replies never queue a requester turn.

Errors: recipient_deleted means that agent no longer exists and the message was
not sent; name a live recipient. request_already_replied means different text
was NOT sent; inspect result
{message_id,request_ids,request_complete:true,new_text_sent:false,existing_reply:
{source:recipient_reply|captured_turn_final,received_at_ms,preview},inspect_command}.
Preview is at most 160 characters plus ellipsis. Read full message status before
resending. command_failed covers invalid/self/empty recipients, bad attachments,
missing --as for you, missing hooks or storage. Fix the named cause first."#;

const STATUS: &str = r#"Read durable settlement once, without waiting or acknowledging delivery.
Result: {message_id,prompt,files,complete,waiting_on_dialog:[AGENT_ID,...],requests:
  [{request_id,agent_id,agent_name,deleted,stage,reason,manual_recovery,status,detail,
  uncertain_outcome,session_id,turn_id,start_bound,dialog,turn_ended,settling,group,
  queue,latency,event_acks,events:[{key,text}],reply_ack,exit_snapshot,reply}]}.
  prompt includes author/text/files/recipients/time; files are absolute paths.
  reply is null or {text,received_at_ms,...}, with raw Markdown.
  manual_recovery is null or {actor:"human"|{agent:AGENT_ID},at_ms:UNIX_MS};
  reason and detail retain any earlier native failure cause and error.
  Notices have stage:posted,requests:[],complete:true. Deleted requests survive.
  complete is true when every recipient request has settled: an explicit reply, a
  turn that ended, or abandonment.
  turn_ended needs trusted Working/Blocked then Idle, a hook-bound turn start/end,
  or a final hook; explicit replies can finish while turn_ended remains false.
  settling holds an ended turn briefly so a late error or final hook can land;
  group is the lead request ID for joined steering and is not a history badge.
  latency contains nullable Unix-ms clocks for accepted, queued, eligible, picked,
  dispatch preparation, native paste/Enter writes and provider_started_at_ms.
  Native write completion is not provider acceptance; provider_started_at_ms is
  the correlated start hook. Missing clocks mean that phase was not observed.
  events retains exits and group summaries; event_acks lists acknowledgeable
  unseen own outcomes; reply_ack stays false because nothing waits on a reply wake.
Stages: queued persisted; submitting native write; awaiting_start typed but no
trusted turn; delivered trusted start; joined same turn; replied explicit
--reply-to; ended the turn finished with no reply; abandoned closed, will not
run later. Queued untyped work may close abandoned after
2m not working (input_box_not_empty, agent_not_ready/blocked, session_hook_missing,
prior_request_active, unavailable). Recovery deadlines stay internal and never
change the reported stage. After 5m blocked the reason is blocked_unanswered.
A turn that ends settles as ended, with no reply. The final text is not stored
as a reply.
Transient native provider errors retain the delivery and show Working while Bus retries
with backoff or waits for a quota reset; details and deadlines go to the client log.
Permanent-error settlement without a final closes as provider_error: MESSAGE.
An agent that is gone settles as agent_unavailable. Unclaimed steering that the
turn did not include can close as reply_not_captured. A turn that simply ends
does not. Conclusive pre-delivery startup failure is agent_startup_failed.
Bus can abandon unclaimed steering after an autonomous turn ends and another
starts or stays idle 5s; it releases later queued work. Late finals keep references.
submission_unconfirmed may have run: inspect transcript/composer before resending
and clear remaining drafts. Guarded Enter retries never repaste uncertain writes;
cleanup only clears an unchanged Bus draft and reports unconfirmed cleanup.
Errors: command_failed for unknown/nonnumeric message; use the send receipt ID.
Open and abandoned statuses themselves are successful reads (exit 0)."#;

const WAIT: &str = r#"Poll the same durable message status used by message status (see its --help for
the result schema/stages). Bounded wait polls every 200ms, timeout 1-600s, default
60. --async has no deadline, backs off 200ms to 2s, and forbids explicit --timeout.
Result: {message_id,prompt,files,complete,waiting_on_dialog:[AGENT_ID,...],
  requests:[{request_id,agent_id,agent_name,stage,reason,reply,turn_ended,settling,
  group,...}]}; reply holds raw text and received_at_ms. See message status help
  for every request field. Both stop for a dialog; send --async waits through it.
Success requires complete:true. A turn that ends is complete for both bounded
and async waits, with or without an explicit reply. An abandoned
reply_not_captured fails a bounded wait and is accepted by an async wait.
Interrupt and provider-error abandonment fail both. Result is the full last
status, including raw Markdown replies/files and per-recipient progress.
--as owning author renews a 3s waiter lease and acknowledges only printed
event_acks after stdout flush, deferring duplicate outcome wakes while alive.
Anonymous waits and message status never suppress outcome delivery. Killing the
waiter leaves the request intact and its unseen events eligible for delivery.
Explicit replies are stored for status/wait reads; they never wake the requester.
Errors: timeout (exit 1): retain result and continue inspecting; it does not
cancel work. agent_waiting_on_dialog: use agent observe and agent press, then wait again.
message_abandoned: inspect reason; repair startup/provider and explicitly resend
only closed work after checking effects. message_provider_error: a non-transient provider failure ended the turn,
worth retrying after repair. message_interrupted: inspect history for who stopped it.
bus_down: async only, four consecutive transport failures; restore session and
resume wait. Bounded transport failures report immediately; accepted disconnects
and live overload back off/retry (a live response resets the async failure budget).
command_failed for unknown message/author: use the captured message/agent ID."#;

pub(super) const SESSIONS: &str = "List saved local sessions without starting one.\nOutput: text rows of session IDs, room names, recent activity and last-opened marker\n(not a control JSON envelope). --dev/--yolo default false and do not launch agents.\nErrors: exit 1 with a registry/read error on stderr; repair the reported path.";
pub(super) const RESUME: &str = "Resume exactly one saved session ID, or --last (default false).\nStarts the interactive UI; no control JSON result. --dev/--yolo default false,\napply to this run only. Cannot combine resume with BUS_DATA_DIR.\nAgents resume into the conversation last bound by their hooks. Adding agents\nin the same project may move .codex/hooks.json/.cursor/hooks.json to another\nBus executable; resume moves existing hooks back, never recreates removed ones\n(the agent stays suspended until added again).\nErrors: exit 1 on missing session/registry or BUS_DATA_DIR conflict; list sessions\nfirst, correct the target/environment, then retry. A trust prompt needs an answer.";
pub(super) const STOP: &str = "Stop the selected session server and all its agent panes.\nTargets absolute BUS_DATA_DIR, otherwise the last opened session.\nOutput JSON (no control envelope): {\"stopped\":true}, or false if none was running.\nQuit an attached UI with Ctrl+Q first so it saves drafts; stop disconnects it\nwithout saving. --dev/--yolo default false and do not change the target.\nErrors: exit 1 with a server/IO error on stderr; inspect the target and processes\nbefore retrying. Never use stop to probe a session you did not intend to stop.";

pub(super) const RESTART: &str = "Save and quit the whole Bus, relaunch another bus binary, and keep this session.\nRun `bus --dev restart --confirm` to build the checkout that owns this executable (`cargo build --locked --bin bus`, plus `--features dev-tools` when this build has it) and relaunch that fresh binary. A failed build aborts and leaves the running Bus untouched. Pass `--binary PATH` for an install with no source checkout. Pass `--clear-data` only when the restarted Bus should open with empty Messaging, Agent and Errors history: before the save and quit it drops settled requests and replies and every launch attempt. Queued and active requests stay. Omit `--clear-data` to keep that history; it is never the default. Without --dev the command is refused and omitted from `bus --help`.\nRestart is explicit: Bus never restarts a server on its own.\n\nBefore restart, the orchestrator of every room asks its working agents to pause:\ncommit and push, stop builds and tests, release slot locks, then stop. Wait for\neach confirmation, then run this command. After checking the outcome, tell them\nto resume.\n\nPreflight requires an absolute executable (the built binary, or PATH when --binary is set), a runnable --identity with a\n40-hex build SHA and nonempty provider list, and at least 5 GB free disk.\nThe running TUI must have a whole-session restart receipt and a saved executable\nwith its own SHA for rollback. A TUI predating this support must be quit with\nCtrl+Q, then opened using the replacement binary with its original flags/session. --confirm is required.\n\nThe relaunch keeps the launch flags recorded at startup (--dev, --yolo, data\ndirectory, session). Omit --yolo to keep that setting. Pass --yolo to turn it on;\nthat override changes only --yolo. The required --dev does not change a recorded --dev mode.\nTo turn --yolo off, quit the UI and resume without --yolo.\n\nThe command returns a handoff receipt, not restart success. A helper requests\nthe normal Ctrl+Q save-and-quit path. The old TUI/coordinator and server stop;\nthe replacement binary then launches in the same terminal using the normal resume path. Terminal\nscrollback and persisted state stay, and agents resume their provider sessions.\nThere is no live TUI reattachment, including for equal package versions or SHAs.\nIf the new Bus fails health checks, the saved previous binary relaunches the same way.\nOnly a responsive new server AND a newly started TUI/coordinator at the replacement binary's build\nSHA produce outcome restarted. Build SHA, not package version, identifies success.\nFailures name the failed part. A healthy rollback needs no additional TUI launch; a failed quit requires inspecting/saving the old UI before retry. If no healthy TUI remains, the log gives a resume command.\n\nAgents keep the CLI paths they were given. Omit --binary to build the checkout that owns the running executable. Pass --binary with an absolute path\n(such as target/debug/bus) for an install that has no source checkout. Restart\ndoes not rewrite other binaries, symlinks, or agent instructions. After outcome restarted, only the newest file under restarts/binaries is kept.\n\nBus sends each work-room orchestrator safe metadata after a successful restart\n(old SHA -> new SHA and actor). Failure diagnostics go to herdr-client.log.\nEvery attempt is appended to restarts/log.jsonl in the session data (who, old SHA,\nnew SHA, outcome and reason). Read that final outcome after relaunch.\n\nResult: {ok:true,stage:handoff,pid,old_sha,new_sha,session_id} means queued only.\nErrors: exit 1 for preflight or an existing handoff. Final log outcomes include\nrestarted, rolled_back, rollback_failed, quit_failed, relaunch_failed and refused. A failed save\nleaves the old Bus running; do not force it closed without checking saved work.";
pub(super) const PERF: &str = "Summarize perf/ records from the selected session's logs directory; files only,\nworks while Bus is hung/exited. --since defaults to 30m; accepts minutes (30m)\nor hours (2h). --all defaults false, excludes --since.\nOutput: text summary of stalls, slow windows and CPU/memory/load peaks, plus\nscan counts on stderr; no JSON envelope.\nErrors: exit 1 for invalid arguments/IO failure; correct the duration/path.\nUnreadable/invalid individual records are counted in scan diagnostics.";
