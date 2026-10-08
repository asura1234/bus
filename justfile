# bus task runner
set windows-shell := ["cmd.exe", "/d", "/s", "/c"]

python := if os() == "windows" { "python" } else if path_exists("temp/gate-tools/python/bin/python") == "true" { "temp/gate-tools/python/bin/python" } else { "python3" }

# Run tests
test: unit-test integration-test

# Collect fresh Rust unit coverage and run every Python maintenance and skill test
unit-test:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py unit

# Rust integration targets and the in-process headless server harness; no real agents
integration-test:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py integration

# Fail below the checked-in Rust floor; requires this round's unit/integration profiles
coverage:
    {{python}} skills/gate-and-fix/scripts/bus_quality.py coverage

# Run repository maintenance contract tests
maintenance-test:
    {{python}} -m unittest tools.tests.acceptance_existing_instance_test tools.tests.sanitize_review_severity_test scripts.skill_migration_contract_test tools.tests.review_artifact_test tools.tests.review_artifact_write_test tools.tests.review_round_common_test tools.tests.review_prologue_entrypoints_test tools.tests.review_pr_round_test tools.tests.vendor_libghostty_vt_test tools.tests.vendor_portable_pty_test
    {{python}} -m pytest packaging/windows/tests/package_conpty_test.py
    {{python}} skills/pr/scripts/tests/pr_format_check_test.py
    {{python}} skills/review-pr/scripts/tests/review_round_test.py
    {{python}} skills/split-pr/scripts/tests/split_plan_test.py

# Live message round trips with real Claude Code, Codex and Cursor; spends model usage (e.g. `just e2e --providers claude`)
e2e *args:
    {{python}} tools/acceptance/e2e_test.py --allow-live-models {{args}}

# Run one nextest filter, e.g. `just test-one codex_stale_working`
test-one filter:
    cargo nextest run --locked "{{filter}}" --status-level fail --final-status-level fail --failure-output final --success-output never

# Enforce deterministic UI hot-path architecture boundaries
ui-hot-path-architecture-test:
    {{python}} -m pytest -q tools/tests/ui_hot_path_test.py tools/tests/import_boundaries_test.py

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
    & .\tools\windows\check.ps1 -Mode check

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
    tools/vendor/build_libghostty_vt.sh
