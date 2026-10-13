# Permission handler coverage

## Run contract

Use e2e-test with the lane target, CARGO_INCREMENTAL=0, at 160x45 with kitty graphics. Run every case in a fresh scratch Bus; no SKIP. Clear BUS_DATA_DIR, BUS_SESSION_ID and every HERDR_* variable on every invocation. Never drive the live Bus or launch a real provider. Fake handlers exercise native dialog parsing, fingerprinted option 1, closure, audited decisions and clear; they do not prove real model judgment.

## Setup and evidence

Let WORK be the session scratch work path returned by start. Creating WORK/worker and WORK/shared is allowed fixture setup. Use scratch CLI to create permission-lab, a Claude orchestrator in MASTER bound to that room with cwd WORK, and an ordinary worker with the case provider and cwd WORK/worker (--consent-hooks). Do not add a handler: Bus must create exactly one managed Claude handler. Wait until it is idle and has a bound session, no reset required and no session_reset_pending; retain that session ID.

PHC-03 first creates the worker, then uses the scratch-only dev command `agent legacy-fixture WORKER`: it removes launch_cwd/backfill metadata from a serialized historical registration shape and reloads through the real state deserializer. This simulates a pre-existing legacy worker, keeps its native identity and never seeds a decision. Verify launch_cwd_backfilled=true and canonical frozen root before triggering. PHC-04 calls `agent allow-root WORKER WORK/shared --by ORCHESTRATOR` and verifies path, grantor and timestamp in state; never infer the grant from the prompt.

Trigger through the UI: click room:permission-lab, open ctrl+p, click recipient:worker, click composer, paste the exact directive, then press Enter, with one driver call per action. CLI is allowed only for setup/legacy migration/grant and read-only state, history, dialog and diagnostics. It must never choose or complete an assignment.

Preserve state, histories, effects, snapshots, trace.ndjson and the handler private fake-permission-cli.ndjson before stop. The fake validates real service launch mode/tool/allowlist configuration before a turn: require service_profile_validated=true and no failed protocol call. This validates the simulated provider configuration path, not all real Claude permission heuristics. Zero self-prompts requires no native handler dialog in captured state or explicit handler dialog inspection, successful service protocol work, bounded completion and reset, and no assignment timeout/fallback or handler actionable_error. Never use a hard-coded fake counter as prompt evidence. Require exactly one decision. No displayed build, script, API or push is executed. All safe cases require actual handler choose with option 1 and matching fingerprint/lease, closed outcome, and worker dialog null. Excluded cases require no choose/answer, an escalated completion and worker dialog still present. Capture a room screenshot for inspection; visual appearance is supplementary, semantic state/trace decides PASS. Always stop each scratch session.

### PHC-01 — Claude safe build

1. Complete setup with provider `claude` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission safe` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-02 — Codex safe build

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission safe` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-03 — Legacy pre-existing worker

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission safe` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-04 — Granted shared root

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission shared` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-05 — Truncated opaque script

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission truncated` through the exact UI sequence. Wait up to 60s for one `escalated` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-06 — Read-only GitHub GET

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission gh-read` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-07 — GitHub POST

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission gh-post` through the exact UI sequence. Wait up to 60s for one `escalated` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-08 — Protected master push

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission master-push` through the exact UI sequence. Wait up to 60s for one `escalated` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-09 — Complete visible script

1. Complete setup with provider `claude` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission visible-script` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.

### PHC-10 — Collapsed Codex permission

1. Complete setup with provider `codex` and the case fixture above. Verify the auto-handler is bound/idle and the worker authority in state.
2. Trigger `fake:permission collapsed` through the exact UI sequence. Wait up to 60s for one `approved` decision. Preserve native dialog, fingerprint, service configuration validation and CLI evidence; verify the safe/excluded assertions above.
3. Wait up to 30s for a different bound handler session, idle status and all reset latches false. Capture final state, both room histories, effects, snapshot and screenshot. Verify zero self-prompts, zero fallback/timeout, no handler error and no duplicate decision. Stop the scratch Bus.
