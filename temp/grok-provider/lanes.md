# Grok provider: G0 handoff and five lane contracts

**Integration branch:** `bus-grok` on `origin`; worktree `/Users/dylanliu/work/bus-bus-grok`.
**Upstream baseline:** `caa94d7705648e454400c2f8935a920fbec68fa9` (`origin/master` when G0 was created).
**Lane baseline:** the immutable G0 commit supplied with this handoff. Branch every lane from that exact commit, not a later origin/master or a moving lane branch. In the original G0 checkout it is HEAD. G creates no worker worktrees or worker agents in this G0-only task; the developer briefs them separately.

Approved decisions: both worker and orchestrator roles; v1 Bus-delivered wakes with ordinary authored sends; macOS / Grok Build 1.0.5 / fullscreen first; unknown allowance. **Native background wake verification is pending because Grok is not signed in. The developer has not accepted it as blocked or waived it.**

Post-G0 interface decisions are recorded in [lane-addendum.md](lane-addendum.md): B's explicitly Bus-defined completion-attestation contract and pending native evidence; G-owned Notification reconciliation; C's `shift+x` dismissal encoding and Esc-before-dismiss behavior; A+D's rejection of managed execution-directory overrides and local argument deduplication. The addendum clarifies the frozen G0 interfaces without moving the lane baseline.

G0 deliberately contains compiling stubs, as explicitly requested. Launch/resume, hook decoding, completion and prompt installation return explicit unfinished-operation errors; composer observation is unavailable; the fake refuses execution; allowance is genuinely unknown. G0 is not working end-to-end Grok support. Existing-provider behavior is preserved.

## Rules shared by all five lanes

- Use an isolated worktree/branch based on the supplied G0 SHA. Suggested names: `bus-grok-ad`, `bus-grok-b`, `bus-grok-c`, `bus-grok-e`, `bus-grok-f`. Use the worktree-new provisioning workflow with the developer's explicit G0 baseline; never accidentally create a worker on origin/master. If provisioning starts at origin/master, fast-forward the new lane to the pinned G0 commit before any edits.
- Edit **only the exact files listed for your lane**, including filling its G0-created new scaffold files. New test/fixture files below may be created; unlisted paths stay with G. No Cargo.toml/Cargo.lock, shared dispatcher or module-registration edits. Report an owner/interface gap to G instead of changing shared files.
- Test declarations belong inside a lane-owned module (`#[cfg(test)]` / `#[path]`); no root registration edits are needed. Use test names beginning `grok_ad_`, `grok_b_`, `grok_c_`, `grok_e_`, or `grok_f_` respectively. G's stable registration and invalid-directory checks remain applicable after implementations replace stubs.
- All commands run with `nice -n 19`; `CARGO_INCREMENTAL=0`. G uses `CARGO_TARGET_DIR=/Users/dylanliu/work/bus-targets/grok-g`, four build jobs/test threads on the 12-core host. For five parallel workers, coordinate a total below six active build/test jobs: **one build job, one test thread, RAYON_NUM_THREADS=1 per lane**, with separate target directories `/Users/dylanliu/work/bus-targets/grok-ad`, `grok-b`, `grok-c`, `grok-e`, `grok-f` to avoid shared output/locks.
- Clear inherited `BUS_*` and `HERDR_*` variables before running any test/harness. The existing test constructor also clears them. Any scratch Bus must supply its own new routing values. Never connect to live Bus or read/write `~/.local/share/bus`; never change the real `~/.grok/config.toml` or install global hooks. No real model work or secret-bearing prompts.
- No push/merge/main checkout changes are part of the lane contract unless the developer separately authorizes them. Return a precise file list, diff/commit identity, commands/results and remaining evidence gaps to G.
- Real Grok model turns/cards/native wakes need authentication and remain pending. Synthetic fixtures must say synthetic; current actual evidence contains only offline screens, session/shutdown hooks and transcript envelopes. Fakes do not establish native wake support.

### Focused test command pattern

Run commands in your own lane worktree with the environment scrubbed as above. Set these process-scoped values using a small subprocess wrapper (do not print environment values):

