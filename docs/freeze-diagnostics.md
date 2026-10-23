# Freeze recovery diagnostics

Bus records UI stalls in its session's `perf/client.jsonl`. After a UI
iteration lasting more than 3 seconds returns, Bus sends a terminal
notification: `Bus froze for N.N s at HH:MM; details: bus perf`. The time is
the local start time of the stall on macOS/Unix; non-Unix targets currently
use UTC. Alerts default on and can be disabled in
Settings → Freeze recovery notifications. They never enter room history or
the composer, and are limited to one per minute. Terminal notification
presentation depends on the host terminal's notification support and settings.

The recorder adds these bounded diagnostics:

- `run_start`: compiled Git SHA, Cargo profile, package version, dev-tools
  feature, process PID, wall time and monotonic time.
- `active_message`: server wire variant, actual payload bytes (excluding the
  four-byte frame prefix), and dispatch phase. One atomic snapshot remains
  readable while the UI handler blocks; it contains no message contents.
- `stall`: stable per-iteration `stall_id`, loop, stage, duration and lifecycle
  state. Capture records include request, actual start and completion times on
  both wall and monotonic clocks, triggering threshold and stage at completion.
  Suppression records name cooldown or a sample already in flight.
- Stack captures may escalate around 5, 15 and 30 seconds despite the ordinary
  cooldown. Only one sample of a process runs at once. Completed captures are
  retained by age (72 hours) and bytes (64 MiB per role); `capture_evicted`
  records name each removed dump, its byte count and the limiting policy.
- `event_queues`: producer-side depth, lifetime high-water mark, oldest queued
  event age, capacity and accumulated producer wait. These remain observable
  while the UI consumes nothing.
- `coordinator.store.serialize`, `.write`, `.fsync`, `.rename`, `.dir_sync`
  timers and `.bytes`: durable state-save phases, without weakening file or
  directory synchronization.
- `coordinator.control.queue`: time from control dispatch to coordinator
  dequeue, separate from `coordinator.control` execution time. Debug logs
  correlate `bus.control.command.dequeued` with the request ID and queue time;
  `bus.control.request.timeout` records deadline expiry without cancelling a
  mutation. Coordinator stalls name the tick phase (`tick.poll`,
  `tick.callbacks`, `tick.save_callbacks`, `tick.settle`, `tick.submit`, etc.).
- `machine`: a separate worker samples macOS every 5 seconds. It reports the
  top 8 readable CPU processes using PID plus process start time, executable basename
  and CPU delta, alongside load, memory pressure level and cumulative page-in
  and page-out counts. It reads no command lines, environments or terminal
  contents. The first sample establishes CPU baselines; unreadable OS fields
  remain null. `process_coverage` reports enumerated, readable and unavailable
  process counts and the `readable_processes` scope. Normal macOS permissions
  can exclude other users' processes, including system services; unavailable
  also includes processes that exit during sampling. Load, memory pressure and
  paging remain machine-wide. Other platforms omit this macOS machine sample.

`BUS_PERF_CAPTURE_MS=0` disables stack sampling. Existing perf recording
configuration still applies. New record kinds and fields are additive JSONL.

Dev-tools builds expose a Settings action, “Test freeze recovery notification
(3.5 s)”, for scratch terminal tests. Release builds do not contain the action.

Control overload emits `bus.control.admission.rejected` at most once per second
per endpoint. Each record names `interactive` or `status`, its active/limit
counts, reading/waiting/writing/closing counts, outstanding status queue permits,
and at most 17 holders with request ID, known method, phase and connection age.
IDs are capped at 128 characters; request parameters and unknown method text
are excluded. Unread connections have no request identity. Standard CLI IDs
contain a PID, which can be correlated with a contemporaneous process snapshot;
they do not identify whether a caller used `send --async` or `wait` by themselves.
These are sampled overload snapshots, not an exhaustive connection ledger.
Status followers use `wait.sock` with separate admission from `control.sock`.
The timeout and dequeued records also cap their logged IDs at 128 characters;
request validation and response correlation still use the full ID.

`bus.deletion.pending` follows the durable delete intent;
`bus.deletion.completed` follows durable agent removal and reports how many
terminals were left open. Both carry the agent ID and, for control deletions,
inherit the control request span. A pending event without completion requires
checking the error and saved state; it is not evidence of successful removal.
Pending is emitted only on the transition into deletion_pending. Failed-startup
cleanup retries the guarded close without repeating that unchanged intent.
