# Grok Provider Implementation Plan

> **For agentic workers:** Use the approved repository execution workflow, with superpowers:subagent-driven-development or superpowers:executing-plans as appropriate. Track the steps below with checkboxes. The developer has approved Grok support and authorized integrator G to complete G0 only, including compiling stubs, focused tests, commit and push.

**Goal:** Support Grok Build as both a managed Bus worker and a MASTER orchestrator of one work room, preserving durable reply ownership, safe dialogs and restart/resume behavior.

**Architecture:** Reuse Bus's PTY, observation spool, request settlement and durable worker notifications. Adapt Claude-style Grok lifecycle events through a dedicated decoder and transcript resolver; install project hooks using Cursor-style consent. Use ordinary authored sends and Bus-delivered wakes for Grok orchestration until a real native-background wake test passes.

**Tech stack:** Existing Rust provider/server/messaging code, JSON hook/ACP transcript formats, TOML screen manifests, Python acceptance harness and the existing fake-agent TUI driver. No ACP transport rewrite or new external service.

**Spec:** [assessment.md](assessment.md), including its evidence limitations and developer questions.

**Baseline:** `master` at `caa94d7705648e454400c2f8935a920fbec68fa9`, Grok Build `1.0.5 (5115b46bc909)` on macOS arm64. Recheck the implementation branch's baseline before starting. This is the requested parallel implementation breakdown, with the v1 defaults approved by the developer; only G0 is authorized for this integrator turn.

## Decisions and constraints

Already decided by the developer:

- Build **both** worker and orchestrator roles.
- Native-background wake verification is **pending because Grok is not signed in**. The developer did not accept it as blocked or waive the test; no pass is inferred from docs or fakes.
- Deliver this plan and assessment as temporary artifacts; no product code changes in this investigation.

Approved v1 decisions (confirmed by the developer):

- macOS / inspected Grok Build 1.0.5 first; fullscreen interactive TUI; other platforms/modes require their own evidence.
- Grok orchestration uses plain `bus send --as ORCH`, not a background `send --async` waiter, and relies on Bus's existing durable reply/blocker notifications.
- Allowance is explicitly unknown in v1, matching Cursor. No numeric usage/quota schema is invented.

Global constraints for implementation:

- Never operate against live Bus or `~/.local/share/bus`. Use isolated feature worktrees and uniquely named scratch sessions. All commands use `nice -n 19`; children inherit it.
- No writes to the developer's `~/.grok/config.toml`, no global hook install, no implicit folder trust or permission bypass. Project installation requires existing consent UX; Grok folder trust remains separate and covers MCP/LSP as well as hooks.
- Clear inherited `BUS_*` and `HERDR_*` before starting any QA harness; only let the scratch Bus add its own routing variables. Use current master's test/gate isolation helpers. Never run acceptance_existing_instance against the user's session.
- Hook callbacks remain observation-only. Preserve launch/session/turn provenance and deduplication. Do not retry prompts whose submission is uncertain, publish partial answers as complete, or bind a wake to the wrong request.
- Keep unrelated providers' argv, permission choices, Codex orchestrator refusal, report routing and hooks unchanged.
- No dependency install, build, product edit, commit, push or live-model test has been performed by this planning task. Tests listed here are future commands and acceptance requirements.

## Review focus

Every item below has an owning test lane:

- Stop gates continue or are interrupted after the observer fires: B/G must withhold final settlement until the owned turn really ends.
- Compatibility-imported Claude/Cursor hooks fire from Grok, or a shared leader inherits the wrong launch: A/B/G must prevent cross-provider and cross-launch binding.
- A card parks focus, changes scope, or hides an editable free-text field: C/G must send no stale/unintended approval.
- A worker completion races a developer MASTER follow-up or a Bus restart: E/F/G must deliver once, preserve the earlier MASTER answer and keep the new wake's provenance.
- A final exceeds 32,768 characters or its JSONL tail is incomplete: B/F/G must recover the full matching text or remain explicitly pending/failed, never claim a clipped success.

## Shared ownership and execution order

The G0 override below supersedes the earlier non-stub assembly approach. Use **one integration owner, G**, for every shared file below, across all worktrees. Parallel lanes A–F own only their dedicated files; they send required shared edits to G instead of making conflicting changes. G also owns module declarations and common test-module registration.

