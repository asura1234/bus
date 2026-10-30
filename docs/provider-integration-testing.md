# Testing a new agent provider

Use this checklist before shipping a provider and append dated testing lessons as
we learn them. Record the Bus commit, provider CLI version, platform, commands,
agent and message IDs, observed screens, results, and evidence location. Repeat
acceptance on the exact final head; a rebase or fix makes earlier evidence stale.
A fake provider or a focused test is useful regression coverage, not real-model
acceptance. Keep transcripts and screenshots private and redact secrets.

## Scratch setup and binary identity

- [ ] Use a disposable project and an isolated Bus server, never an existing
  live session. Use a new absolute `BUS_DATA_DIR` with mode 0700; remove inherited
  `BUS_*` and `HERDR_*` variables. Disable real notifications or use recording
  stand-ins. Confirm `bus --paths` and `bus state` identify the scratch root.
- [ ] Use the provider's real signed-in account through its supported shared
  credential and refresh-lock stores. Do not copy credentials into scratch
  homes: independent copies can invalidate a rotating login. Provider settings,
  hooks, sessions and logs must remain isolated. Never print credentials.
- [ ] Follow the current disk, build-slot and target rules. After a build, the
  top-level `target/debug/bus` in a shared Cargo target may belong to another
  worktree even when Cargo says `Fresh`. Check the binary's embedded commit
  with `--version` or `state.build` against `git rev-parse HEAD`, and check that
  it contains the provider. If either differs, rebuild in an isolated target or
  force an appropriate rebuild; copy the verified binary into this run's private
  directory and check it again before use. Never substitute the live binary.

Example setup, after building and verifying the binary under the current slot
rules (run the UI and control commands in separate terminals):

```sh
umask 077
scratch=$(mktemp -d /private/tmp/bus-provider-test.XXXXXX)
mkdir -m 700 "$scratch/data" "$scratch/project" "$scratch/bin"
git -C "$scratch/project" init
# Copy only the verified build, not credentials or a provider configuration.
cp /absolute/path/to/verified/bus "$scratch/bin/bus"
"$scratch/bin/bus" --version
git rev-parse HEAD
# Launch in a clean environment; add only the provider's necessary login access.
env -i HOME="$scratch" PATH="$scratch/bin:/usr/bin:/bin" TERM="${TERM:-xterm-256color}" \
  BUS_DATA_DIR="$scratch/data" "$scratch/bin/bus" --dev
```

Use the same clean environment and `BUS_DATA_DIR` for every control command.
The example PATH may need the real provider executable and its dependencies;
configure the provider's shared login access without copying secrets. Set up
notification isolation before starting agents. Inspect the `--help` for the
installed build; record the session and stop only this scratch instance when done.

## Fresh first launch and dialogs

- [ ] Add a **new** agent through Bus in a new, untrusted project directory,
  exactly as the live session does. Do not pre-trust the folder or warm up the
  composer by typing into it. Observe the welcome/first-run screen before sending.
- [ ] Folder trust and permission prompts must appear as actionable Bus dialogs.
  Inspect `bus agent dialog AGENT` and `bus agent read AGENT --source visible`;
  use `bus agent choose AGENT --option N --fingerprint F` only after observing
  the current dialog and with authority to choose it. Re-observe stale fingerprints.
- [ ] Send the first message through Bus while the welcome card is present. After
  handling any surfaced dialog through Bus, it must be delivered without a
  manual terminal keypress; confirm the model repeats a unique marker and the
  request reaches a successful reply. Repeat when the welcome card is above the
  composer. A queued send, an idle screen or visible text alone is not a pass.
- [ ] Exercise normal permissions and the provider's no-prompt mode separately.
  Do not bypass the trust-dialog test with no-prompt mode.

```sh
bus room create provider-test
bus agent add --room ROOM --name new-worker --provider PROVIDER --pwd /absolute/scratch/project
bus agent dialog new-worker
bus agent read new-worker --source visible
bus send --room ROOM --to new-worker --text 'Reply with FIRST_LAUNCH_MARKER.'
bus message status MESSAGE_ID
bus history --room ROOM --limit 100
```

Substitute returned IDs; require `ok: true`, successful request completion and
the marker in the actual reply. On an uncertain submission, inspect status,
visible composer and native records before recovery; do not blindly resend.

## Real-model messaging matrix

Use a real new-provider orchestrator and workers, plus real Codex and Claude
agents. Give each route a unique marker; capture message/request IDs, final text,
`bus message status`, room history and wake evidence. Run both directions with
Codex **and** Claude, including the new provider in worker and orchestrator roles;
ask the new provider to state what it heard from the other provider.

