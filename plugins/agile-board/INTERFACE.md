# `agile-board` Interface, Version 1

`agile-board` is a standalone Rust executable under `plugins/agile-board` that persists a board session under `<data-dir>/agile-board/board.json` and serves a read-only localhost web UI on `127.0.0.1`. All JSON output is UTF-8 and newline-terminated. Diagnostics and errors are written to `stderr`; `stdout` contains only command results.

## CLI Surface

```text
agile-board --data-dir ABSOLUTE_PATH serve [--port PORT] [--mock] [--run-for SECONDS]
agile-board --data-dir ABSOLUTE_PATH room add --id N --name NAME [--json]
agile-board --data-dir ABSOLUTE_PATH room list [--json]
agile-board --data-dir ABSOLUTE_PATH ticket add --room NAME_OR_ID --title TITLE [--description MARKDOWN] [--column COLUMN] [--mr URL] [--author-model MODEL] [--reviewer-model MODEL] [--investigation-only] [--note TEXT] [--json]
agile-board --data-dir ABSOLUTE_PATH ticket move ID --column COLUMN [--note TEXT] [--json]
agile-board --data-dir ABSOLUTE_PATH ticket update ID [--title TITLE] [--description MARKDOWN] [--mr URL] [--author-model MODEL] [--reviewer-model MODEL] [--note TEXT] [--json]
agile-board --data-dir ABSOLUTE_PATH ticket list [--room NAME_OR_ID] [--column COLUMN] [--view day|week|session] [--json]
agile-board --data-dir ABSOLUTE_PATH history [--json]
agile-board --data-dir ABSOLUTE_PATH history clear --confirm [--json]
agile-board --help
agile-board --version
```

### Data Directory Resolution

- `--data-dir` is global (accepted before or after the subcommand) and must be an absolute path.
- Relative paths and missing `--data-dir` values are rejected with exit code `2`.
- State lives in `<data-dir>/agile-board/board.json` alongside advisory lock `<data-dir>/agile-board/board.lock`.

### Ticket Description Sanitization

- `description` is stored as raw Markdown text and rendered server-side via `pulldown-cmark` into `description_html`.
- Raw HTML tags in `description` are escaped and never passed through.
- `http://` and `https://` links (both `[label](url)` and bare URLs) render with `target="_blank" rel="noopener noreferrer"`.
- `mr_url` (`--mr`) is stored separately and rendered as a badge using the [MR/PR link format](GUIDE.md#description-markdown-and-link-format).
- `author_model` and `reviewer_model` are optional model identifiers in `ticket list --json`. The page does not display them. Omitting a model flag during `ticket update` preserves its value; passing an empty value clears it.

### Columns and Transition Matrix

Valid `COLUMN` values are `todo`, `blocked`, `investigation`, `implementation`, `testing`, `review`, `done`, and `wont_work`. The CLI accepts `wont-work` as an alias for `wont_work`. JSON output and persisted state use `wont_work`.

| From \ To | `todo` | `blocked` | `investigation` | `implementation` | `testing` | `review` | `done` | `wont_work` |
|---|---|---|---|---|---|---|---|---|
| `todo` | — | yes | yes | no | no | no | no | yes |
| `blocked` | yes | — | yes | yes | yes | yes | yes | yes |
| `investigation` | no | yes | — | yes | no | no | only if `investigation_only` | yes |
| `implementation` | no | yes | no | — | yes | no | no | yes |
| `testing` | no | yes | no | yes | — | yes | no | yes |
| `review` | no | yes | no | yes | no | — | yes | yes |
| `done` | no | yes | no | no | no | no | — | yes |
| `wont_work` | yes | yes | no | no | no | no | no | — |

Creating a ticket directly in `done` without `--investigation-only` is also refused with exit code `3`.

## HTTP Boundary (`127.0.0.1` only)

`agile-board --data-dir <ABSOLUTE_PATH> serve [--port PORT]` binds strictly to `127.0.0.1:<PORT>` (default `8787`; `--port 0` selects an ephemeral port) and prints `http://127.0.0.1:<BOUND_PORT>` as the first line on `stdout`.

| Method & Path | Description |
|---|---|
| `GET /` or `GET /index.html` | Returns the read-only Kanban board (`text/html; charset=utf-8`). Scroll right past **Done** to see **Won't Work**. The top bar provides `View`, `Hide Done`, and `Clear session history` controls. |
| `GET /healthz` | Returns `{"ok":true}` (`200 OK`). |
| `GET /api/board?view=day\|week\|session` | Returns the board JSON (`ok`, `version`, `view`, `now_ms`, `local_midnight_ms`, `columns`, `rooms`, filtered `tickets`, `all_tickets`, `total_session_tickets`, `session_history`). |
| `GET /api/history` | Returns `{"ok":true,"count":N,"history":[...]}`. |
| `POST /api/history/clear` | Requires body `{"confirm":true}`. Clears all tickets and session history while preserving registered sections; returns `{"ok":true,"cleared_tickets":N,"cleared_events":M}`. Returns `400 Bad Request` if `confirm` is not `true`. |

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | Command succeeded |
| `1` | Runtime or network bind failure |
| `2` | Invalid CLI arguments, missing `--confirm`, unknown section/ticket, or missing/non-absolute `--data-dir` |
| `3` | Disallowed column transition |
| `4` | Local file I/O, lock, or JSON persistence failure |
