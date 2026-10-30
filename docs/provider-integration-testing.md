# Testing a new agent provider

Use this checklist before shipping a provider and append dated testing lessons.
Record the exact Bus commit, provider CLI versions, platform, commands, session,
agent and message IDs, screenshots, request status and evidence location. Redact
secrets and keep raw transcripts private. A fake or focused test is regression
coverage, not real-model acceptance. Repeat acceptance for the final head.

## Scratch setup and build identity

- [ ] Use a disposable, untrusted project and a private Bus server. Never test
  against a live session. Give the scratch root mode 0700, scrub inherited
  `BUS_*`/`HERDR_*`, and check `--paths` before any control mutation. On a
  fresh `BUS_DATA_DIR` with no `settings.json`, `sbus settings` shows defaults:
  `feishu_waiting_notifications` true, `feishu_recipient`
  `ou_ca1b530d792419a044e281f998ce68dc`, and `feishu_cli_path` null. Nested
  `feishu.enabled` is true because that settings path exists. Do not call this
  unconfigured, and do not run `sbus settings feishu-notifications --off`.
  Isolation is the wrapper PATH `$scratch/bin:/usr/bin:/bin` not resolving
  `lark-cli`: none of those three directories may contain an executable
  `lark-cli`. Check those paths, not `command -v` in the tester's own shell.
- [ ] Use real provider executables and the account's shared login and refresh
  locks, not copies of credentials. Isolate provider config, hooks, sessions
  and logs. **Stop and ask the orchestrator for a validated, authorized setup**
  for every provider under test if the executable PATH, credential store and
  shared refresh-lock mapping are not known. Do not guess paths, link whole
  global config directories, print secrets, or use the real HOME as a shortcut.
  Include Codex and Claude as well as the new provider in this setup. The Kimi
  test setup must share both credential and OAuth-lock directories, not copies.
- [ ] Obey the active room's resource policy; the following is the current
  two-slot example, not authority to use another room's slots. Before **each**
  cargo build, test or gate, check `df -h /Users/dylanliu/work` (at least 20 GiB
  free). If a priority queue exists, wait until your exact name is in its first
  two lines. Atomically acquire an available slot with `mkdir`, install an EXIT
  trap immediately, and use its matching target. Remove only your queue entry.

Create `$scratch` and `$scratch/bin` before taking a slot. Paste the block
below into one interactive shell, top to bottom. It refuses the copy when
`scratch` is empty, copies and `cmp`s while the slot is held, releases the
slot, then starts `sbus --dev`.

```sh
set -eu
umask 077
scratch=$(mktemp -d /private/tmp/bus-provider-test.XXXXXX)
printf '%s\n' "$scratch"
mkdir -m 700 "$scratch/data" "$scratch/project" "$scratch/bin" "$scratch/home"
git -C "$scratch/project" init
# Slot 1, after you are admitted and it is free. Slot 2 uses the pair below.
mkdir /tmp/bus-568-build-slot-1
trap 'rmdir /tmp/bus-568-build-slot-1' EXIT
export PATH="$HOME/.cargo/bin:$PATH" CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3
export CARGO_TARGET_DIR=/Users/dylanliu/work/.cargo-targets/bus-shared
# Slot 2: /tmp/bus-568-build-slot-2 and
# CARGO_TARGET_DIR=/Users/dylanliu/work/.cargo-targets/bus-shared-2
# Interim shared-target rule: run both cleans immediately after acquisition,
# before EVERY manual acceptance build, test, or gate, including reruns.
if [ -z "${scratch:-}" ] || [ ! -d "$scratch/bin" ]; then
  echo "create \$scratch/bin before the slot; refusing to copy" >&2
  exit 1
fi
nice -n 19 cargo clean -p bus --target-dir "$CARGO_TARGET_DIR"
nice -n 19 cargo clean -p bus --target-dir "$CARGO_TARGET_DIR/llvm-cov-target"
nice -n 19 cargo build --locked --features dev-tools
cp "$CARGO_TARGET_DIR/debug/bus" "$scratch/bin/bus"
cmp "$CARGO_TARGET_DIR/debug/bus" "$scratch/bin/bus"
rmdir /tmp/bus-568-build-slot-1
trap - EXIT
# After the approved provider login/PATH setup, put its executable directories
# and non-secret environment in this private script. Do not copy credentials.
cat > "$scratch/bin/sbus" <<'WRAPPER'
#!/bin/sh
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P) || exit 1
# Add only the orchestrator-approved provider PATH and login variables here.
exec env -i HOME="$root/home" PATH="$root/bin:/usr/bin:/bin" \
  TERM="${TERM:-xterm-256color}" BUS_DATA_DIR="$root/data" \
  "$root/bin/bus" "$@"
WRAPPER
chmod 700 "$scratch/bin/sbus"
alias sbus="$scratch/bin/sbus"  # interactive shell; later examples use sbus
sbus --paths                    # data must equal $scratch/data
sbus --dev                      # UI terminal; leave it open
```