```python
lane_env = {k: v for k, v in os.environ.items()
            if not k.startswith(("BUS_", "HERDR_"))}
lane_env.update(CARGO_TARGET_DIR="/Users/dylanliu/work/bus-targets/grok-<lane>",
                CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1", RAYON_NUM_THREADS="1")
subprocess.run(["nice", "-n", "19", "cargo", "test", "--locked", "--bin", "bus",
                "--features", "dev-tools", "grok_<lane>_", "--", "--test-threads=1"],
               env=lane_env, check=True)
```

Replace `<lane>` with ad/b/c/e/f. Python test commands listed below use the same clean environment and `nice -n 19`. Format only owned Rust/Python files. Full gates and integrated E2E belong to G after the join; do not claim they ran from these focused commands.

## A+D: launch, args, project settings, resume and allowance

**Suggested branch:** `bus-grok-ad`. **Depends on:** G0 only. **Runtime integration tests after join:** B/C/E/F as applicable; those are not a prerequisite for authoring A+D unit tests.

Exact editable files:

- `src/agents/providers/grok/launch.rs`
- `src/agents/providers/grok/resume.rs`
- `src/agents/providers/grok/usage.rs`
- `src/agents/providers/grok/launch/arguments.rs` (optional new private child module, declared in your launch.rs)
- `src/agents/providers/grok/tests/launch_test.rs`
- `src/agents/providers/grok/tests/arguments_test.rs`
- `src/agents/providers/grok/tests/settings_test.rs`
- `src/agents/providers/grok/tests/resume_test.rs`
- `src/agents/providers/grok/tests/usage_test.rs`

Frozen interfaces (crate visibility; exact definitions are in the scaffold):

```rust
runtime_args() -> Vec<String>
resume_args(session: &str) -> Vec<String>
take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String>
option_arity(name: &str) -> Option<usize>
is_session_selector(name: &str) -> bool
validate_args(args: &[String]) -> Result<(), String>
resolve_executable() -> Result<PathBuf, String>
environment(hooks: &HookContext) -> Result<HashMap<String, String>, String>
setup_notice(root: &Path) -> SetupNotice
prepare(spec: &LaunchSpec,
        initialize: impl FnOnce(&HookContext) -> Result<(), String>)
    -> Result<PreparedLaunch, String>
resume::capture_args(hooks: &HookContext, project: &Path)
    -> Result<(PathBuf, Vec<String>, bool), String>
usage::allowance_unavailable_reason() -> &'static str
```

`prepare` owns the complete Grok preparation path: common `launch::prepare` delegates **before cwd checks, installation or spool initialization**. Call common cwd/token/argv/atomic-hook helpers as appropriate, preserve originals in `launch-options.json`, and call the provided initializer once at the correct capture boundary. Do not assume common prepare already did these steps.

Use `grok::HOOK_CONTRACT` / `HookContract::for_provider(ProviderKind::Grok)`; install project-root `.grok/hooks/bus.json` only with consent. No automatic folder trust. Native runtime args are `--no-leader --fullscreen --no-auto-update`; resume is `--resume UUID`. Preserve opaque options/literal tails and explicit permission flags; reject demonstrated noninteractive modes. Shared arity/selector/Yolo dispatch already delegates to you.

Prefer validated `~/.grok/bin/grok`, otherwise validate a PATH candidate as Grok Build; reject the regex namesake. Do not validate one executable then run another. Native launch/resume uses canonical command `grok`, so `environment` must supply a launch-scoped PATH with the validated alias directory first, plus `HookContext::env()`; `prepare` and cold-resume use the same resolution. Do not redirect GROK_HOME just to install settings. Fake identity validation must be possible with synthetic unit fixtures; do not require lane F's unfinished fake to run your tests.

Allowance stays unknown with an honest reason; no token-to-quota conversion or unsolicited OTEL. Persisted session options and adopted UUID semantics must survive cold resume; never create missing capture silently.

**Tests:** `grok_ad_` focused command; identity/path collision, argv round trip and `--`, resume versus new-session flags, explicit permission choices, consent absent => no files/initializer, user hooks preserved, atomic conflict, resume missing/mutated capture, private environment, allowance unknown. All fixtures use scratch directories.

