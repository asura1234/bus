# bus task runner
set windows-shell := ["cmd.exe", "/d", "/s", "/c"]

python := if os() == "windows" { "python" } else if path_exists("temp/gate-tools/python/bin/python") == "true" { "temp/gate-tools/python/bin/python" } else { "python3" }

# Run tests
test: unit-test integration-test

# Collect fresh Rust and Python unit coverage, including every maintenance and skill test
unit-test:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py unit

# Rust integration targets and the in-process headless server harness; no real agents
integration-test:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py integration

# Fail below the checked-in Rust/Python floors; requires this round's unit/integration profiles
coverage:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py coverage

# Run repository maintenance contract tests
maintenance-test:
    {{python}} -m unittest scripts.test_bus_dev_acceptance scripts.test_package_windows_conpty scripts.test_sanitize_review_severity scripts.test_skill_migration_contract scripts.test_review_artifact scripts.test_review_artifact_write scripts.test_review_round_common scripts.test_review_prologue_entrypoints scripts.test_review_pr_round scripts.test_vendor_libghostty_vt scripts.test_vendor_portable_pty
    {{python}} skills/pr/scripts/test_pr_format_check.py
    {{python}} skills/review-pr/scripts/test_review_round.py
    {{python}} skills/split-pr/scripts/tests/test_split_plan.py

# Live message round trips with real Claude Code, Codex and Cursor; spends model usage (e.g. `just e2e --providers claude`)
e2e *args:
    {{python}} scripts/bus_e2e.py --allow-live-models {{args}}

# Run one nextest filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo nextest run --locked "{{filter}}" --status-level fail --final-status-level fail --failure-output final --success-output never

# Enforce deterministic UI hot-path architecture boundaries
ui-hot-path-architecture-test:
    {{python}} -m unittest scripts.test_ui_hot_path_architecture

# Run local Rust/Python lint checks
lint:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py lint

# Run PR CI checks
ci: lint unit-test integration-test coverage

# Run Windows target lint from Unix/macOS to catch cfg(windows) compile and clippy failures before CI
[unix]
windows-lint:
    rustup target add x86_64-pc-windows-msvc
    LIBGHOSTTY_VT_SIMD=false cargo clippy --bin bus --locked --target x86_64-pc-windows-msvc -- -D warnings

# Check formatting + run unit tests + Windows target lint + documentation contract tests
[unix]
check: ci windows-lint

[script("powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File")]
[windows]
check:
    & .\scripts\windows_check.ps1 -Mode check

# Install repo-local git hooks
install-hooks:
    git config core.hooksPath .githooks
    chmod +x .githooks/pre-commit
    chmod +x .githooks/commit-msg
    @echo "installed git hooks from .githooks"

# Build release binary
build:
    cargo build --release --locked

# Non-gating full-render scaling profile for background workspaces and active panes
bench-render-scale:
    cargo test --release --locked --bin bus render_scale_profile -- --ignored --nocapture --test-threads=1


# Build the vendored libghostty-vt source dist
build-libghostty-vt:
    scripts/build_vendored_libghostty_vt.sh
