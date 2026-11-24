# agile-board

A read-only localhost web Kanban board driven by a CLI.

- Usage guide, column semantics, transition matrix, Markdown description link format, view filters (`day`, `week`, `session`), and CLI examples: [GUIDE.md](GUIDE.md)
- CLI & HTTP v1 contract and exit codes: [INTERFACE.md](INTERFACE.md)

## Quick Start

```sh
cargo build --manifest-path plugins/agile-board/Cargo.toml
./plugins/agile-board/target/debug/agile-board \
  --data-dir /absolute/path/to/board-session \
  serve --port 3847 --mock
```
