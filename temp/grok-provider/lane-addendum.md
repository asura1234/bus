# Grok lane interface addendum after G0

Baseline: `5eee9b6f1b4a257f42dcc5c4f25a27f326845aa7`. This addendum clarifies the five lane contracts without changing that immutable baseline or authorizing worker edits to shared files. It records the frozen responsibilities and the G1 join. The runtime/coordinator changes are implemented in the joined branch; authenticated native completion is still pending.

## B: evidence status and normalized completion contract

There is **no verified native Grok 1.0.5 committed turn-final record schema keyed by `promptId`** in the collected evidence. Do not invent one. Native model-turn captures, native background wake, and the completion evidence adapter remain pending authentication. Pending is not an accepted block or a waiver.

Evidence available locally:

- `~/.grok/docs/user-guide/10-hooks.md:281-314`: Stop runs before the gate finishes; continuations reuse the turn; the continuation cap can end a turn without a final Stop; cancellation can follow an observed Stop; persistence can fail after Stop. `stopHookActive` cannot distinguish continuing and final fires.
- `~/.grok/docs/user-guide/10-hooks.md:293-303` and `:332-393`: `Notification` / `notificationType: "idle_prompt"` is a session-state backstop, not an outcome; reports may arrive after a later submit. Correlation is by session-scoped opaque `promptId`, not dispatch timestamp.
- `~/.grok/docs/user-guide/14-headless-mode.md`: headless result/end records describe the headless protocol. They do not establish a record in the interactive TUI's `updates.jsonl` with the required prompt identity.
- `~/.grok/docs/user-guide/15-agent-mode.md` and `17-sessions.md`: ACP chunks and the persisted session-file inventory do not by themselves supply that committed turn-final schema.
- The actual offline artifacts in the assessment checkout, `temp/grok-provider/offline-hook-payloads.jsonl` and `offline-updates.jsonl`, contain session/shutdown hooks and hook-execution envelopes. They contain no authenticated model turn or turn-final proof. The binary was `grok 1.0.5 (5115b46bc909)`, SHA-256 `3dfa7f04fbb5427a8fbead286591543aaecb478b3a0ab222c4329eca1a3b2f86`. The version-matched guides were compared with the docs emitted by that binary during the assessment.
- B reports that static strings from the exact binary include `SessionUpdate::TurnCompleted` (four elements), `turn_completed`, `stop_reason`, `agent_result`, and `usage`. This is a lane-reported clue, not a verified serialized field layout, prompt-identity binding, or committed-success boundary. Do not promote it into a native fixture schema.

Keep the G0 entry point unchanged:

```rust
completion::resolve_candidate(value: &Value, settled_idle: bool)
    -> Result<CompletionResolution, String>
```

For production native inputs, absent verified authoritative evidence, return `Pending` (or an explicit validation error for invalid input). Documented cancellation/failure remains a non-success outcome. B may finish its lane with this intentional fail-closed behavior; it is not an unfinished decoding stub. This does **not** mean real worker/orchestrator reply capture is accepted or complete.

B may add this companion contract entirely inside its owned `completion.rs` and tests:

```rust
// A Bus-defined, trusted internal value, NOT a native Grok record.
// Do not derive Deserialize or read this from hook/transcript JSON.
pub(crate) struct CompletionAttestation {
    pub(crate) session: String,
    pub(crate) turn: String, // native session-scoped promptId
    pub(crate) text: String, // complete committed turn-final reply, possibly empty
}

pub(crate) fn resolve_attested_completion(
    expected_session: &str,
    expected_turn: &str,
    settled_idle: bool,
    attestation: Option<&CompletionAttestation>,
) -> Result<CompletionResolution, String>
```

Exact semantics:

