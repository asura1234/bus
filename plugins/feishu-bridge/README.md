# feishu-bridge

A macOS command-line bridge for a locally authenticated Feishu application bot.
The external boundary is [INTERFACE.md](INTERFACE.md); consumers should use its
CLI and JSON protocol. The bridge has no knowledge of sink applications.

## Setup

Install `lark-cli` >= 1.0.97 (the send path passes `--text -` and streams the
body on stdin; older releases treat `-` as the literal message and are refused
with a version error) and authenticate its bot outside this project. The bridge
does not manage credentials. Build with `cargo build --locked --release`, then
place the executable at a stable absolute path.

Create a private TOML config following INTERFACE.md. Select exact app, sender
open_ids and p2p chat_ids. The sink must commit its own dedupe key and durable
work before exiting 0. Test with a private state directory and synthetic import:

```sh
feishu-bridge --config /absolute/test/config.toml listen --input /absolute/events.ndjson
feishu-bridge --config /absolute/test/config.toml status
```

For production, first run `listen` in the foreground and inspect diagnostics.
`install` only writes a launchd plist; an operator decides when to load it.
`uninstall` leaves state intact and does not unload an already loaded agent.
Do not run competing consumers on different machines for the same app.

## Development

Tests use synthetic event fixtures, private temporary directories, real SQLite
state and fake lark-cli/sink executables. They do not send network messages.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
```

This machine additionally requires the shared build lock, queue admission,
jobs 3, CARGO_INCREMENTAL=0 and the assigned target directory. See docs/plan.md.
No launchd agent is loaded by tests. Real Feishu sending/reply acceptance is a
separate operator step; passive connection proof does not establish it.