In the other terminal, set `scratch` to the printed path and the same alias,
then run `sbus --paths` and `sbus state`. `build.binary` must be
`$scratch/bin/bus`. Run `sbus settings` there too. With no `settings.json`,
expect `result.settings.feishu_waiting_notifications` true,
`result.settings.feishu_recipient` `ou_ca1b530d792419a044e281f998ce68dc`,
`result.settings.feishu_cli_path` null, and `result.settings.feishu.enabled`
true. A null CLI path execs `lark-cli` when that PATH resolves it, so the
scratch PATH is the isolation. Do not run
`sbus settings feishu-notifications --off`.

A shared target's top-level binary can belong to another worktree even after
Cargo reports `Fresh`. A copy is not verified merely because Cargo succeeded.
Verify the **copied binary** and the scratch server's compiled identity before
adding agents: the block above starts `sbus --dev` only after `cmp`. Get `logs`
from `sbus --paths`, and read `<logs>/perf/client.jsonl` and `server.jsonl` for
this run's `run_start` records. Both `build_sha` values must equal the full
`git rev-parse HEAD`; correlate the new records using their PID/time and
`sbus state`'s `build.binary` path. Check `sbus agent add --help`'s
`possible values` includes the new provider. `--version` and `state.build` do
**not** expose the SHA on current builds. If a record is missing, `unknown`,
mismatched, or cannot be tied to this run, stop: obtain a verified unique-target
build or wait for the provenance fix. Never accept a short SHA or a guessed
binary. Keep the slot until the shared-target copy is made; compare its
checksum to the source while the slot is held.

### One wrapper for UI and control

Use `sbus` for **every** later example. The wrapper above is a template: without
an approved real-provider PATH and login mapping, **stop**, do not run a pretend
acceptance. Run `sbus --paths` again before mutations and after relaunch; require
`ok: true` on control commands. `sbus stop` is outside that envelope: it prints
`{"stopped":true}` when a server was running, or `{"stopped":false}` when none
was running, with no `ok` field. Require `stopped: true` after a server this
scratch started.
Use only a provider listed by `sbus agent add --help`; Kimi applies once the
build actually supports it. Preserve the private root until evidence is saved.

## Fresh first launch and dialogs

- [ ] Add a brand-new agent through Bus in a never-trusted project directory;
  do not pre-trust, warm up its composer, or type in its terminal. Use the
  first-add result for that provider and for whether its project hook file is
  already current. A current file is one installation would leave unchanged;
  a missing or different file is not current. After any add that returns
  `ok: true`, do not add that name again. The repeat fails with
  `Agent name "NAME" is already used by agent AGENT_ID in room ROOM_ID; choose a unique name across all rooms`.
  `sbus agent setup-confirm AGENT --confirm` is only for an agent that
  already exists: it sets hook consent and does not install hooks. Without
  `--confirm` it is rejected with
  `This command changes or removes work; add --confirm to proceed`.
