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
- [ ] Use real provider executables and the account's existing logins. Run
  acceptance with the real `HOME` so those logins are read in place, plus a
  private `BUS_DATA_DIR` (mode 0700). A scratch `HOME` has no logins. Never
  copy credentials, print secrets, or edit `~/.claude`, `~/.codex`, `~/.grok`,
  or `~/.kimi-code`. **Stop and ask the orchestrator for a validated,
  authorized setup** for every provider under test if the executable PATH,
  credential store and shared refresh-lock mapping are not known. Do not guess
  paths or link whole global config directories. Include Codex and Claude as
  well as the new provider. The Kimi test setup must share both credential and
  OAuth-lock directories, not copies. Confirm `sbus --paths` data root is the
  private directory, never the live session under `~/.local/share/bus`.
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
mkdir -m 700 "$scratch/data" "$scratch/project" "$scratch/bin"
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
# Real HOME: provider logins are read in place. Never copy them.
if [ -z "${HOME:-}" ]; then
  echo "HOME must be the real login home; a scratch HOME has no logins" >&2
  exit 1
fi
exec env -i HOME="$HOME" PATH="$root/bin:/usr/bin:/bin" \
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
- [ ] Fresh first launch in a never-trusted directory. The folder-trust prompt
  and ordinary permission prompts must surface as actionable Bus dialogs for
  every provider under test. Grok 1.0.70's "Do you trust the contents of this
  directory?" must be `dialog` true and status blocked before any delivery.
  Answer Grok with `y` or `n` and no Enter; `y` only for the agent's own cwd.
  `session_hook_missing` with `dialog` false on that screen fails. Kimi 2.4.0's
  "Trust this folder?" is not that `y`/`n` check. `src/agents/dialog/kimi.rs`
  parses it as a choice dialog: the marked "Trust and continue" is chosen with
  Enter, and "Exit" is Down then Enter. There is no `y`/`n` key. The hint is
  `↑↓ navigate · Enter select · Esc exit`. Answer only through
  `sbus agent dialog` and `sbus agent choose`, and only when
  `agent add --help` lists `kimi`. Enter on an empty composer is not this
  check. An agent left "launching", with messages abandoned as
  `agent_launching`, fails. After trust is chosen, this master has no
  `Try "..."` composer match, so Claude Code 2.1.309's untouched placeholder
  is not an empty composer here. After #239 merges, that untouched
  `Try "..."` placeholder (no SGR, cursor on the first cell) is an empty
  composer. The first message must deliver with no manual key.
  `composer_busy` on that placeholder fails. The same glyphs are a draft
  after any real keystroke or after Bus has pasted; do not paste over them.
  Until that merge, do not score the placeholder rule as current master
  behaviour. Observe `sbus agent dialog AGENT` and `sbus agent read AGENT
  --source visible`; choose only an authorized option using its fresh
  fingerprint. Re-observe after a stale fingerprint.
- [ ] While the welcome card is still above the composer, send the first
  message through Bus. After resolving any dialog through Bus, it must reach
  the model and get the unique marker back without a manual terminal keypress.
  Grok 1.0.70 shows the card above a real `❯` composer and no footer under the
  frame. That layout must accept the message. `prompt_not_shown` while `❯` is
  on screen fails. A warmed or card-free test does not cover it. Do not prime
  the pane.
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

Create two work rooms. `provider-matrix` has one non-Codex orchestrator in
MASTER and real Codex, Claude, and new-provider workers. `provider-claude`
has its own Claude orchestrator in MASTER and a new-provider worker. An
orchestrator binds exactly one work room for its whole life, and a work room
has at most one orchestrator, so `new-orch` stays on `provider-matrix` and
`claude-orch` stays on `provider-claude`. Record each `room create` result's
`room_id`. `--room` and `--orchestrates` accept that ID or the room's unique
name. Use real models and unique markers for each cell. A new Codex
orchestrator is refused before hook setup and is not saved. Expect `ok: false`
and this `error.message`:

`Codex cannot be an orchestrator yet: an orchestrator dispatches work with bus send --async in the background and must be woken when it finishes, but Codex does not start a new turn when a background command exits (openai/codex issues 32188, 50079). Use Claude Code or Cursor as the orchestrator; Codex works fine as a worker.`