1. Expected identities and attested identities must be nonempty and exactly equal; a mismatched supplied attestation is an error. Never repair, infer, or replace an identity from the newest transcript text or the clock.
2. No attestation, or `settled_idle == false`, yields `Pending`; idle alone never yields `Final`.
3. A matching attestation plus settled idle yields `Final` with those exact identities and the attested text. The text is lossless: preserve Unicode, whitespace and newlines. An explicitly attested empty final may settle without a reply body; absence of captured text is not such an attestation.
4. G owns the production attestation producer and its evidence/provenance checks. B's helper does not certify its caller's claims. The producer must establish a post-gate committed terminal outcome for the exact launch/session/prompt, recover the complete reply, reject partial/uncommitted writes, and bind the observation to the current Bus request/revision. A newer submit, cancellation/failure, session replacement or ambiguous ownership must invalidate/defer a success. Existing generic Bus callback ownership checks still apply after resolution.
5. There is presently **no approved production evidence adapter**. `resolve_candidate` must not manufacture an attestation from Stop, `lastAssistantMessage`, `stopHookActive`, idle, quiet time, mtime, ACP chunks, usage, `end_turn` on a model-response chunk, or an invented JSON field. B can use the companion helper with `None` in its production path and explicitly constructed values in synthetic unit tests. Such tests prove the Bus contract only.
6. A bounded transcript reader may safely collect documented chunks/envelopes, reject malformed/path-mismatched/oversized input, and recover text only when its ownership and boundary are established. Unsupported native shapes or lack of a proven final boundary stay pending. Do not make a synthetic fixture's final marker a production schema by naming it native.

Required synthetic contract cases: no attestation; idle false; wrong/empty session or prompt; exact multiline/Unicode and empty final; late old-prompt attestation; cancellation/failure superseding a candidate; unsupported/partial transcript; no final Stop. Keep actual offline fixtures and synthetic attestation fixtures clearly distinguished in the fixture README. No new fixture path outside B's existing list is necessary.

### G-owned reconciliation and wake work

G will own the shared spool/coordinator path for an authenticated top-level session `idle_prompt` Notification. It must pass launch/session/nesting checks, request reconciliation, and retain enough durable work for bounded retry rather than silently losing a no-final-Stop observation. B can leave session-level Notification as `Ignore` on the pinned G0 interface and report that limitation; G must intercept/represent it before generic Ignore acknowledgement when implementing this path.

The reconciliation trigger carries a session, not a fabricated prompt. G may examine previously recorded starts and Bus request ownership; it must not attribute a final to whichever request happens to be newest. A verified evidence adapter may later supply an attestation to the companion helper even when there was no Stop candidate. Without that adapter, reconcile remains pending and surfaces the evidence gap; it cannot publish a successful result. Retries must be bounded and must not spin on a retained spool record.

This coordinator reconciliation trigger is distinct from the approved Bus-delivered **orchestrator wake prompt** after a worker result/blocker. Neither establishes that Grok's native background completion notification starts a model turn. That native test remains pending sign-in. Do not run an extra Grok prompt merely to manufacture completion evidence.

## C: question dismissal and fixture/composer contract

Preferred `QuestionInput.skip_key` encoding is exactly **`"shift+x"`**. C owns parser/fixtures only. G owns `src/terminal/runtime/dialog.rs` and its shared tests.

- For a question in free-text editing (`Dialog.input` present), skip must send **Esc, then Shift+X**. Esc leaves the editor; Shift+X dismisses the question. Use an ordered submission with the existing dialog confirmation delay; do not type `X` into the text field. Return the action keys in that order.
- On answer rows, the semantic dismiss gesture is Shift+X alone. Never treat Esc as question dismissal, approval or denial. Existing `esc` / `ctrl+]` behavior for the other providers stays covered by regression tests.
- Encode the shifted character correctly for both legacy and Kitty keyboard protocols. Unknown skip strings must not silently become Esc. Keep digest/stale observation checks under the existing content lock.
- The version-matched source is `~/.grok/docs/user-guide/03-keyboard-shortcuts.md:143-153`: Shift+X dismisses; while typing, Enter submits, Esc returns to rows, and other keys go to the text field. Native model-card behavior is still pending capture; the lane fixtures are synthetic.

C's proposed synthetic fixtures are accepted: gutter rows under a diamond heading; explicit focus marker or unique highlight; checkbox focus separate from checked state; active tab/scope/input in the digest; parked Tab/Space hints treated as blocked and unactionable; boxed normal-mode composer. Collapsed paste chips yield `Unavailable` because a count cannot prove hidden text ownership. Native long-paste capture remains a gap.

Preserve developer drafts: Grok `observe` returns `Unavailable`, **not `Other`**, for a nonmatching/unknown composer body, and `Pending` only for the exact visible expected text. `ObservedSubmission::clear_step` and `withdraw_unshown` may clear `Other`, so it is unsafe for an unrelated Grok draft. Keep `input` consistent with the exact canonical body used for `Pending`; G also retains responsibility for the shared observer's ownership-change path if that can produce `Other` for Grok. Regression cases must prove that pre-existing unrelated drafts and uncertain paste chips are neither cleared nor submitted. Returning `Unavailable` may leave an uncertain Bus paste untouched; do not claim definite non-delivery or silently retry until ownership can be established.