- [ ] The folder-trust prompt and ordinary permission prompts must surface as
  actionable Bus dialogs for every provider under test. That includes Kimi
  2.4.0's "Trust this folder?" screen. An agent left "launching", with messages
  abandoned as `agent_launching`, fails this check. Observe
  `sbus agent dialog AGENT` and `sbus agent read AGENT --source visible`;
  choose only an authorized option using its fresh fingerprint. Re-observe
  after a stale fingerprint.
- [ ] While the welcome card is still above the composer, send the first
  message through Bus. After resolving any dialog through Bus, it must reach
  the model and get the unique marker back without a manual terminal keypress.
  Grok 1.0.70 showed this exact card/composer layout and Bus held every message
  as `prompt_not_shown`; a warmed or card-free test does not cover it.
- [ ] Run normal permission and no-prompt cases separately; no-prompt cannot
  substitute for testing trust and permission dialogs.

Claude has no project-hook notice. Add it once, with or without
`--consent-hooks`. Expect `ok: true` and `result`
`{agent_id, stage: "launching"}`. The agent is saved. Do not run the add again.

Codex (`.codex/hooks.json`), Cursor (`.cursor/hooks.json`), and Grok
(`.grok/hooks/bus.json`), while that file is missing or not current: add once
**without** `--consent-hooks`. Expect `ok: false`, `result: null`, and no
`agent_id`. `error.message` is
`Project hook setup requires --consent-hooks: MESSAGE (PATH)`. That attempt
does not save the agent. Read `PATH`, then run the same add with
`--consent-hooks`. Expect `ok: true` and
`result` `{agent_id, stage: "launching"}`.

Once a Codex or Cursor hook file is current, skip that missing-hook check. An
add without `--consent-hooks` succeeds and saves the agent. Add the name once.

Once `.grok/hooks/bus.json` is current, an add without `--consent-hooks` still
fails and still does not save the agent. `error.message` is
`Setup confirmation required for PATH. MESSAGE`. Retry that same name once
with `--consent-hooks`, then stop.

Run only the sequence that matches the provider under test. A fresh
`$scratch/project` has no hook files, so Codex, Cursor, and Grok take the
missing-file sequence. Continue with the dialog commands for the agent that
returned `ok: true`.

```sh
# Claude, or Codex/Cursor after its hook file is current. One add.
sbus room create provider-test
sbus agent add --room ROOM --name new-worker --provider PROVIDER \
  --pwd "$scratch/project"
# Expect ok:true and {agent_id, stage:"launching"}. Do not add new-worker again.
```

```sh
# Codex, Cursor, or Grok while its hook file is missing or not current.
sbus room create provider-test
sbus agent add --room ROOM --name new-worker --provider PROVIDER \
  --pwd "$scratch/project"
# Expect ok:false, result:null, no agent_id, and
# Project hook setup requires --consent-hooks: MESSAGE (PATH)
sbus agent add --room ROOM --name new-worker --provider PROVIDER \
  --pwd "$scratch/project" --consent-hooks
# Expect ok:true and {agent_id, stage:"launching"}. Do not add new-worker again.
```

```sh
# Grok after .grok/hooks/bus.json is current. The first command does not save.
sbus room create provider-test
sbus agent add --room ROOM --name new-worker --provider grok \
  --pwd "$scratch/project"
# Expect ok:false and Setup confirmation required for PATH. MESSAGE
sbus agent add --room ROOM --name new-worker --provider grok \
  --pwd "$scratch/project" --consent-hooks
# Expect ok:true. Do not add new-worker again.
```

```sh
# Existing agent only. Records consent and does not install hooks:
sbus agent setup-confirm new-worker --confirm
sbus agent dialog new-worker
sbus agent read new-worker --source visible
# If a dialog is present and its choice is authorized:
sbus agent choose new-worker --option N --fingerprint F
sbus send --room ROOM --to new-worker --text 'Reply with FIRST_LAUNCH_MARKER.'
sbus message status MESSAGE_ID
sbus history --room ROOM --limit 100
```