**Done:** stubs replaced, unit tests pass in this branch, launch selection and saved options are deterministic and documented; unknown allowance retained; no real config writes; no existing shared file changes. Real worker QA is separately pending until authenticated joint acceptance.

## B: hook decoding and completion/transcript resolver

**Suggested branch:** `bus-grok-b`. **Depends on:** G0 only. **Consumes:** immutable common observation and completion contracts; no A+D helper calls required.

Exact editable files:

- `src/agents/providers/grok/hooks.rs`
- `src/agents/providers/grok/completion.rs`
- `src/agents/providers/grok/tests/hooks_test.rs`
- `src/agents/providers/grok/tests/completion_test.rs`
- `src/agents/providers/grok/tests/fixtures/session-start.json`
- `src/agents/providers/grok/tests/fixtures/shutdown-stop.json`
- `src/agents/providers/grok/tests/fixtures/turn-events.jsonl`
- `src/agents/providers/grok/tests/fixtures/updates.jsonl`
- `src/agents/providers/grok/tests/fixtures/README.md`

Frozen interfaces:

```rust
hooks::parse(value: &Value) -> Result<Parsed, String>
hooks::is_foreign_callback(adapter: ProviderKind, value: &Value) -> bool
completion::resolve_candidate(value: &Value, settled_idle: bool)
    -> Result<CompletionResolution, String>
enum CompletionResolution {
    Pending,
    Final { session: String, turn: String, text: String },
    Cancelled { session: String, turn: String, reason: String },
}
```

`Parsed::CompletionCandidate { session, turn }` is wired through session checks and per-turn record ordering; coordinator calls your resolver only after that check. Pending retains the original spool record. Final must return matching IDs; mismatch is rejected. Cancelled uses Bus's existing matched provider-error path with a cancellation reason, never a successful reply; native StopCancelled can also map to `Parsed::Failure` with an explicit reason. A different generic interruption API would require discussion with G rather than a shared enum edit.

The registered events are SessionStart, UserPromptSubmit, Stop, StopFailure, StopCancelled, Notification, nested command shape. Native fields are camelCase (`hookEventName`, `sessionId`, `promptId`, `transcriptPath`); event values are snake_case. Session-level shutdown Stop is not a final; subagent events are ignored. Imported legacy Claude/Cursor hooks must be filtered by `is_foreign_callback` without suppressing those providers' actual native hooks. The callback entry invokes this function before decoding/spooling.

Treat Stop as a candidate; verify actual completion and settled idle, handle continuation/cancel races, and recover clipped `lastAssistantMessage` from bounded session-matched transcript reads. Delayed/malformed/partial transcripts must remain pending or fail explicitly. A session-level idle Notification cannot identify a request's final by itself. Remove the G0 enum dead-code allowance in your completion.rs once variants are produced; G owns the common Parsed attribute.

**Tests:** `grok_b_` focused command; actual offline start/shutdown fixtures, wrong/absent identity, child/foreign event, gate continuations, cancel/failure after candidate, no final Stop, late report, long/clipped reply, partial/torn JSONL and ownership boundaries. Model-turn schema evidence is pending sign-in; label synthetic records and do not claim real native completion proof.

**Done:** no decoding/resolution stub; deterministic focused tests pass; no candidate published as a final on unsupported evidence; all output IDs scoped; fixture provenance explicit; real model-turn evidence gap returned to G. Do not change common callback or model files.

## C: Grok dialog parser, composer and screen rules

**Suggested branch:** `bus-grok-c`. **Depends on:** G0 only.

Exact editable files:

- `src/agents/dialog/grok.rs`
- `src/server/api/agents/prompt/grok.rs`
- `src/agents/manifest/bundled/grok-1.0.5.toml`
- `tests/fixtures/grok-dialog/permission.txt`
- `tests/fixtures/grok-dialog/trust.txt`
- `tests/fixtures/grok-dialog/question.txt`
- `tests/fixtures/grok-dialog/free-text.txt`
- `tests/fixtures/grok-dialog/parked-focus.txt`
- `tests/fixtures/grok-dialog/wrapped.txt`
- `tests/fixtures/grok-dialog/README.md`
- `tests/fixtures/grok-composer/empty.txt`
- `tests/fixtures/grok-composer/draft.txt`
- `tests/fixtures/grok-composer/pasted.txt`
- `tests/fixtures/grok-composer/README.md`

