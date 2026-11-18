# feishu-bridge interface, version 1

macOS only. This executable is independent of every sink application. It uses
the locally authenticated `lark-cli` **bot** identity and never changes login,
scopes, app configuration or credentials. Requires lark-cli >= 1.0.97: the send
path passes `--text -` and streams the body on stdin, which older releases treat
as the literal text `-`; older versions are refused before any send. All JSON below is UTF-8, compact, newline terminated. Logs go to stderr;
stdout contains only command results. Unknown config keys are errors.

## CLI

```text
feishu-bridge [--config ABSOLUTE_PATH] send (--open-id ou_ID | --chat-id oc_ID) --text TEXT [--idempotency-key KEY]
feishu-bridge [--config ABSOLUTE_PATH] listen [--run-for SECONDS]
feishu-bridge [--config ABSOLUTE_PATH] listen --input ABSOLUTE_NDJSON_FILE
feishu-bridge [--config ABSOLUTE_PATH] status
feishu-bridge [--config ABSOLUTE_PATH] install
feishu-bridge [--config ABSOLUTE_PATH] uninstall
feishu-bridge --help
feishu-bridge --version
```

`--config` is global (also accepted after the subcommand). Default:
`$XDG_CONFIG_HOME/feishu-bridge/config.toml` when XDG_CONFIG_HOME is absolute;
otherwise `~/Library/Application Support/feishu-bridge/config.toml`.
Relative paths and tilde expansion in config values are not supported.

`send` requires exactly one recipient. Text is literal, including text beginning
with `@` or `-`; it is not interpreted as a filename, shell expression or option.
The optional key is 1–50 ASCII characters and is passed unchanged to lark-cli.
The bridge does not automatically retry sends: timeout/failure may mean the
message was sent. Callers should persist a key before sending and reuse it for
uncertain retries, subject to Feishu's own idempotency window.
Success prints exactly:

```json
{"message_id":"om_example","chat_id":"oc_example"}
```

Before sending or opening a live listener, the bridge checks `lark-cli auth
status --json`: appId must equal config `app_id` and the bot must be ready.
The installed transport and the machine's credential configuration are trusted
local dependencies; do not switch their app while a listener is running.
Successful sends register their message IDs for old-root/thread reconciliation.
A storage failure after an upstream send is an uncertain send, exit 4.

`listen` reconciles configured chats on startup and periodically, consumes
`im.message.receive_v1 --as bot` with renewable bounded runs, persists accepted
messages and invokes the sink. Normal operation runs until SIGINT/SIGTERM.
`--run-for` is a positive integer wall-clock bound in seconds; expiry shuts down
the consumer and exits 0, including with pending deliveries. Child-command
shutdown may add up to two seconds. No startup/sink success is implied by exit 0.

`listen --input FILE` is the synthetic-input route: reads flattened lark-cli
event NDJSON from a local file, applies the same authorization, normalization,
durable dedupe and spool, then attempts the pending queue once. It makes **no
lark-cli calls**, including auth/history, and exits 1 if any delivery remains.
Use a private config/state directory and fake sink for acceptance. This route is
an explicit local import, not an authenticated network endpoint. It cannot be
combined with `--run-for`. Malformed records fail the import; earlier committed
records remain recoverable. Re-importing the file is safe.

## TOML configuration

```toml
app_id = "cli_example"
lark_cli = "/opt/homebrew/bin/lark-cli" # default
state_dir = "/Users/example/Library/Application Support/feishu-bridge/state"
allowed_sender_open_ids = ["ou_example"]
allowed_chat_ids = ["oc_example"]
sink_command = ["/absolute/path/to/sink", "ingest", "--instance", "/absolute/instance"]
command_timeout_seconds = 30 # default; auth/send/history
sink_timeout_seconds = 30    # default
retry_seconds = 5            # default, fixed delay after failed sink attempts
reconcile_seconds = 60       # default
overlap_seconds = 300        # default; must be positive
initial_history_start = "0"  # default, epoch milliseconds string
```

`app_id` is required. `state_dir` defaults to
`~/Library/Application Support/feishu-bridge/state` (independent of config path).
`lark_cli`, `state_dir`, and sink executable must be absolute. The allowlists and
sink are required and nonempty for listen/install; send only needs app_id and
the transport/state settings. All durations are positive integers, at most
86400 seconds. IDs must have their indicated prefixes and nonempty suffixes.
`initial_history_start` is nonnegative epoch milliseconds, at most i64::MAX.
It is used only for a chat with no committed watermark.

Allowlisting is app scoped and conjunctive: sender open_id **and** chat_id must
match; sender_type must be `user`, chat_type must be `p2p`, and message_type must
be `text`. Names, mentions and message text are never identity evidence. Other
message types, bots, groups and nonallowlisted identities are ignored. Invalid
authorized text records cause a diagnostic and history retry rather than a
watermark advance. Allowlists are checked again before sink delivery, so pending
messages revoked by config stay parked. Restart to apply config changes.

## Sink protocol

One invocation per message, no shell, argv exactly `sink_command`. Stdin is one
JSON object plus newline, then EOF. Sink stdout/stderr are not protocol data
and are discarded. Example (these are the exact version 1 keys):