Use actual returned IDs. Require a successful completed reply, not just queueing,
idle or screen text. If submission is uncertain, inspect status, composer and
native records before recovery; do not blindly resend.

## Real-model messaging matrix

Create the new-provider orchestrator in MASTER and real Codex, Claude and new
provider workers in its work room. Separately create a Claude orchestrator and
new-provider worker in a second room. New Codex orchestrators are refused; do
not claim that role was tested. Use real models and unique markers for each cell.
The block below is each role's successful add. Use the first-add rule for that
provider and for this project's hook file, and run each name only until the
first `ok: true`.

`claude-worker` is Claude. The command below is the one add. `--consent-hooks`
would not change the result. If an unconsented add of that name already
returned `ok: true`, do not run the command again.

`codex-worker` is the first Codex add unless `.codex/hooks.json` is already
current. While it is missing or not current, run the command once without
`--consent-hooks`, expect
`Project hook setup requires --consent-hooks: MESSAGE (PATH)`, then run the
command as written. Once the file is current, the unconsented add succeeds
and saves the agent; do not run a second add of `codex-worker`.

`new-orch` and `new-peer` use PROVIDER in the same `$scratch/project` as the
fresh-launch agent. Claude, and Codex or Cursor whose hook file is already
current, succeed on one add. Grok whose `.grok/hooks/bus.json` is already
current fails the unconsented add with
`Setup confirmation required for PATH. MESSAGE` and does not save the agent;
retry that name once with `--consent-hooks`. A provider whose hook file is
still missing uses the `Project hook setup requires --consent-hooks` error,
then the flagged command. A name that already returned `ok: true` is finished.

```sh
sbus room create provider-matrix
sbus agent add --room master --name new-orch --provider PROVIDER \
  --orchestrates provider-matrix --pwd "$scratch/project" --consent-hooks
sbus agent add --room provider-matrix --name codex-worker --provider codex \
  --pwd "$scratch/project" --consent-hooks
# Claude: this single command is the whole add. Do not run it twice.
sbus agent add --room provider-matrix --name claude-worker --provider claude \
  --pwd "$scratch/project"
sbus agent add --room provider-matrix --name new-peer --provider PROVIDER \
  --pwd "$scratch/project" --consent-hooks
```

| Route | Evidence to capture |
| --- | --- |
| [ ] New orchestrator → Codex worker → orchestrator wake | Worker result and a new orchestrator turn acknowledging it |
| [ ] New orchestrator → Claude worker → orchestrator wake | Same, with a different marker |
| [ ] Claude orchestrator → new worker → orchestrator wake | New worker repeats what it heard and Claude acknowledges the wake |
| [ ] New worker → Codex worker → new worker | New worker initiated send and reports the returned marker |
| [ ] Codex worker → new worker → Codex worker | New worker repeats the Codex message and Codex reports its reply |
| [ ] New worker → Claude worker → new worker | New worker initiated send and reports the returned marker |
| [ ] Claude worker → new worker → Claude worker | New worker repeats the Claude message and Claude reports its reply |
| [ ] Codex worker → new orchestrator | New orchestrator wakes and repeats the Codex marker |
| [ ] Claude worker → new orchestrator | New orchestrator wakes and repeats the Claude marker |
| [ ] New worker → orchestrator | Orchestrator wakes and acknowledges the worker's marker |
| [ ] Agent → user | Correct room contains exactly one post; long answer has both boundary markers once |

Initiate each route by sending a task **to the real source agent**, not by
running `sbus send --as SOURCE` yourself. For example:

```sh
sbus send --room master --to new-orch --text 'Send codex-worker a task asking it to reply CODEX_R1. End your turn and, when the worker result wakes you, tell me exactly what it replied and include the message ID.'
sbus message status TASK_MESSAGE_ID
sbus history --room provider-matrix --limit 100
sbus history --room master --limit 100
```