Order:

```text
G0: freeze contracts, file owners, evidence prerequisites
             |
     +-------+-------+-------+-------+-------+
        A+D       B       C       E       F   (parallel authoring)
     +-------+-------+-------+-------+-------+
             |
G1: assemble registrations, integrate lifecycle and run focused gates
             |
G2: deterministic mixed-provider QA, full final gates, evidence handoff
```

There are now five parallel Codex lanes: **A+D**, **B**, **C**, **E**, **F**. All lanes consume the same pinned G0 commit. G1 consumes all lane outputs. G2 consumes a compiling, passing G1 candidate. There are no lane-to-lane **write** dependencies. B/C expose fixture contracts early so F can author against them; final F integration tests wait for G1. No lane is declared green merely because an unregistered Rust file was not compiled.

**How parallel modules are tested:** G registers each lane's completed, pure modules for test compilation in that lane's worktree, or assembles them in the integration worktree. Only G edits shared/module files. The new ProviderKind variant and all exhaustive dispatch matches land together at G1 after the modules exist; explicit compiling stubs are supplied as authorized by the G0 override; unfinished operations return errors rather than fake success. Task-local gates run on that assembled tree, and final evidence is regenerated on the final tree. This distinguishes parallel source work from independent deployability.

| Shared paths | Exclusive owner | Required changes / consumers |
|---|---|---|
| `src/agents/providers/mod.rs`, new `src/agents/providers/grok/mod.rs` | G | Module registration, `ProviderKind::Grok`, canonical label, executable dispatch, parser dispatch, child module declarations; all lanes consume. |
| `src/agents/providers/launch.rs`, `launch/arguments.rs`, `hook_json.rs`, `callback_entry.rs`, `spool/parsed_event.rs` | G | Hook contract, option arities, runtime flags, Grok callback entry, completion-candidate and cancellation handling if needed. A/B supply provider functions; they do not edit these files. |
| `src/messaging/identity.rs`, `src/cli/control/parse.rs`, `src/cli/help.rs`, `src/messaging/coordinator/control/agents.rs`, `src/client/rooms/dialogs/forms.rs`, `src/client/rooms/input/forms.rs` | G | Provider/API/form registration, worker/MASTER choices, clear/interrupt commands and CLI help. Keep Codex policy intact. |
| `src/messaging/coordinator/agents.rs`, `resume/capture.rs`, `resume/capture/ownership.rs`, `startup.rs` | G | Selected executable routing, readiness/session binding, cold-resume hook validation and prompt delivery; consume A/E. |
| `src/messaging/coordinator/callbacks.rs`, `callback_binding.rs`, `provider_turns.rs`, `src/messaging/model/callbacks.rs`, `settlement.rs`, `src/messaging/coordinator/reports.rs` | G | Invoke B's candidate resolver; map failures/cancels, nested/foreign events and wake provenance; preserve report destinations. Change only direct Grok integration needs. |
| `src/agents/dialog.rs`, `dialog/composer.rs`, `src/server/api/agents/dialog.rs`, `prompt.rs`, `prompt/composer.rs`, `src/terminal/runtime/dialog.rs` | G | Invoke C, preserve digest/single-use checks, Grok composer observation and skip/navigation semantics. C owns new parser implementation/fixtures. |
| `src/messaging/coordinator/usage.rs`, `src/messaging/orchestration.rs` | G | Unknown Grok allowance (D) and provider-specific prompt text/delivery (E). |
| `src/devtools/tui/fake_agents.rs`, `src/devtools/tui/mod.rs`, `src/devtools/tui/tests/fake_agents_test.rs` | G | Register/delegate to F's fake Grok; add a fourth executable without conflating real CLI identity checks. |
| Existing common test files under `src/agents/providers/tests/`, CLI/form/coordinator/server tests, `src/agents/resume/catalog.rs` | G | Exhaustive-match fallout and integration assertions. Native Grok resume already exists; preserve it and add managed coverage. |
| Existing notification model files `src/messaging/model/reply_routing.rs`, `worker_events.rs`, `worker_notifications.rs`, coordinator `delivery.rs` | G | Reuse first. Modify only if a deterministic test demonstrates a Grok integration gap; E/F must not fork another delivery queue. |