Put tests inline in the two owned Rust modules; no other test registration is needed.

Frozen interfaces:

```rust
// Inside agents::dialog::grok; pub(super)
parse(lines: &[&str], styles: &[Vec<Style>]) -> Option<Dialog>
contains_composer(lines: &[&str]) -> bool
// Inside server::api::agents::prompt::grok; pub(super)
observe(screen: &str, expected: &str) -> InputObservation
input(screen: &str) -> Option<String>
```

Shared parse/contains/observed submission already delegates to these functions. Existing Dialog/QuestionInput/option fields, keys_for, single-use digest and identity checks stay unchanged. Use contiguous API row numbers; existing per-option navigation_key can express checkbox toggles and question navigation. Include actual scope, focused row, check state, active question and input in the existing fingerprint context/labels. A parked card remains blocked; Esc is not a skip/approval; Grok questions dismiss with the documented shortcut.

The new manifest is initially a copy of the pre-existing 0.2.101 rules, **not verified 1.0.5 evidence**. Loader registration points to your new file; old grok.toml is read-only. Retain provenance and update only rules supported by actual capture or labelled synthetic tests. Support non-Kitty Ctrl+X footer hints. Do not infer readiness from a login screen.

**Tests:** `grok_c_` focused command; radio/checkbox/free-text/tab/scope states, stale digests, wrap/narrow views, quoted historical options, editable numbered drafts, unrelated draft preservation, long paste chips, unavailable composer while blocked. Real permission/trust/question captures are pending authentication; that limitation must stay explicit.

**Done:** no stub parsing/observation behavior for supported shapes; focused tests pass; unsupported states return unavailable/none and fail closed; fixture labels distinguish actual and synthetic; no legacy/shared parser changes.

## E: orchestrator instructions and prompt installation

**Suggested branch:** `bus-grok-e`. **Depends on:** G0 only. **Integration after join:** A+D enables launch, B/C provide accepted delivery and replies; no need to edit their files.

Exact editable files:

- `src/agents/providers/grok/system_prompt.rs`
- `src/agents/providers/grok/tests/system_prompt_test.rs`
- `orchestration/providers/grok.md`

Frozen interfaces:

```rust
orchestrator_prompt(text: &str) -> Result<String, String>
prompt_args(path: &Path, adopted: bool) -> Result<Option<Vec<String>>, String>
prompt_message(text: &str) -> String
```

G's coordinator fills room/name/docs values then calls `orchestrator_prompt` **for Grok only**. It receives the authored common prompt; derive a complete Grok prompt with unambiguous plain-send/Bus-wake instructions, so old Claude/Cursor `--async` text cannot govern Grok dispatch. Shared DOCS already embeds your guide at `docs/providers/grok.md`. Return None from prompt_args for the approved first-message bootstrap; prompt_message must be provider-local (no upward production import into messaging). Cold resume delegates prompt installation through the same function.

Use ordinary `bus send --room R --as ORCH --to WORKER --text ...`, return to idle, then process durable worker result/blocker notifications. Do not leave `send --async` waiters which acknowledge/suppress the wake. Preserve work-room versus MASTER final/report policy and fingerprinted worker-dialog commands. Do not claim native background wake works or is waived; verification remains pending sign-in.

**Tests:** `grok_e_` focused command; authored/custom text and placeholders preserved correctly, Grok dispatch protocol unambiguous, bootstrap and adopted/resumed policy, docs content, worker-dialog command guidance. Existing Claude/Codex/Cursor prompt behavior is a G-owned regression gate.

**Done:** installation errors replaced with the approved bootstrap policy; unit tests pass; document installed via existing shared table; no global prompt or shared docs edits. Joint worker-wake and MASTER delivery QA remains with G after the join.

## F: fake Grok and isolated acceptance tooling

**Suggested branch:** `bus-grok-f`. **Depends on:** G0 for independent authoring; consumes B/C fixtures when available. Full fake journeys need the joined A+D/B/C/E code.