For worker routes, ask the source worker to run its own `bus send --room ROOM
--as ITS_NAME --to TARGET --text 'Repeat MARKER'` and report the recipient's
actual result. Ask the new provider to quote what it heard. Use the provider's
wake/wait protocol: in particular, a waiter may consume a wake. Record the
source's task, its authored send and ID in history, target `message status`,
target reply, and the source's later turn acknowledging that reply. A receipt
or an attributed send by the tester proves neither model initiation nor wake.
Also send a long prompt clipped in a narrow composer: prove the complete text
was submitted, not merely a visible suffix, and an unrelated/edited draft was
not submitted. Mark every unrun cell explicitly.

- [ ] Deliver while the agent is busy: a turn in progress, tool use, or a
  background task still running. Bus must submit that message or keep it
  queued, and must not leave text in the composer. A draft left behind, so
  later messages stay `prompt_not_shown` until a person clears the box, fails
  the test.

## Reply capture and failure

- [ ] For each request save `sbus message status MESSAGE_ID` privately. Select
  the correct `requests[]` entry; record `session_id`, `turn_id`, `start_bound`,
  `turn_ended`, `stage`, `reason`, and `reply.text`. Bind its exact prompt to the
  native owned session and turn. For automatic capture, require a committed
  terminal record and successful `stage: replied`; an explicit CLI reply can
  settle before `turn_ended`, so test automatic capture separately.
- [ ] Locate the owned native transcript from the provider's callback metadata
  and scratch session, not by picking the latest file. Grok 1.0.70's Stop hook
  gives `transcriptPath` to the session's `updates.jsonl`: select the last text
  `agent_message_chunk` for that prompt ID before `turn_completed/end_turn`.
  Kimi 2.4.0 uses the private agent home's `agents/main/wire.jsonl`: match
  `turn.prompt`, assistant final text, `agent.turn.ended` (`done`), `turn.ended`
  and `prompt.completed` (`completed`). For another provider, identify its
  owned transcript and commit boundary before accepting it; ask if unknown.
- [ ] Extract the full matching native final into a private `native.txt` and the
  status entry's exact `reply.text` into `bus-reply.txt` without adding a newline;
  run `diff -u "$scratch/native.txt" "$scratch/bus-reply.txt"`. A difference,
  wrong session/turn, incomplete JSON row or missing terminal record fails. Do
  not use Stop preview, visible text or idle alone as a final; a transient idle
  can occur between tool calls. Check ordinary, tool preamble, continuation,
  Unicode and long replies including one clipped in the hook preview.
  For example, after saving status as `"$scratch/status.json"`, extract the
  exact Bus text for a single numeric agent ID (set `AGENT_ID` first):

  ```sh
  AGENT_ID=123 python3 - "$scratch/status.json" "$scratch/bus-reply.txt" <<'PYCODE'
  import json, os, pathlib, sys
  status = json.loads(pathlib.Path(sys.argv[1]).read_text())
  rows = [r for r in status['result']['requests']
          if r['agent_id'] == int(os.environ['AGENT_ID'])]
  assert len(rows) == 1 and rows[0]['stage'] == 'replied'
  assert isinstance(rows[0]['reply']['text'], str)
  pathlib.Path(sys.argv[2]).write_bytes(rows[0]['reply']['text'].encode('utf-8'))
  PYCODE
  diff -u "$scratch/native.txt" "$scratch/bus-reply.txt"
  ```

  Derive `native.txt` from the selected complete native turn, preserving its
  exact UTF-8 bytes; save the session/prompt/turn IDs and terminal-record proof
  alongside it. If the provider's format cannot be unambiguously extracted,
  stop and ask for a verified procedure rather than substituting a preview.
- [ ] Start a long turn, run `sbus agent interrupt AGENT`, then
  `sbus message status MESSAGE_ID`. The status read is `ok: true`. When no
  final was captured before the stop, expect `complete: true`,
  `requests[].stage` `abandoned`, `reason` `interrupted`, and `reply` null.
  A reply captured before the stop still settles as `replied`. The interrupt
  result's `outcome` is `stopped`, `already_idle`, or `failed`; that field is
  not the output of `sbus stop`.
