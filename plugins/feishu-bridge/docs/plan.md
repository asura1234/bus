# Feishu bridge implementation plan

Spec: ../INTERFACE.md, v1 at d732a76. The supplied workflow and locked decisions
authorize direct execution; no new product/design approval is needed.

Architecture: SQLite FULL/WAL spool, separate transport ingestion and dispatch
connections, one app advisory lock, bot-only lark-cli subprocess boundary.
Each task follows RED, observed assertion failure, GREEN, full suite, commit.
Reviewers are allocated by bus master after implementation; no local master
fast-forward before both reviewers are Ready.

Global constraints: macOS, local git only, no real sends or launchd load, no
credential/config/scope mutation, shared cargo lock and jobs 3, no incremental
compilation, target /Users/dylanliu/work/.cargo-targets/feishu-bridge. Minimum
15 GiB free per task brief (current workflow says 30 GiB before build; use 30).

Review focus: crash after sink acceptance; partial/repeated history pagination;
old roots receiving new thread replies; competing same-app listeners with
different state dirs; slow sinks while events arrive. Tests exercise each.

## Task 1: CLI, authorization, spool, send

Files: src/{main,config,model,process,store,delivery}.rs, tests/contract.rs.
Public boundary: CLI and sink JSON in INTERFACE.md; no Rust API contract.

- [x] RED: binary integration tests for literal/idempotent send, app mismatch,
  dual-identity authorization, decoded text, durable replay, sink refusal,
  per-chat ordering, config validation and parked revoked pending messages.
- [x] Verify assertion failures using cargo test, under assigned shared lock.
- [x] Implement only covered behaviors; SQLite primary key (app,message_id),
  persisted acceptance seq, delivered markers, no send retries.
- [x] GREEN: cargo test; commit task 1.

## Task 2: paginated recovery and daemon

Files: src/{history,listener}.rs; extend tests/contract.rs; unit fixture tests.

- [x] RED: fake lark-cli history with multiple pages, partial failure watermark
  rollback, repeated token refusal, old-root thread pagination, event readiness,
  late events, sink-down event spooling and app singleton lock.
- [x] Verify failures before implementation.
- [x] Implement fixed-end overlapping chat scans and stored roots/threads;
  transactionally merge sorted recovery batch, then advance watermark.
  Consumer thread spools directly while sinks run; terminate child groups.
- [x] GREEN: cargo test; commit task 2.

## Task 3: supervision and final verification

Files: src/launchd.rs, README.md, tests/contract.rs, temp/evidence/.

- [x] RED: status counts/lock and install/uninstall private-HOME lifecycle;
  verify no launchctl invocations and state preserved.
- [x] Implement lifecycle; GREEN: full suite; commit.
- [x] Bus master 49235 authorized the complete final batch in this hold; cargo fmt --check, clippy -D warnings,
  cargo test, cargo build --locked, record exact head and disk evidence.
- [ ] Real bounded bridge/lark-cli smoke: listener ready + websocket connected, zero
  events OK; no sends, auth changes, launchd load or developer interaction.
- [ ] Send bus master branch/head/evidence path for reviewer allocation.

## Execution ledger

- Contract committed d732a76; task 1 tests/scaffolding prepared before cargo slot.
- Ruling: no extra worktree for new standalone repo; feature branch is already
  isolated from Bus, as explicitly assigned. One target is used for all builds.

- Initial semantic RED observed: 1 unit and 15 integration failures, red.log.
- Core implementation 7b3da25 passed 2 unit + 16 integration tests.
- Additional semantic RED observed for group history bypass, readiness on exit,
  launchd PATH, cross-config app locking, and slow-auth listener bounds.
- 5 unit + 24 integration tests pass; SQL trigger fault proves whole recovery
  transaction rollback, old outbound roots discover new paginated threads, and
  a sink can recover after application acceptance without bridge acknowledgement.
- Ruling: test listener windows use 2 seconds and reconciliation 60 seconds,
  rather than competing 1-second windows, to isolate one complete recovery pass.
  The blocked-sink test synchronizes the second event on sink entry. This
  corrects harness scheduling assumptions without reducing assertions.
- Ruling: app lock uses the canonical user Application Support directory,
  independent of XDG_CONFIG_HOME, to preserve per-user/app exclusion.
- Ruling: live raw history checks chat_mode via bot `im chats get`; the existing
  configured DM was read-only verified as p2p with current bot permissions.
  This prevents raw history from bypassing the event-side group refusal.
- Current checks: cargo fmt --check, clippy --all-targets -D warnings PASS.
- Contract unchanged from d732a76. Fresh external reviewers remain bus master's
  next step; no merge/Ready verdict is claimed by the author.