A newly discovered shared path is assigned to G before editing. A newly discovered dedicated path is added to its lane's contract. No two lanes own the same file or directory wildcard.

## G0: freeze interfaces and evidence prerequisites

**Owner:** G. **Dependencies:** none. **Produces:** immutable lane contract and test-assembly conventions, not placeholder product code.

- [ ] Verify branch/dirty baseline and exact real CLI path. Publish final baseline SHA and owner table. Keep the main checkout unchanged during implementation.
- [ ] Freeze `grok` as the persisted provider spelling and `grok-hook` as the new callback adapter. Freeze nested command-hook registrations for SessionStart/UserPromptSubmit/Stop/StopFailure/StopCancelled/Notification; filter Notification by payload rather than requiring a global generic installer redesign.
- [ ] Freeze private-runtime launch and the intended resume behavior. G owns the shared constant/HookContract; A and B consume the same bytes and event names.
- [ ] Freeze B's completion-candidate contract before adding ProviderKind. Existing Parsed::Final must continue to mean a publishable final observation, not an arbitrary Grok stop-gate attempt.
- [ ] Publish captured fixtures from this investigation as **session/shutdown only**, and distinguish unobserved turn/dialog/usage schemas. Do not transform hypothetical samples into “real 1.0.5” evidence.

Planned module interfaces, implemented by their lane and wired by G:

| Lane | Interface | Contract |
|---|---|---|
| A | `grok::launch::{runtime_args, resume_args, take_adopted_session, setup_notice}`; `grok::resume::capture_args` | Follow existing adapter return types: Vec<String>, Result<Option<String>,String>, SetupNotice, and Result<(PathBuf,Vec<String>,bool),String>. Inputs stay room-free. |
| A | `grok::launch::resolve_executable()` → `Result<PathBuf,String>` | Returns a validated launch path ending in the canonical executable alias, without running any model work. G propagates its resolution to launch **and** resume. |
| B | `grok::hooks::parse(&Value)` → `Result<Parsed,String>` | Produces Session/Started/Failure/Ignore and a specifically named candidate/cancel observation agreed with G; never emits a gate-attempt success directly. |
| B | `grok::completion::resolve_candidate(value:&Value, settled_idle:bool)` → `Result<CompletionResolution,String>` | Resolution enum: Pending, Final{session,turn,text}, Cancelled{session,turn,reason}. Requires owned turn metadata and authoritative completion evidence; no guessing from newest assistant text. |
| C | `agents::dialog::grok::parse(lines:&[&str], styles:&[Vec<Style>])` → `Option<Dialog>` | Actual style type matches the existing parser's `ratatui::style::Style`. Child module uses existing Dialog/QuestionInput/option types; no independent approval API. |
| C | `grok composer observation` → existing `InputObservation` | Extend the current prompt observer through G; no second input sender or retry loop. |
| D | `grok::usage::allowance_unavailable_reason()` → `&'static str` | Explicit stable explanation for unknown allowance. Consumed by G's existing state JSON path. Numeric quota fields remain absent. |
| E | `grok::system_prompt::prompt_args(path:&Path, adopted:bool)` → `Result<Option<Vec<String>>,String>` and `prompt_message(text:&str)` → `String` | Return None for first-message bootstrap in v1. Grok-specific authored instructions select the Bus-delivered wake protocol. |
| F | dedicated `fake_agents/grok.rs` entry returning an exit code | Register through G's shared fake dispatch. Simulates this version's contract; never invokes a real model. |

The exact transcript fields that prove a model turn completed and the actual 1.0.5 permission/card rendering need authenticated synthetic evidence or version-matching authoritative source before production acceptance. **B and C can implement documented parsing and synthetic tests in parallel, but those tests cannot discharge these evidence prerequisites.** No unconditional native orchestration milestone depends on the pending native wake probe because v1 uses Bus-delivered wakes.

## A: provider launch, args, project settings and resume

**Owns new files:** `src/agents/providers/grok/launch.rs`, `arguments.rs`, `resume.rs`, `tests/launch_test.rs`, `tests/resume_test.rs`, `tests/settings_test.rs`.

**blocked-by:** G0. **Consumes:** shared HookContract/HookContext. **Produces:** launch/resume functions and validated executable selection. **Reference:** Cursor launch/resume + common hook_json; Codex private-runtime rationale.