```json
{"app_id":"cli_example","message_id":"om_reply","chat_id":"oc_example","chat_type":"p2p","sender":{"open_id":"ou_example","sender_type":"user"},"text":"literal decoded text","reply_to":"om_parent","root_id":"om_root","thread_id":null,"create_time":"1794470000000"}
```

All IDs/text/timestamps are strings. Missing or empty ancestry becomes JSON
null, never an absent key. `reply_to` is the direct parent message_id (`parent_id`
in raw history); `root_id` is the original root message_id; `thread_id` is a
discussion identifier, not an outbound message_id. `create_time` is epoch
milliseconds. Flattened event `content` is already decoded text; raw history
`body.content` is decoded exactly once. Edits/replays do not create a second
delivery: the first stored version of `(app_id,message_id)` wins.

Exit **0** acknowledges durable application acceptance. Every nonzero exit,
signal, launch failure, stdin failure or timeout leaves the message pending and
retries it after `retry_seconds`. A failed head blocks later messages in its
chat; other chats continue. The sink must durably deduplicate
`(app_id,message_id)` **before** exiting 0. A crash after sink acceptance but
before the bridge's acknowledgement commit can cause a repeat invocation.
Exactly-once sink side effects therefore require sink idempotency.

## Persistence and recovery guarantees

- SQLite state is stored at `state_dir/spool.sqlite3`, with transactions,
  WAL and synchronous FULL. State directories/files are created owner-only.
  Accepted messages are committed before sink invocation; successful delivery
  markers and dedupe keys survive restart. There is no automatic retention
  deletion. Back up the complete state with SQLite-aware tooling while stopped.
- A user-level advisory lock for `app_id` prevents two bridge listeners using
  different config/state paths on this Mac. It is released by the OS on exit.
  Use one ingress owner across machines; this cannot lock other hosts or
  independent lark-cli consumers.
- Startup, periodic reconciliation and consumer restarts cover acknowledgement,
  crash, disconnect and CLI queue-drop gaps. History requests exhaust
  `has_more` with `page_token`, detect missing/repeated tokens and fail closed.
  Chat intervals have a fixed end; their overlapping watermark advances only
  after **all pages and discovered/remembered threads and roots** are persisted.
  Existing threads and successful outbound roots are revisited even when older
  than the watermark. No thread time filter is used.
- Backfill is sorted by creation time, then message_id, with parents preceding
  children within the batch. Each chat has a serial persisted acceptance lane.
  Committed spool rows, including live events, may dispatch while background
  recovery runs or retries. Each chat/thread backfill container enters the lane
  atomically after all of its own pages complete; an incomplete old context
  keeps the watermark retryable without blocking other committed containers.
  Late backfilled items append behind already committed live items, even when
  their creation times are earlier. Already
  queued messages retain their lane and accepted instructions are never reordered.
  There is no global order across chats or promise of network arrival order.
- Spooling continues when the sink fails. A storage failure stops ingestion
  rather than discarding accepted messages. History/API failure does not advance
  that chat's watermark; reconciliation retries. Pending messages remain on disk.
- Recovery is limited to accessible Feishu history. Recalled/deleted messages,
  missing permissions, service retention, unavailable threads and a permanently
  full/broken disk cannot be promised recoverable. `initial_history_start`
  deliberately limits initial coverage. Transport-ready with zero events proves
  connection only. Credentials and message bodies are never included in status.

## status and launchd

`status` reads local state only and prints:

```json
{"app_id":"cli_example","running":false,"pending":0,"delivered":0,"watermarks":{},"installed":false}
```

`running` means the app advisory lock is held, not that the transport/sink is
healthy. Counts are scoped to the configured app. `watermarks` maps chat_id to
epoch millisecond strings. Absent state reports zero counts. `installed` means
the plist exists; it does not mean launchd loaded it.

`install` writes `~/Library/LaunchAgents/local.feishu-bridge.APP_ID.plist`, with
absolute executable/config paths, RunAtLoad and KeepAlive, throttled restarts,
and stdout/stderr paths under `state_dir`. It prints `{"plist_path":"..."}`.
It never invokes launchctl or starts a listener. Re-running updates that file.
`uninstall` removes only that app's plist and prints the same JSON; missing is
success. It does not unload a running agent or delete state/config/logs. The
operator must bootout a loaded agent before uninstalling it. No daemon is
loaded as part of development/testing.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Command succeeded; bounded listen ended cleanly (pending is possible) |
| 1 | Runtime/import failure or pending deliveries after synthetic import |
| 2 | Invalid CLI/config, unsupported platform |
| 3 | lark-cli launch/auth/app mismatch/API/send timeout or invalid response |
| 4 | State/config/plist I/O or SQLite persistence failure |
| 5 | Another listener holds this app's lock |

Errors are short diagnostics on stderr, with no success JSON. Normal termination
of listen by SIGINT/SIGTERM is exit 0. Supervision retries runtime failures;
operators fix invalid config/auth/storage. No command logs or interprets secrets.

Changes to this version 1 contract must be communicated to all consuming owners
before implementation changes; additive fields are still contract changes.