- [ ] Run the fixture filters below with `just test-one FILTER`. Each one is a
  cargo nextest run and follows the slot, disk, and clean-first rules above.
  Do not edit a live transcript or session file to manufacture a negative case.
  A torn, mismatched, or cancelled record must not become `stage: replied`
  with a partial prefix. It stays pending in the fixture, or the request
  closes as `reply_not_captured` or `provider_error: MESSAGE`.

  Grok, in `src/agents/providers/grok/tests/`:

  ```sh
  just test-one grok_oversized_or_torn_transcripts_never_publish_prefix
  just test-one grok_1070_torn_commit_and_wrong_session_fail_closed
  just test-one grok_cancel_and_failure_end_matching_turn_without_answer
  just test-one grok_1070_cancel_and_max_turns_never_publish_a_reply
  just test-one grok_failures_and_late_cancels_are_never_successes
  ```

  Oversized or torn JSON stays pending and publishes no prefix. A torn commit
  stays pending, and a wrong `sessionId` is an error. Cancel, failure, and
  max-turns end or stay pending without a published answer.

  Claude:

  ```sh
  just test-one claude_adapter_stop_failure_reports_turn_identity_without_requiring_reply_text
  just test-one claude_stop_failure_names_the_api_error_that_ended_the_turn
  just test-one a_claude_turn_ended_by_an_api_error_reports_that_error
  ```

  A StopFailure names the turn without requiring reply text. The settlement
  fixture closes the request abandoned with reason
  `provider_error: API Error: Connection lost mid-response. The response above may be incomplete. (server_error)`
  and does not publish a reply.

  Codex has no torn-transcript fixture under the Grok names. Run:

  ```sh
  just test-one a_codex_turn_that_ended_on_a_provider_error_reports_that_error
  just test-one an_unbound_codex_turn_without_its_prompt_in_the_rollout_stays_reply_not_captured
  just test-one a_codex_turn_whose_rollout_has_no_answer_for_it_stays_reply_not_captured
  just test-one codex_empty_stop_keeps_the_turn_with_null_missing_or_empty_text
  just test-one message_status_reports_the_interrupt_reason
  ```

  The capacity fixture closes abandoned with reason
  `provider_error: Selected model is at capacity. Please try a different model. (server_overloaded)`.
  A rollout that did not run this prompt, or that has no answer for this turn,
  stays `reply_not_captured`. A Stop hook with null, missing, or empty text
  parses as a final with empty text; non-string text is an error. The
  interrupt fixture expects `complete: true`, stage `abandoned`, and reason
  `interrupted`.

  Cursor has no torn-commit or wrong-session fixture of the Grok name. An
  incomplete transcript, `status: error`, or a torn trailing row returns no
  final:

  ```sh
  just test-one cursor_final_reply_waits_for_completed_matching_turn
  ```

  Kimi is not a provider in this build and has no provider fixture directory.
  There is no Kimi `just test-one` filter to run.

  The `just test-one` filters above are the required proof for a torn
  transcript, a wrong session, a cancelled record, and `provider_error`.
  The interrupt above is the safe live close: it yields a `MESSAGE_ID` with
  `reason` `interrupted`. Bus accepts `--model` as a provider argument and
  leaves flag and model validation to the provider, so an invalid model is
  not a Bus-defined `provider_error`. Do not invent one, and do not edit a
  transcript. A live `provider_error` is opportunistic. When acceptance
  itself ends a turn with a capacity error, an API error, or a StopFailure,
  record `sbus message status MESSAGE_ID`. That read is `ok: true` and exit
  0. Expect `complete: true`, `requests[].stage` `abandoned`, `reply` null,
  and `reason` starting with `provider_error: `. Record the full reason. A
  partial prefix with `stage: replied` fails the check.

## Clear, restart and isolation

- [ ] Save the old session ID; with no active or queued work, run
  `sbus agent clear AGENT`. Record `result.sent` and `stage: cleared`, then
  send a fresh message. Require a distinct bound session and successful reply;
  old callbacks and turn ends must not settle the new request.