Record that refusal. Do not expect `ok: true` or `{agent_id, stage: "launching"}`,
and do not claim the orchestrator cells passed. The orchestrator is another
provider listed by `sbus agent add --help`. The check allows every provider
except Codex, including Grok. If the new provider is Codex, use Claude, Cursor,
or Grok for `new-orch`.
The block below records the Codex orchestrator refusal, then each other role's
successful add. Use the first-add rule for that provider and for this project's
hook file, and run each saved name only until the first `ok: true`.

`claude-worker` is Claude. The command below is the one add. `--consent-hooks`
would not change the result. If an unconsented add of that name already
returned `ok: true`, do not run the command again.

`codex-worker` is the first Codex add unless `.codex/hooks.json` is already
current. While it is missing or not current, run the command once without
`--consent-hooks`, expect
`Project hook setup requires --consent-hooks: MESSAGE (PATH)`, then run the
command as written. Once the file is current, the unconsented add succeeds
and saves the agent; do not run a second add of `codex-worker`.

`new-orch` is not Codex. The hook rule for its provider applies only after
the orchestrator check allows the add. `new-peer` and `codex-worker` are the
Codex adds that can succeed. Claude, and Codex or Cursor whose hook file is
already current, succeed on one worker add. Grok whose `.grok/hooks/bus.json`
is already current fails the unconsented worker add with
`Setup confirmation required for PATH. MESSAGE` and does not save the agent;
retry that name once with `--consent-hooks`. A provider whose hook file is
still missing uses the `Project hook setup requires --consent-hooks` error,
then the flagged command. A name that already returned `ok: true` is finished.

`claude-orch` is Claude, same as `claude-worker`: one add, and
`--consent-hooks` does not change the result. Create `provider-claude` first.
Expect `ok: true` and `{agent_id, stage: "launching"}`. A second orchestrator
on `provider-matrix` fails with `Room {room_id} already has orchestrator agent
{agent_id}; a work room has at most one orchestrator`. A second `agent add` of
the saved name `new-orch`, including one with `--orchestrates provider-claude`,
fails with `Agent name "new-orch" is already used by agent AGENT_ID in room
ROOM_ID; choose a unique name across all rooms` and leaves `new-orch` on
`provider-matrix`.

`second-peer` is the new provider in `provider-claude`. Use the same first-add
rule as `new-peer`. The name is session-unique, so it is a different agent
from `new-peer`.

```sh
sbus room create provider-matrix
# Expect ok:true and {room_id:NUMBER}.
# Refused before hook setup. ok:false, agent not saved. Not an orchestrator-cell pass.
sbus agent add --room master --name codex-orch --provider codex \
  --orchestrates provider-matrix --pwd "$scratch/project"
# ORCH is not codex. Claude, Cursor, or Grok, as agent add --help lists them.
sbus agent add --room master --name new-orch --provider ORCH \
  --orchestrates provider-matrix --pwd "$scratch/project" --consent-hooks
sbus agent add --room provider-matrix --name codex-worker --provider codex \
  --pwd "$scratch/project" --consent-hooks
# Claude: this single command is the whole add. Do not run it twice.
sbus agent add --room provider-matrix --name claude-worker --provider claude \
  --pwd "$scratch/project"
sbus agent add --room provider-matrix --name new-peer --provider PROVIDER \
  --pwd "$scratch/project" --consent-hooks
sbus room create provider-claude
# Expect ok:true and {room_id:NUMBER}. This room is only for claude-orch.
# Claude orchestrator for provider-claude. One add. Do not run it twice.
sbus agent add --room master --name claude-orch --provider claude \
  --orchestrates provider-claude --pwd "$scratch/project"
# Expect ok:true and {agent_id, stage:"launching"}.
# New-provider worker in provider-claude. Same first-add rule as new-peer.
sbus agent add --room provider-claude --name second-peer --provider PROVIDER \
  --pwd "$scratch/project" --consent-hooks
```