- [ ] Write failing tests named with `grok_`: real install path wins over regex PATH collision; correct fake scratch path can be resolved; unknown CLI identity rejected; UUID resume/adoption; title selectors remain opaque native input; `--session-id` is creation, not resume; literal `--`/quoted values/unknown options preserved.
- [ ] Implement executable selection: prefer validated `~/.grok/bin/grok`, then a verified Grok Build PATH candidate. Validate version/help without prompts. Keep selected executable resolution identical in availability, launch and resume. The current native launcher uses a canonical command name; either prepend the validated alias directory to the **launch-scoped PATH** and persist/revalidate it on resume, or make the smallest explicit executable plumbing change with G. Never validate one path then execute a different PATH result.
- [ ] Implement interactive args: `--no-leader`, fullscreen first-version surface, no auto-update; avoid duplicate generated flags. Preserve explicit permission/allow/deny options; add bypass only for Bus's explicit run-scoped setting. Reject one-shot/agent/export/etc modes which replace the managed interactive process, with tests proving the restriction.
- [ ] Use common owned-hook merge for `<project-root>/.grok/hooks/bus.json`, consent notice and locked atomic writes. Missing consent writes nothing; preserve user hooks and reject ambiguous ownership. Do not write trust or user config.
- [ ] Resume only an attested launch/session/spool, rebind exact owned hooks to the current binary, and restore private runtime/executable selection. Missing/mutated capture fails closed.
- [ ] With G's module registration, run `nice -n 19 just test-one grok` on an isolated test environment; expected all A tests pass. Also run existing argv/consent/resume tests after G1. Record actual test names and candidate SHA in evidence.

## B: hook bridge and completion/transcript resolution

**Owns new files:** `src/agents/providers/grok/hooks.rs`, `completion.rs`, `tests/hooks_test.rs`, `tests/completion_test.rs`, `tests/fixtures/` **only within the Grok provider directory**.

**blocked-by:** G0. **Consumes:** common Parsed/callback contracts and A-independent HookContext semantics. **Produces:** parsed facts, completion resolver, sanitized fixtures. **Reference:** Claude hooks, Cursor final_reply, Codex bounded rollout readers.

- [ ] Write red tests for actual session_start/session_end/shutdown Stop fixtures; camelCase sessionId/promptId, `source:new`, missing required fields, unknown events, subagentType, nested/compatibility events and post-restart delayed callbacks.
- [ ] Implement event normalization explicitly. Turn IDs remain opaque and scoped to session/launch. StopFailure text becomes an error; StopCancelled closes the matching turn as cancelled/interrupted, never a success. Ignore session-level shutdown Stop without promptId. Ignore foreign adapter events without poisoning the active request.
- [ ] Add a completion-candidate observation with G if existing Parsed variants cannot express the distinction. G alone edits the shared enum and dispatcher. Decoder success does not authorize request settlement.
- [ ] Implement bounded, partial-line-safe reading of owned `transcriptPath`, checking session/turn identity and committed completion evidence. Recover answers clipped at 32,768 characters. A transcript lag, continued gate, newer cancel/failure, malformed suffix or unresolved identity yields Pending/error with no prompt retry or published final.
- [ ] Add red cases: two Stop gate attempts in one turn, continuation-cap path without final Stop, cancellation after Stop capture, late report following the next submit, transcript unavailable, truncated JSONL, long final, nonfinal commentary, and stale session path. Session-level idle-prompt Notification triggers reconciliation only; it cannot attribute an answer by itself.
- [ ] Obtain a version-bound synthetic model-turn transcript before declaring real turn-final support accepted. Authentication is currently unavailable; native wake verification remains pending; if still unavailable, mark real evidence unrun and keep the release gate open. Do not invent a final-marker schema.
- [ ] On G's assembled registration, run `nice -n 19 just test-one grok`, plus existing callback/reply-settlement tests selected by G; require all deterministic cases green and no existing provider behavior changes.

## C: Grok dialog parser, composer and screen fixtures