- [ ] Deliberately type the provider's `/resume OLD_SESSION` in its native pane
  for this negative test only, not via `sbus send`. Inspect `sbus state`, visible
  pane and a follow-up request: an old session must fail loudly or be reported as
  a failure if Bus silently follows the wrong identity. Record the actual error,
  invalidation or status reason; a silent rebind to the old session is a failure.
  Never infer success from the command receipt.
- [ ] Ctrl+Q the scratch UI, then relaunch `sbus --dev` in the same environment
  and data root. Confirm paths, binary, saved provider session/context and a
  fresh reply. For a mid-request restart, run `sbus stop` during the turn.
  Expect `{"stopped":true}` and no `ok` field, then relaunch `sbus --dev` and
  inspect `sbus message status MESSAGE_ID`.
  Explicit `BUS_DATA_DIR` cannot be combined with `bus resume`; this is an
  explicit-root relaunch, not a registry-resume test. Use the separate isolated
  registry procedure below. Machine sleep or Bus restart ends in-flight turns
  as `reply_not_captured`; rerun acceptance from a fresh scratch instance, never
  resume the interrupted run as a pass.
- [ ] Test provider flags through `agent add --args`: Claude
  `--dangerously-skip-permissions`, Codex `--yolo`, Cursor `--force`, Grok
  `--always-approve`, Kimi `--yolo` (when supported). Check saved launch options
  and a routine tool action after relaunch. Kimi's mode can still ask about
  risky actions. Separately test Bus `--yolo`: it is run-scoped, not saved, and
  must be supplied again on relaunch. Record the generated provider flag and
  check routine work; an attach with a different Bus yolo setting is refused.
- [ ] `git -C "$scratch/project" status --short` may list `?? .codex/` and
  `?? .cursor/` after a consented Codex or Cursor add (`.codex/hooks.json` and
  `.cursor/hooks.json`). Grok writes `.grok/hooks/bus.json` and
  `.grok/hooks/.bus-hooks.lock`; only those two paths are in `.git/info/exclude`,
  so they may be absent from that status. Do not delete project hooks to make
  status empty. The isolation check is no edit to a global provider config
  (`~/.claude`, `~/.codex`, `~/.cursor`, `~/.grok`, `~/.kimi-code`) and no
  credential copies. Stop only the scratch server with `sbus stop` and require
  `{"stopped":true}`. Retain only authorized, redacted evidence.

### Separate registry-based cold resume

Use a second private scratch HOME and the same verified binary and approved
provider login mapping. Do not set `BUS_DATA_DIR` for this test. The wrapper
below is a template with the **same approved provider PATH and login variables**
as `sbus`; stop and ask if they are unavailable. It creates its own registry,
not an entry in the developer's registry:

```sh
mkdir -m 700 "$scratch/registry-home" "$scratch/registry-xdg"
cat > "$scratch/bin/rbus" <<'WRAPPER'
#!/bin/sh
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P) || exit 1
# Add the same approved provider PATH and login variables as sbus here.
exec env -i HOME="$root/registry-home" XDG_DATA_HOME="$root/registry-xdg" \
  XDG_CONFIG_HOME="$root/registry-xdg" XDG_STATE_HOME="$root/registry-xdg" \
  PATH="$root/bin:/usr/bin:/bin" TERM="${TERM:-xterm-256color}" \
  "$root/bin/bus" "$@"
WRAPPER
chmod 700 "$scratch/bin/rbus"
alias rbus="$scratch/bin/rbus"
rbus --paths                    # all session/registry paths must be under scratch
rbus --dev                      # start a new registry session in the UI terminal
# In the second terminal, same rbus wrapper:
rbus sessions                   # record this new session's exact ID
rbus --paths                    # confirm its root is under scratch
rbus state                      # verify binary, agents and saved provider session
# After Ctrl+Q, in the UI terminal:
rbus resume SESSION_ID --dev
# In the control terminal, recheck paths, state and the earlier message status.
```