- [ ] Orchestrator → worker: worker executes the task and its reply wakes the
  orchestrator, which acknowledges the actual result.
- [ ] Worker → worker: recipient answers and the sending worker reports the reply.
- [ ] Worker → orchestrator: the orchestrator receives the wake and responds.
- [ ] Agent → user: the post appears once in the correct room. Also check a long
  answer to a user's request appears once, complete, with both boundary markers.
- [ ] Exercise long prompts, including one clipped in a narrow composer. Confirm
  the exact full submitted text, not just a visible suffix, and that an unrelated
  or edited draft is never submitted. No manual Enter is allowed.

Use `bus send --room ROOM --as SENDER --to RECIPIENT --text '...'` for
agent-originated messages. Use `bus wait --message ID --timeout 600`,
`bus message status ID` and `bus history --room ROOM --limit 100` to inspect
settlement and routing. Follow each provider's documented wake/wait protocol;
a waiter can affect wake delivery. Report failures and unrun matrix cells rather
than inferring them from another route.

## Reply capture and failure

- [ ] Bind the exact submitted prompt to the owned provider session and turn.
  Accept the final answer only when the provider's committed turn-end record
  proves that same prompt completed successfully. Stop hooks, previews, visible
  text and idle alone cannot certify completion; partial or mismatched records
  must fail closed. A transient idle between tool calls is not a final.
- [ ] Check ordinary, tool-preamble-then-final, continuation, Unicode and long
  answers, including a reply whose hook preview is clipped. Compare the complete
  native final with the one captured by Bus, including both boundary markers.
- [ ] Interrupt/cancel and provoke a provider failure; neither may publish a
  partial answer as success. Check incomplete, torn, stale-session and wrong-
  prompt records and late signals cannot settle another request.

For example, in observed Grok 1.0.70 records the submit hook binds session,
prompt ID and text; `updates.jsonl` supplies the last matching
`agent_message_chunk` followed by `turn_completed` with the same prompt ID and
`end_turn`. A Stop preview can be clipped or precede cancellation. In observed
Kimi 2.4.0 records, `wire.jsonl` binds `turn.prompt` to the matching assistant
text and completed `agent.turn.ended`, `turn.ended` and `prompt.completed`
records. These are version-specific observations to verify again on upgrades,
not a substitute for provider-specific tests.

## Clear, resume and isolation

- [ ] Save the session identity, clear the agent with `bus agent clear AGENT`,
  and send a new message. Verify a distinct native session is bound, the message
  gets a reply, and stale callbacks or turn-end records from the old session are
  retired. Attempt `/resume` to the old session and confirm it fails loudly;
  it must not silently rebind or answer with stale context.
- [ ] Quit the scratch Bus, cold-resume the saved Bus session, and verify the
  same provider session and prior context. Restart during a request and inspect
  its durable outcome; no duplicate delivery, false success or lost ownership.
- [ ] With no-prompt mode enabled at launch, resume and perform a routine tool
  action. Confirm the saved launch option survives and no routine approval is
  needed; document any provider-specific exceptions.
- [ ] Compare provider global config and hooks before and after; there must be
  no writes. Check `git status --short` in the scratch project: generated project
  hook files must be excluded, and no credential copies may exist. Stop the
  scratch server and agents, then retain only authorized, redacted evidence.

## Gate, review and post-merge smoke

- [ ] Run the repository gate on the exact head (`just ci`) under the current
  build-slot and disk rules. Until shared-target provenance is fixed, immediately
  after acquiring the slot run `cargo clean -p bus --target-dir "$CARGO_TARGET_DIR"`
  and repeat for its `llvm-cov-target` subdirectory before **each** cargo gate.
  Release the slot on exit. Record failures; a focused pass is not a full gate.
- [ ] Get independent Claude and Codex reviews of the exact head; resolve their
  findings and repeat affected tests, acceptance and review for any new head.
- [ ] After merge and the developer's Bus restart, perform an authorized live smoke:
  add a fresh agent and have a real Codex agent send it a unique message. Verify
  the new provider confirms what it heard and the request completes. Declare the
  integration working only after this smoke; report a blocked or unrun smoke.

## Testing lessons

- **2026-10-30 — Grok:** four scratch messaging paths passed, but live fresh
  agents could not receive messages: folder trust was not detected and the
  composer below the 1.0.70 welcome card was missed. Always test the actual
  untrusted first launch, then a post-restart live smoke.
- **2026-10-30 — Kimi:** copying rotating credentials broke the login; sharing
  both credentials and refresh locks preserves one source of truth. A clipped
  composer and transient idle also exposed failures that a short prompt and idle
  screen could not prove away. Test full prompt ownership and committed finals.