**Owns new files:** `src/agents/dialog/grok.rs`, `src/agents/tests/grok_dialog_test.rs`, `src/server/api/agents/tests/grok_composer_test.rs`, `tests/fixtures/grok-dialog/`, `tests/fixtures/grok-composer/`, and dedicated `src/agents/manifest/tests/grok_test.rs`. **Owns modification:** `src/agents/manifest/bundled/grok.toml`. G owns their parent module wiring and shared parser/runtime files.

**blocked-by:** G0. **Produces:** parser, input observations and version-labelled screen fixtures. **Reference:** Claude specialized parser, generic Dialog fingerprints, existing 0.2.101 Grok manifest.

- [ ] Capture 1.0.5 permission, folder-trust, single/multi-choice, focused free-text, narrow/wrapped and parked-focus surfaces using only synthetic tasks when authenticated access is available. Until then label generated examples synthetic; preserve the current offline welcome/modal capture as actual evidence.
- [ ] Write red parser tests: `┃ N (○)/(●)` and letter/free-text rows, focus versus selection, active question changes, permission scope changes, long wrapped labels, quotes in transcript and numbered text in an editable composer. A dismissed-looking but parked card must remain blocked.
- [ ] Implement Grok-specific parsing through existing Dialog types. Prefer existing arrow/Enter navigation from a visible selected row; do not type arbitrary digits/letters into a possibly changed surface. Explicitly represent question navigation and focused custom input. Skip a Grok question using the documented dismiss command, not Esc's focus-parking behavior.
- [ ] Through G, include card/tab/scope/input state in digest identity, keep single-use fingerprints, and add Grok to observed PTY submission. Confirm multiline text/paste-chip matching and refusal to overwrite an unrelated draft or submit into login/trust/cards.
- [ ] Refresh only evidence-supported manifest rules. Cover Ctrl+X shortcut hints on non-Kitty terminals; current old rules requiring Ctrl+. cannot be treated as universal 1.0.5 readiness proof.
- [ ] Run `nice -n 19 just test-one grok` after G registration. G additionally runs generic/Claude/Codex/Cursor dialog and prompt regressions on the assembled tree. Permission/card real captures remain an explicit acceptance gap until available.

## D: usage and allowance policy

**Owns new files:** `src/agents/providers/grok/usage.rs`, `tests/usage_test.rs`. G owns coordinator state JSON and module wiring.

**blocked-by:** G0. **Produces:** explicit unknown allowance reason and tests; no numeric quota source required for v1. **Reference:** Cursor's unknown allowance in coordinator usage.rs; provider-neutral UsageWindows.

- [ ] Write red tests that Grok allowance serializes as unknown with a reason, with no fabricated percentages/reset times. Malformed/missing usage observations must not create a quota-exhaustion alert.
- [ ] Implement the explicit absence policy; G adds it to existing state JSON. Keep consumed tokens/cost conceptually separate from remaining subscription allowance.
- [ ] Document verified future sources: session signals.json, headless usage and ACP/OTEL, with their provenance and incompleteness flags. Do not enable telemetry or scrape the user's usage panel in the background.
- [ ] If numeric session usage is later required, make schema capture a separate follow-up input and add missing/partial/stale/wrong-session cases before implementing it. This is not silently included in the v1 effort.
- [ ] Run `nice -n 19 just test-one grok` and existing coordinator usage tests through G. D is intentionally small and can finish while other lanes continue.

## E: orchestrator docs, authored prompt and Bus wake protocol

**Owns new files:** `src/agents/providers/grok/system_prompt.rs`, `tests/system_prompt_test.rs`, and `orchestration/providers/grok.md`. **Owns modifications:** `orchestration/prompt.md`, `orchestration/guide.md`, `orchestration/how-to-bus-cli.md`. G owns common orchestration.rs and coordinator/model integration.

**blocked-by:** G0. **Consumes:** frozen plain-send/Bus-wake choice. **Produces:** provider-specific instruction content and bootstrap functions. **Reference:** Cursor first-message installation; common orchestration prompt/docs; worker_notifications.rs and reply_routing.rs.