Check the provider recalls the prior marker and replies to a new task. Both
terminals must use `rbus`; if any path or settings escape scratch, stop. Repeat
with `--yolo` on both launches for a run-scoped bypass. This registry test is
separate from the explicit-root relaunch and does not rescue an interrupted run.

## Gate, review and mandatory live smoke

- [ ] Run `just ci` on the exact head with the current slot/disk policy and the
  clean-first procedure above, including `llvm-cov-target`, on every rerun.
  Record gate failures; a focused pass is not a full gate.
- [ ] Get two independent reviews of that exact head. The reviewers are two
  different agents, and neither authored the change. At least one reviewer
  runs on a different model from the author. When only two models are
  available, the second reviewer may use the author's model and must still be
  a different agent. Record both reviewer agents, their models, and the full
  head SHA they reviewed. On 2026-10-31 the authorized lanes are Grok and
  Kimi, while Codex is at a usage limit and Claude orchestrates. Resolve
  findings and repeat affected checks, acceptance and review after any new head.
- [ ] Do not depend on one provider. If Codex or another model stops on a usage
  limit, keep another real model that can send and reply, and continue
  authoring, review, and live smoke with it. Record which providers actually ran.
- [ ] After merge and the developer's Bus restart, an authorized operator must
  perform the live smoke before declaring the integration working. Verify the
  live server's binary and merged `build_sha` using its current run_start record;
  if identity is unavailable, stop and report the smoke blocked. Add the new
  provider and one peer that is available now: Codex when it has quota,
  otherwise Claude, Grok, or Kimi. Record the peer's provider. Use a new
  untrusted project, surface trust as a Bus dialog, and while the new
  provider's welcome card is visible have the peer send a unique message.
  Require the new provider to repeat what it heard and reply. Then have the
  new provider send a different marker to the peer and require the peer to
  repeat it and reply. Record IDs, status, screen and exact merged build.
  Use the first-add result for each provider and for whether the project's
  hook file is current. After `ok: true`, do not add that name again. When
  authorized, delete each smoke agent on the live session, not the scratch
  wrapper: `bus agent delete SMOKE_AGENT --confirm`. Expect `ok: true` and
  `result.updated: true`. Without `--confirm` the delete is rejected.

## Testing lessons

- **2026-10-30 — Grok:** four scratch paths passed, yet live first launch could
  not receive messages: folder trust was missed and the composer under the
  1.0.70 welcome card was reported `prompt_not_shown`. Test that exact screen
  and run the mandatory live smoke after restart.
- **2026-10-30 — Kimi:** copied rotating credentials broke login; share credentials
  and refresh locks. A clipped composer and transient idle also defeated short
  prompt and idle-only checks. Verify full prompt ownership and committed finals.
- **2026-10-30 — interruption:** machine sleep or Bus restart can end in-flight
  turns as `reply_not_captured`; rerun interrupted acceptance in a fresh instance.
- **2026-10-30 — Grok welcome card:** the welcome card hid the composer, so Bus
  held messages as `prompt_not_shown`. Send the first message while that card
  is still visible.
- **2026-10-30 — Codex quota:** Codex hitting its usage limit stopped all
  authoring. Grok and Kimi took over once their messaging worked. Live smoke
  pairs the new provider with whichever other provider is available, in both
  directions: Codex when it has quota, otherwise Claude, Grok, or Kimi.
  Record which peer ran.
- **2026-10-31 — Grok busy draft:** a Grok agent busy with a background task was
  treated as idle. Bus typed into the composer, could not confirm the submit,
  and left a draft. Later messages stayed `prompt_not_shown` until a person
  cleared the box. Deliver while the agent is busy and require that no draft
  remains.
- **2026-10-31 — Kimi trust:** Kimi 2.4.0's "Trust this folder?" screen was not
  detected. Agents stayed "launching" and messages were abandoned as
  `agent_launching`. The fresh-launch test must cover each provider's trust
  prompt as a Bus dialog.