Exact editable files:

- `src/devtools/tui/fake_agents/grok.rs`
- `tests/e2e-tui/grok-provider.md`
- `tools/acceptance/grok_provider.py`
- `tools/acceptance/grok_probe.py`
- `tools/tests/grok_e2e_test.py`
- `tools/tests/grok_probe_test.py`

Put Rust tests inline in fake_agents/grok.rs. Existing shared fake name registry includes `grok` and delegates before the generic provider path. Existing Python acceptance registers your COMMAND, delegates Grok case-support/dialog prompts and calls your case runner; no existing file edits are needed.

Frozen interfaces:

```rust
// fake_agents::grok; pub(super)
run(args: &[String]) -> i32
```

```python
# tools.acceptance.grok_provider
COMMAND: str
case_support(case: str) -> str | None  # reason means honest SKIP, not PASS
dialog_prompt(token: str) -> str
run_case(run, case: str, row: dict) -> None  # raise on failure
```

The run object is the existing tools.acceptance.e2e_test.Run; use its scratch bus, send/settle/screen helpers and bookkeeping. Existing cases include single/queued/steering/background/dialog/orchestrator/adoption/resume; use only evidence-supported cases and explain pending native-background acceptance. Generic multi-provider and resume journeys still use the shared harness. COMMAND must select the intended Grok Build alias; coordinate its fake version/help format with A+D's documented identity check.

Implement a deterministic fake with real-shaped camelCase hooks/project discovery, composer/cards, transcript, gate/clipping/error/cancel and wake scenarios. Do not invoke real Grok from a fake. Optional native probe must enforce an actual idle gap and a second model turn after a synthetic sleeping shell completes, with zero additional keyboard input. Return PENDING_AUTH when unsigned-in; no fake/offline output can produce a native PASS.

**Tests:** `grok_f_` Rust focused command; `nice -n 19 python3 -m unittest tools.tests.grok_e2e_test tools.tests.grok_probe_test` in a clean environment; parser/mock-process tests must not start real models or live Bus. Fake mixed-provider journeys are executed after the join against a scratch Bus, not on this unfinished G0 baseline.

**Done:** explicit fake refusal replaced; metadata and project hooks/JSONL/control surfaces simulated faithfully; unit/tooling tests pass; scenario checklist and provenance complete; pending authenticated tests remain pending. Return the post-join journey command and expected observations to G.

## Integrator G retains all shared files

G owns every changed file outside the lane lists, notably provider/mod/HookContract and common launch argument dispatch, callback entry/spool Parsed, coordinator candidate/session/request handling, CLI/UI provider choices and rendering, resume environment reconstruction, common orchestration/docs table, generic parser/composer delegates, manifest loader, shared fake registry, Python acceptance entry point and existing fixtures/tests. No lane edits `grok/mod.rs`, the G0 registration tests, original grok.toml, existing orchestration prompt/guide/CLI docs, or existing acceptance harness.

If an interface is insufficient, stop editing at that boundary and send G the concrete missing signature/behavior and a failing test/probe. G arbitrates a shared change; do not silently coordinate cross-lane edits. Individual lanes must not run full gates or claim a working fourth provider from local unit results.

## G0 verification and handoff status

- RED: Grok registration test failed with serde's unknown provider variant before registration.
- Compiles: `cargo check --locked --all-targets --features dev-tools` in G's isolated environment/target.
- Focused checks: G0 registration and invalid-directory safety; existing provider adapters/argv/resume; CLI hook consent; provider form/render; unknown usage; prompt delivery; fake executable installation. Result: **90 focused tests passed** (including the two G0 registration/safety tests); a separate initial run passed those two tests as well. Python delegate import/dispatch smoke passed; **195 focused architecture/test-placement checks passed**. Logs are under the integrator's `temp/grok-provider/`: `red.log`, `check.log`, `grok-tests.log`, `focused-tests.log`. All target checks exited zero with no compiler warnings.
- No full suite/gate, no real model, no real Grok config changes, no live Bus operation, no worker-lane implementation and no worker-agent dispatch were part of G0.