- [ ] Write red tests that the authored Grok prompt bootstraps as the first observed delivery-only message and selects ordinary `bus send --room R --as ORCH --to WORKER --text ...`, then returns to idle. Existing Claude/Cursor async instructions and Codex refusal must remain present and unchanged in behavior.
- [ ] Add the Grok-specific instruction appendix through common docs installation. Clarify that `--async` blocks the CLI and should not be used by the initial Grok protocol; no long-running waiter should consume/ack replies that Bus must inject.
- [ ] Reuse queued worker-result and blocked/resolved notices. G verifies that replies are authored by the orchestrator and remain eligible for the existing retained/deduplicated wake route. Read-only status/history queries do not mark a reply seen.
- [ ] Test bootstrap readiness, cold resume, worker answer wake, worker failure/blocker wake, and developer follow-up while a notification is pending. Wake acknowledgement must follow accepted delivery; a failed send leaves durable pending work.
- [ ] Preserve MASTER rules: a final answering a developer MASTER message belongs there; spontaneous status goes to the work room; explicit `send --to human` can report to MASTER. Apply identical fingerprinted worker-dialog commands and risk policy.
- [ ] Document native background optimization as pending/unverified, with the exact no-keyboard second-turn test from assessment.md. Do not enable it from the fact that a completion notification exists.
- [ ] Run `nice -n 19 just test-one grok` and common orchestration/report/reply-routing tests on G's assembled tree; no new delivery queue should be necessary.

## F: fake Grok and E2E support

**Owns new files:** `src/devtools/tui/fake_agents/grok.rs`, `src/devtools/tui/tests/grok_fake_agent_test.rs`, `tests/e2e-tui/grok-provider.md`, `tools/acceptance/grok_probe.py`, `tools/tests/grok_probe_test.py`, `tools/tests/grok_e2e_test.py`. **Owns modifications:** `tools/acceptance/e2e_test.py` and `docs/tui-driver.md`. G owns shared fake dispatch/registry Rust files and common integration-test wiring.

**blocked-by:** G0. **Consumes:** frozen hook/dialog/wake contracts; B/C fixtures as they become available. **Produces:** deterministic fake implementation, acceptance cases and native-wake probe tooling.

- [ ] Write fake Grok discovery, argv and hook tests first. Use the real nested project hook shape and camelCase payloads. Its version/help identify the intended fake Grok Build, satisfying the resolver without bypassing production identity checks.
- [ ] Implement full idle/composer/working/cards/cancel behavior; synthesize a real-shaped transcript and long-answer/gate-attempt/failure paths. Leave unknown real fields visibly synthetic, with fixture provenance.
- [ ] Add scratch journeys for worker round trip; queued prompts; dialog observe/choose/answer with stale fingerprints; Grok orchestrator dispatch to an existing fake worker; an existing orchestrator dispatch to fake Grok; MASTER reply while worker work continues; work-room wake; and restart before wake delivery. Assert exactly-once final routing and preserved pending work.
- [ ] Extend Python provider/case registration and installed-executable resolution. Do not retain assumptions that background support is uniquely Claude or accidentally make the existing-instance live-session harness the default test.
- [ ] Implement an optional real Grok native-wake probe using only a scratch sleep/sentinel command. Require genuine idle between first turn and shell completion, a new model turn after completion, unchanged keyboard input count, and a matching second final. Distinguish FAIL, PASS, PENDING_AUTH and TIMEOUT; neither offline smoke nor fake output can yield a native PASS.
- [ ] Run `nice -n 19 python3 -m unittest tools.tests.grok_probe_test tools.tests.grok_e2e_test` and `nice -n 19 just test-one grok` after G registration, plus the supported scratch TUI journeys built from the candidate tree. Authenticated acceptance remains pending until Grok is signed in and is reported separately.

## G1/G2: shared integration and verification

**Owner:** G, with the shared paths table as its exclusive write boundary and dedicated integration regression files `src/messaging/coordinator/tests/grok_provider_test.rs` / `src/server/api/agents/tests/grok_test.rs` as needed.

**blocked-by G1:** A, B, C, D, E, F source outputs. **blocked-by G2:** G1 focused gates. **Produces:** coherent fourth-provider support and exact-tree evidence.