| Route | Evidence to capture |
| --- | --- |
| [ ] New orchestrator → Codex worker → orchestrator wake | Worker result and a new orchestrator turn acknowledging it |
| [ ] New orchestrator → Claude worker → orchestrator wake | Same, with a different marker |
| [ ] `claude-orch` → `second-peer` → orchestrator wake | `second-peer` repeats what it heard and Claude acknowledges the wake |
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
sbus send --room master --to claude-orch --text 'Send second-peer a task asking it to reply CLAUDE_R1. End your turn and, when the worker result wakes you, tell me exactly what it replied and include the message ID.'
sbus message status TASK_MESSAGE_ID
sbus history --room provider-claude --limit 100
sbus history --room master --limit 100
```

For worker routes, ask the source worker to run its own `bus send --room ROOM
--as ITS_NAME --to TARGET --text 'Repeat MARKER'` and report the recipient's
actual result. Ask the new provider to quote what it heard. Use the provider's
wake/wait protocol: in particular, a waiter may consume a wake. Record the
source's task, its authored send and ID in history, target `message status`,
target reply, and the source's later turn acknowledging that reply. A receipt
or an attributed send by the tester proves neither model initiation nor wake.
Also send a long paste that clips in a narrow composer. Prove the complete
owned text was submitted, not a visible suffix, and that an unrelated or
edited draft was not submitted. Grok 1.0.70 labels a paste longer than 10000
bytes as `floor(UTF-8 bytes / 1000)` KB. A 21217-byte bootstrap is
`[Pasted: 21 KB]`, not 20. Bus must press Enter on that chip. Withholding
Enter is `prompt_not_shown` and fails. A nearby length that Grok does not
  label with that same KB chip must stay rejected. At or below 10000 bytes a
  multiline paste is labelled with `split('\n').count()` lines, and that count
  must be greater than 1. A single line stays inline: there is no
  `[Pasted: …]` chip, and a `[Pasted: 1 lines]` chip must stay rejected. Do
  not shorten the stock bootstrap to dodge the chip. On Kimi, a long
  multi-line paste left in the composer with footer `shift+enter: newline` is
  `submission_unconfirmed` and fails: Enter inserts a newline there. The full
  text has to be submitted by the editor handoff. A one-line pointer does not
  prove the long paste. Mark every unrun cell explicitly.

- [ ] Busy-then-idle delivery. A turn, a tool, or a background row is busy even
  when Bus status says idle. A bare `grok` title does not win over `Queued`,
  still-running, or suggestion frames. Busy frames to record include Grok
  `[stop]`, `◎ … still running`, `◎ waiting · send a message to interrupt`,
  `Queued N` or `N queued` with N at least 1, and Kimi `[N tasks running]`,
  `Running a command`, or `Working…` (Kimi only when `agent add --help` lists
  `kimi`). Bus must submit the message or keep it queued, and must not leave
  its text in the composer. A `Queued N` prompt is accepted: wait for that
  turn, and closing it as `reply_not_captured` fails. A suggestion ghost
  (`Tab/→:accept suggestion`) is not a draft. Do not Tab-accept it.
  `prompt_not_shown` on that ghost fails. After the busy work ends, deliver
  a second marker. `prompt_not_shown` until a person clears the box fails.
  `composer_busy` against an empty composer means a preserved `draft-*.txt`
  from a failed editor handoff. That reason starts no queued recovery clock, so a
  "No reply" notice will not fire. Fail the cell. Do not delete live-session
  files, and do not paste over the empty composer.
- [ ] Long idle, then deliver. Leave the agent idle long enough for a provider
  chooser, then send. Kimi's "Cache expired — the next message re-sends the
  entire history at full price" chooser (Compact and continue / Start a new
  session / Continue as-is / Don't ask me again) must be a Bus dialog.
  `submission_unconfirmed` with `agent dialog` null fails. Answer only through
  `sbus agent dialog` and `sbus agent choose`, the same way as "Trust this
  folder?". For a delivery that must keep the same session, choose
  "Continue as-is". Do not press Enter on the composer. Compact spends a
  full-history resend, and a new session drops the thread. Do not choose
  "Don't ask me again" to invent a tester default.
  `tui.toml` `cache_expiry_hint` is a product setting, not a default the tester
  invents. A Kimi request that stays `current_request` with `turn_ended` false
  and an empty composer fails the next send as `prior_request_active` or
  `prompt_not_shown`. The committed turn-end record must clear it first. Do
  not `request recover` while a busy marker is still on screen.

## Reply capture and failure

- [ ] For each request save `sbus message status MESSAGE_ID` privately. Select
  the correct `requests[]` entry; record `session_id`, `turn_id`, `start_bound`,
  `turn_ended`, `stage`, `reason`, and `reply.text`. Bind its exact prompt to the
  native owned session and turn. For automatic capture, require a committed
  terminal record and successful `stage: replied`; an explicit CLI reply can
  settle before `turn_ended`, so test automatic capture separately.
  Deliverable prompts carry one short `[Bus message N from NAME]` header
  (`from you` when the developer sent it). Blocked, No-reply, and idle-agent
  notices have no header. Match the native prompt on the task body. Do not
  require the old multi-line reply hint in the delivered text. That hint lives
  in the system prompt and `bus send --help`. Reply semantics are unchanged.
  A Kimi request that is `replied` with `turn_ended` false and still
  `current_request`, on an empty composer, fails. The next send is
  `prior_request_active` or `prompt_not_shown`. The committed turn-end record
  must clear it. `request recover` is a repair, not a pass, and is forbidden
  while the frame shows `[N tasks running]`, `Working…`, or `Running a command`.
  `transcript_unmatched` or `no_start_hook` with those markers visible is a
  false recovery diagnosis.
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
- [ ] Interrupt, then deliver. Start a long turn and run
  `sbus agent interrupt AGENT`, without Bus `--yolo`. That command sends Esc
  for Claude and Codex, and Ctrl+C for Cursor and Grok (`Ctrl+c:cancel`). On
  success, interrupt `outcome` is `stopped`. Read `sbus message status
  MESSAGE_ID` (`ok: true`). When no final was captured before the stop, expect
  `complete: true`, `requests[].stage` `abandoned`, `reason` `interrupted`,
  and `reply` null. A reply captured before the stop stays `replied`. Then
  read the visible composer. A Ctrl+C typed in the pane on
  `Waiting for response…` can put Bus's own prompt back in the idle composer,
  and the next delivery is `NotSubmitted`. Withdraw that owned text before
  sending. Deliver a new marker and require `stage: replied` with no draft
  left behind. Interrupt `outcome` is not the output of `sbus stop`. Esc
  leaving the turn running (`outcome` `failed`, "still working 15s after esc")
  is not a pass.
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

  Kimi has focused fixtures in `src/agents/providers/kimi/` and the shared
  request/resume suites. Run:

  ```sh
  just test-one kimi_
  just test-one committed_final_survives_an_oversized_line_before_the_turn
  just test-one malformed_wire_is_an_error_not_pending_evidence
  just test-one kimi_successful_silent_turn_completes_without_a_reply
  just test-one launch_rejects_attached_session_and_prompt_short_options
  just test-one launch_rejects_positional_input_after_the_option_terminator
  just test-one unstarted_kimi_resume_rejects_attached_session_and_prompt_options
  just test-one identity_includes_every_supported_provider
  ```

  These cover committed ownership, long and malformed wires, silent success,
  launch ownership, and restart identity. Run the Kimi live cases in this guide
  whenever `sbus agent add --help` lists `kimi`; otherwise leave them unrun and
  say so.

  The `just test-one` filters above are the required proof for a torn
  transcript, a wrong session, a cancelled record, and `provider_error`.
  The interrupt above is the live close when the provider honors its stop
  key: `outcome` `stopped`, the request `abandoned` with `reason`
  `interrupted`, then a new marker delivered. `outcome: failed` while the
  agent is still working is a fail. Bus accepts
  `--model` as a provider argument and
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
  explicit-root relaunch, not a registry-resume test. Scratch relaunch is
  `sbus --dev` on that private root. `bus --dev restart --binary PATH --confirm`
  swaps the running server and is not a step in this scratch test. Do not point
  it at the live session. Before any such live restart, commit and push, stop
  builds and tests, and release slot locks. The real-HOME section below is
  that same private-root relaunch, not `bus resume`. Machine sleep or Bus
  restart ends in-flight turns as `reply_not_captured`; rerun acceptance
  from a fresh scratch instance, never resume the interrupted run as a pass.
- [ ] Test provider flags through `agent add --args`: Claude
  `--dangerously-skip-permissions`, Codex `--yolo`, Cursor `--force`, Grok
  `--always-approve`, Kimi `--yolo` (when supported). Check saved launch options
  and a routine tool action after relaunch. Kimi's mode can still ask about
  risky actions. Separately test Bus `--yolo`: it is run-scoped, not saved, and
  must be supplied again on relaunch. Record the generated provider flag and
  check routine work; an attach with a different Bus yolo setting is refused.
  Also run one worker with neither Bus `--yolo` nor the provider's
  skip-permissions flag. Grok 1.0.70's tool-approval card (a selected `●`, a
  trailing `┃`, footer `Ctrl+c:cancel`, no `◆` header) is returned by
  `sbus agent dialog` with status blocked. `dialog` false while it is up
  fails. Do not press Enter. Yes is a permission decision. Choose only an
  authorized option with a fresh fingerprint.
- [ ] `git -C "$scratch/project" status --short` may list `?? .codex/` and
  `?? .cursor/` after a consented Codex or Cursor add (`.codex/hooks.json` and
  `.cursor/hooks.json`). Grok writes `.grok/hooks/bus.json` and
  `.grok/hooks/.bus-hooks.lock`; only those two paths are in `.git/info/exclude`,
  so they may be absent from that status. Do not delete project hooks to make
  status empty. The isolation check is no edit to a global provider config
  (`~/.claude`, `~/.codex`, `~/.cursor`, `~/.grok`, `~/.kimi-code`) and no
  credential copies. Stop only the scratch server with `sbus stop` and require
  `{"stopped":true}`. Retain only authorized, redacted evidence.

### Real-HOME cold resume

Every real-HOME run sets a private mode-0700 `BUS_DATA_DIR`. The `sbus`
wrapper in Scratch setup does that (`BUS_DATA_DIR="$root/data"`). Provider
logins are read from the real `HOME`. A scratch `HOME` has no logins. Never
copy credentials.

`XDG_DATA_HOME` is not the session registry. The registry base is
`home_dir()/.local/share/bus`. With `BUS_DATA_DIR` unset, `bus` and
`bus --dev` call `registry.create()`, which writes `sessions/<id>` and moves
the last-session pointer in that live registry. A wrapper that sets the real
`HOME` and `XDG_*` but not `BUS_DATA_DIR` does that. Do not paste one. Do not
run `bus` or `bus --dev` from a real `HOME` without the private data dir.

`bus resume` cannot be combined with `BUS_DATA_DIR`. It exits with
`BUS_DATA_DIR cannot be combined with bus resume`. Cold resume is the
isolated relaunch already in Clear, restart and isolation: Ctrl+Q, then
`sbus --dev` again on the same private root. Confirm `sbus --paths`, the
same check as Scratch setup: `data` equals `$scratch/data`. `logs` is
`$scratch/data/herdr-config/sessions/bus`, `config` is
`$scratch/data/herdr-config`, and `state` is `$scratch/data/herdr-state`.
`session_base` is expected to show the default registry,
`~/.local/share/bus`. A `BUS_DATA_DIR` run does not write it, and
`--paths` creates no session. Do not stop because `session_base` names
that default. Do not call `sbus resume` or `bus resume` in this test.

Check the provider recalls the prior marker and replies to a new task. Both
terminals must use `sbus`. Stop if `data`, `logs`, `config`, or `state`
leaves the private root. Repeat with `--yolo` on both launches for a
run-scoped bypass. This relaunch does not rescue an interrupted run.

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
  provider and one peer that `bus agent add --help` lists on the build under
  test and that is available now. Prefer Codex when it has quota and the
  build lists it. Record the peer's provider. Use a new
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
  pairs the new provider with another provider that `bus agent add --help`
  lists on the build under test and that has quota, in both directions.
  Record which peer ran.
The 2026-10-31 lessons below each name the Bus reason, the cause, the
acceptance case, and the rule. Cases are fresh first launch in an untrusted
directory, busy-then-idle delivery, interrupt-then-deliver, long paste, long
idle then deliver, and an approval card without `--yolo`.

- **2026-10-31 — real HOME:** Symptom: acceptance on a scratch `HOME` never
  logs in. Cause: that home has no provider logins, and copying credentials is
  forbidden. Test: real `HOME` plus a private mode-0700 `BUS_DATA_DIR`; `sbus
  --paths` `data` equals that directory, and `logs`, `config`, and `state`
  stay under it. `session_base` shows `~/.local/share/bus` and is not written.
  Rule: never copy credentials and never use
  the live Bus data dir. `bus resume` cannot be combined with `BUS_DATA_DIR`;
  relaunch with `sbus --dev` on that private root.
- **2026-10-31 — Grok welcome composer:** Symptom: `prompt_not_shown`, status
  idle, `dialog` false, welcome card above a real `❯` composer. Cause: the
  1.0.70 frame ends on the model-labelled border and was not a composer Bus
  recognised. Test: fresh first launch in an untrusted directory; send while
  the card is visible; no manual key. Rule: that layout must reach the model.
  A warmed or card-free launch does not count.
- **2026-10-31 — Grok trust:** Symptom: `session_hook_missing`, status idle,
  `agent dialog` null, on "Do you trust the contents of this directory?".
  Cause: the chooser was not a blocked dialog, so the bootstrap was abandoned.
  Test: fresh untrusted directory; `agent dialog` must list Yes (`y`) and No
  (`n`) before any send. Rule: `y` only for the agent's own cwd. No Enter.
  After the choose, the welcome-card send still has to run.
- **2026-10-31 — Grok paste size:** Symptom: `prompt_not_shown` because no
  Enter was sent; the chip was `[Pasted: 21 KB]` for a 21217-byte bootstrap.
  Cause: Grok labels `floor(bytes/1000)` once the paste is over 10000 bytes;
  Bus expected `floor(bytes/1024)` (20) and withheld Enter. Test: long paste
  at the stock bootstrap size, and a neighbor length of the same chip. Rule:
  press Enter only when the chip matches `floor(bytes/1000)`. At or below
  10000 bytes, a multiline paste keeps the newline line-count match and the
  count must be greater than 1. A single line stays inline. Do not shorten
  the bootstrap.
- **2026-10-31 — slim reply header:** Symptom: a long per-message reply hint
  inflated prompts and was repeated on notices that need no reply. Cause: the
  hint belongs in the system prompt and `bus send --help`. Test: a deliverable
  message shows one `[Bus message N from NAME]` line (`from you` for the
  developer); a Blocked, No-reply, or idle-agent notice has none. Rule: bind
  the native prompt on the task body. Do not require the old hint in the
  delivered text. Reply semantics stay the foreground `--reply-to` check.
- **2026-10-31 — Grok busy frame and ghost:** Symptom: `submission_unconfirmed`
  or `prompt_not_shown` while status was idle, and `reply_not_captured`
  mid-turn. A ghost line `Tab/→:accept suggestion` also abandoned as
  `prompt_not_shown`. Cause: Bus treated `[stop]`, `◎ … still running`,
  `Queued N`, and the suggestion ghost as an empty idle composer and typed
  into it. A bare `grok` title does not win over `Queued`, still-running, or
  suggestion frames. Test: busy-then-idle delivery. Those markers mean
  working: a `Queued N` prompt (N at least 1) is accepted (wait; do not close
  `reply_not_captured`), and the ghost is not a draft (do not Tab-accept it).
  If submit is unconfirmed, Bus must withdraw its own text. The next idle
  send must not be `prompt_not_shown`.
- **2026-10-31 — Grok interrupt:** Symptom: `interrupt_failed`, "still working
  15s after esc". The live 1.0.70 cancel hint is Ctrl+C, and Esc left the turn
  running. A Ctrl+C typed on `Waiting for response…` can also drop `[stop]`
  and put the Bus prompt back in the composer, so the next delivery is
  `NotSubmitted`. `sbus agent interrupt` sends Ctrl+C for Grok. Esc is the
  key for Claude and Codex. Cursor is Ctrl+C. Test: interrupt-then-deliver,
  without `--yolo`, and record the key Bus actually sent. The rule is: stop
  with Ctrl+C, read the composer, withdraw Bus's own restored text, then
  require `stage: replied`. An Esc failure is not a pass.
- **2026-10-31 — Grok approval card:** Symptom: a worker without
  `--always-approve` sat on the tool-approval card, `agent dialog` was null,
  and no reply was sent. Cause: the card has a selected `●`, a trailing `┃`,
  and `Ctrl+c:cancel`, and no `◆` header. Test: approval card without
  Bus `--yolo` and without the provider skip-permissions flag. Do not press
  Enter. Yes is a permission decision. The card is a blocked dialog.
  `dialog` false while it is up fails.
- **2026-10-31 — Claude `Try "..."` placeholder:** Symptom: `composer_busy` on
  a fresh Claude Code 2.1.309 worker, so no agent-to-agent message arrived. One
  manual key was required, and the welcome ghost survived a key plus a
  backspace. An older Claude head had taken the first message with no prime.
  Cause: the placeholder has no SGR and the cursor sits on the first cell, so
  it was stashed as a developer draft. This master has no `Try "..."` match
  under `src/server/api/agents/prompt`. Test: fresh first launch in an untrusted
  directory, trust answered only through `agent dialog` / `agent choose`, no
  manual key. After #239 merges, that untouched placeholder is empty, and
  after a real key or a Bus paste the same text is a draft. Do not treat an
  unrecognised composer as empty. Until that merge, do not score the empty
  placeholder rule as current master behaviour.
- **2026-10-31 — Kimi turn end:** Symptom: `stage: replied` with `turn_ended`
  false left the request current, so later sends were `prompt_not_shown` or
  `prior_request_active` on an empty idle composer. Long turns were
  `transcript_unmatched` while `[N tasks running]` was on screen. Cause: the
  turn end was not recorded, and a transient idle closed the request. Test:
  long idle, then deliver, on a head whose `agent add --help` lists `kimi`.
  Rule: the committed turn-end record must clear the request before the next
  send. Do not `request recover` while a busy marker is visible. A false
  `transcript_unmatched` is not a pass.
- **2026-10-31 — preserved draft file:** Symptom: `composer_busy` for hours
  with no recovery notice, while the composer on screen was empty. Cause: a
  failed editor handoff left `draft-*.txt`, the next exchange returns
  `WouldBlock`, and `composer_busy` does not start the queued recovery clock.
  Test: after a long paste or a failed handoff, the next send must not stay
  `composer_busy` against an empty composer. Rule: the blocker is the draft
  file, not the screen. Do not paste over it, and do not delete files under
  the live session. Scratch acceptance fails if its own preserved draft latches
  the following send.
- **2026-10-31 — Kimi long paste:** Symptom: `submission_unconfirmed`; the
  whole brief sat in the composer and the footer said `shift+enter: newline`.
  Cause: Enter was not confirmed, and on that composer Enter inserts a
  newline. Test: long paste. Rule: submit the full owned text, not a visible
  suffix, and do not submit an unrelated draft. A one-line pointer does not
  prove this case.
- **2026-10-31 — Kimi cache chooser:** Symptom: `submission_unconfirmed`,
  `agent dialog` null, status idle, on the cache-expired chooser. Cause: the
  four rows (Compact and continue / Start a new session / Continue as-is /
  Don't ask me again) were not a dialog, so the blocked clock never started.
  Test: long idle, then deliver. Rule: answer only through `sbus agent dialog`
  and `sbus agent choose`. For the same session, choose "Continue as-is".
  Do not press Enter on the composer. Compact spends a full resend; a new
  session drops history. Do not choose "Don't ask me again" to invent a
  tester default. `cache_expiry_hint` in `tui.toml` is a product setting,
  not a tester default.
- **2026-10-31 — Kimi trust:** Symptom: `agent_launching` (and sometimes
  `prompt_not_shown`) on a fresh agent, status still launching. Cause: "Trust
  this folder?" was not a Bus dialog. Test: fresh first launch in an untrusted
  directory, only when `agent add --help` lists `kimi`. Rule
  (`src/agents/dialog/kimi.rs`): answer only through `sbus agent dialog` and
  `sbus agent choose`. The prompt must be a dialog before any send. The marked "Trust and continue"
  is chosen with Enter; "Exit" is Down then Enter. There is no `y`/`n` key.
  Do not press Enter on the composer. An agent left launching fails the case.
- **2026-10-31 — scratch relaunch is not `bus --dev restart`:** Symptom: an
  in-flight turn ended `reply_not_captured` when the server was swapped.
  Cause: `bus --dev restart --binary PATH --confirm` stops the running server,
  health-checks the new binary, and rolls back on failure. It is hidden
  without `--dev`. Test: scratch acceptance still Ctrl+Q and relaunches `sbus
  --dev` on the private data dir. Rule: do not run that restart against the
  live session from this checklist. Before any live restart, commit and push,
  stop builds, and release slot locks. A turn ended by restart or sleep is not
  a pass.

- **2026-11-01 — Kimi folder trust:** copying a source decision into each private
  home loses trust answered in a Bus agent. Share `workspace-trust/` with the
  source home, alongside `credentials/` and `oauth/`; rebind old private homes.
  Run `tests/kimi_trust_acceptance_test.py` with `BUS_KIMI_TRUST_BINARY` and the full
  `BUS_KIMI_TRUST_SHA`: the first agent in a fresh scratch folder must expose
  the native Bus trust dialog, and the second must reach the composer without
  it. Record the one throwaway trust key added by the native answer.