Tests use `grok_c_`. Invoke a Bus binary only by an explicit isolated lane build path under its configured `CARGO_TARGET_DIR`; do not fall back to the main checkout's `target/debug/bus` or an unqualified live `bus` command. F should consume C's committed fixture shapes after the join; synthetic fixtures are not evidence of native cards.

## A+D: execution directory and generated arguments

Reject native `--cwd` and `--worktree` overrides for managed Grok launches/resume: Bus's selected PWD determines the execution project and the project-local hooks we install. This includes supported `--name=value` forms and native aliases if the version-matched parser supports them. Preserve a literal `--` tail according to actual CLI parsing; do not reinterpret arbitrary prompt text as flags. Validate before installing hooks or initializing captures.

Deduplicate Bus-generated Grok flags inside A+D's owned `prepare` and `capture_args` paths. No shared `extend_generated_args` change is required. Preserve explicit permission choices and semantic value/option pairs, and test new launch plus cold resume with duplicates and literal tails.

These are decisions, not new worker file permissions. All existing lane file boundaries, clean test environments, resource limits and protections for live Bus / real Grok config continue to apply.

## Subsequent handoffs and G-owned routing follow-up

B is pushed at `c11d1a7b2a791f6a40019be431d48dee09365732`, directly on G0. G verified the remote SHA, direct parent and exact nine-file allowlist, and inspected the decoder/attestation implementation. B reports RED before GREEN, 30/30 `grok_b_`, 2/2 registration tests, isolated build/format/diff checks. G has merged all five lanes and rerun the joined focused tests. Worker worktrees are retained. Native completion and wake acceptance remain pending.

A+D may expose `pub(crate) fn resumable_options(args: &[String]) -> Vec<String>` from its owned `grok::launch` module. G owns the common `launch::arguments::resumable_options` delegate and will wire it after the join; no worker shared-file edits. Original `launch-options.json` stays verbatim; only derived resume arguments strip selectors. A+D's offline parser-error probe reports native `-cpTEXT`, `-cwNAME`, and `-cmMODEL` clustering. Reject the first two managed modes; test that stripping the continue selector from `-cmMODEL` preserves the model selection, alongside equivalent split flags and literal-tail cases. Do not infer unprobed short-option arities.

A+D's PATH prefix alone does not bind the executable after shell startup: startup can change PATH or define a `grok` function. G implemented carrying the validated absolute alias path through the new optional `AgentStartParams.executable` field (required for Grok), and replacing bare argv[0] in native resume before shell encoding. The source is the first PATH directory supplied by A+D after validation/revalidation; shell startup cannot change the absolute command. Missing/relative/invalid managed bindings fail closed. Other providers keep their existing invocation. A+D needs no additional file edits. Four synthetic routing regressions cover shell-function/PATH shadowing, quoted paths/arguments and invalid/missing bindings. This does not prove an authenticated Grok launch or reply journey; joint acceptance remains pending. G1 wires C dismissal and guarded observer ownership, the clustered-selector resume delegate, and durable bounded idle-prompt reconciliation. A dev-tools-only, owned-scratch adapter accepts F's explicitly synthetic completion records; production native success still requires the authenticated evidence adapter. E is merged, including its missing shared documentation inventory test entry.


## Joined fake acceptance boundary

`tools/acceptance/grok_fake.py --binary /absolute/isolated/dev/bus` runs the joined
fake providers against a fresh private Bus, empty home and closed PATH. It refuses
inherited Bus routing and the main checkout's debug binary. Synthetic aliases are
bound to the running dev executable during identity probes; real Grok is rejected
inside that test context. No credentials or real model tasks are used.

The fake composer displays the complete bootstrap in an enlarged PTY so ownership
can be established from visible text. This does **not** prove native long-paste
ownership. Collapsed chips, incomplete/wrapped frames and unrelated drafts stay
unavailable and are never cleared or submitted speculatively. A normal-size native
orchestrator bootstrap may therefore remain uncertain until an authenticated
capture establishes a safe delivery contract.

The fake orchestrators' decisions and dialog answers are scripted by the harness;
the actual Bus prompt, hooks, wake, routing, persistence and answer endpoints run.
Do not treat this as evidence that a model autonomously dispatched a task. Reports
carry the exact checkout head and binary hash. Native worker/orchestrator replies,
real cards/composer, cold resume and background auto-wake remain pending sign-in;
a completion toast or synthetic success cannot satisfy those checks.