- [ ] Add Grok enum/dispatch/CLI/UI registrations together, including MASTER selection and the existing native-pane identity/resume plan. Preserve all pre-existing providers and saved session compatibility.
- [ ] Wire A's exact executable and private runtime through availability, interactive launch and cold resume. Test the installed symlink identity and downloaded binary process name; fix identification only for demonstrated Grok Build identity, not an unrestricted prefix matching regex tools.
- [ ] Wire B's candidate/final/error/cancel facts without weakening request ownership. Filter compatibility-imported legacy hooks and nested provider launches before they can create the wrong binding or actionable error. Preserve user callbacks and `{}` observation stdout.
- [ ] Wire C's parser, composer and skip/navigation behavior with single-use fingerprints and no stale approval. Wire D's unknown allowance and E's prompt/docs plus durable wakes.
- [ ] Run focused new tests and existing launch/resume, dialogs/composer, callbacks/settlement, MASTER reports, worker-reply routing and usage suites. Exact command example after compilation: `nice -n 19 just test-one grok`; additional filters follow actual existing test names. A lane's passing result is bound to the assembled candidate, not an earlier standalone file.
- [ ] Run deterministic scratch mixed-provider E2E on this same tree. Include one Grok worker and one Grok orchestrator; exercise replies, a worker dialog, MASTER routing, failure/cancel and a Bus restart. Never connect to the existing instance.
- [ ] Run final full commands in order, from an environment scrubbed of inherited live Bus routing: `nice -n 19 just lint`, then `nice -n 19 just test`, then `nice -n 19 just build`. Run repository coverage/gate requirements and applicable platform CI; repeat final checks only if the tree changes or a relevant failure requires it.
- [ ] Record SHA/tree, Grok binary hash/version, fixture provenance, tests and screenshots/transcripts. Product behavior that requires authenticated evidence stays unrun/blocked until it has that evidence. Do not claim full real-worker acceptance merely from the fake.
- [ ] Hand off the reviewed implementation/evidence using the developer's chosen repository workflow. This plan does not authorize a push, merge, or operation on the live Bus session.

## Acceptance matrix and unresolved evidence

| Behavior | Required proof | Current state |
|---|---|---|
| Worker launch, hooks, dialog, reply, resume | Deterministic tests plus synthetic authenticated Grok round trip | Plan only; session/shutdown smoke available, model turn/cards unrun. |
| Grok orchestrator with Bus-delivered wakes | Mixed-provider fake E2E and real scratch two-agent round trip; no native waiter | Plan only; underlying Bus notification code verified by inspection. |
| Native background orchestrator | No-input sleep/sentinel probe, genuine idle gap and second model turn | **PENDING_AUTH**, pending because Grok is not signed in; not waived by the developer. Not required for the proposed Bus-wake protocol. |
| Long final and Stop gate continuation | Exact 1.0.5 final-boundary evidence plus deterministic recovery/race tests | Model transcript schema capture outstanding; release gate for full reply correctness. |
| Grok permission/trust/question automation | Real 1.0.5 fixtures, narrow/wrapped/parked-focus tests, fingerprint E2E | Real card captures outstanding. Unknown layouts must fail closed. |
| Allowance | Explicit unknown state and no fabricated/exhaustion data | Chosen v1 policy proposed; no numeric contract established. |

The authentication gap does not justify a worker-only deliverable: **both roles remain in scope**. It prevents honest claims of real model acceptance. The Bus-wake design removes dependency on the specific native-background wake feature, while all ordinary real-provider acceptance still needs an authenticated test environment before release.

## Effort and developer questions

Total estimate: **medium, about 7–12 engineer-days**, with A–F parallel source work and one shared integration owner. The largest pieces are B's completion correctness, C's version-specific cards/composer, and F/G's both-role/restart acceptance. Native-background parity or a new stable subscription-allowance source can raise this to large.

All three v1 choices are approved: Bus-delivered wakes; macOS / Grok 1.0.5 / fullscreen first; unknown allowance. Native wake verification remains pending until Grok sign-in, with no waiver.

## Approved G0 override

The developer explicitly requested compiling stubs and every shared-file registration before parallel work. G0 therefore registers ProviderKind::Grok, supplies explicit unfinished-operation errors, and commits all shared delegates. The authoritative interfaces, exact five-lane file lists, test commands and done criteria are in [lanes.md](lanes.md); they supersede the earlier task split and any conflicting signatures or shared-file ownership above. A+D is one lane. Lanes only fill their own new scaffold files and add the listed new test/fixture files; shared files stay with G. G0 does not claim working end-to-end provider support.
