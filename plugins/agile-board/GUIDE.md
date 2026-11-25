# Guide: `agile-board`

`agile-board` is a standalone, workflow-agnostic, read-only localhost web Kanban board driven entirely through its CLI. A person views the board in a browser on `127.0.0.1`; an automation caller or consumer CLI updates sections and tickets from the command line.

---

## 1. Design Lineage (Reference Projects)

The board layout and ticket model borrow ideas from six open-source Kanban projects without adopting their stacks:

- **Kanboard & Wekan (Horizontal Swimlanes):** Each registered section occupies one row. The top bar and column headers stay visible during vertical scrolling, and section names stay visible during horizontal scrolling. On wide screens the columns through **Done** fill the available width. Scroll right to see **Won't Work**.
- **Planka & Vikunja (Activity Log & View Windows):** Every state change is recorded in the board's chronological `session_history`. The top bar includes the `View` (`day`, `week`, `session`) time-window selector, the `Hide Done` toggle, and the `Clear session history` button with two-step confirmation.
- **Focalboard & Taiga (Self-Contained Card Properties):** Each ticket card is a near-square rounded rectangle (slightly wider than tall via `container-type: inline-size` on `.kanban-column` and `min-height: 86cqi; height: auto` on `.ticket-card` so longer descriptions grow the card vertically without truncation and the MR badge always stays inside the border, plus a plain 1px solid border in one consistent color without per-column accents, gradients, or glow) displaying the plain-text title, a one-sentence sanitized Markdown description with clickable external links, and a distinct MR/PR link (internal ticket IDs like `T-1` are kept for CLI commands and hidden on the web page).

---

## 2. Board Model

### Sections (`room`)

A section (addressed as `room` in the CLI) is an opaque grouping label with a numeric `id` and a `name` (for example `backend-api`, `web-client`, `storage-engine`). Each section has its own horizontal band of workflow columns.

### Tickets (`ticket`)

A ticket tracks a proposed work item through delivery, investigation, or a decision to close it as **Won't Work**.

| Field | Type | Description |
|---|---|---|
| `id` | `String` (`T-1`, `T-2`, …) | Internal sequential ticket identifier for CLI `move`/`update` commands (hidden on the rendered web page) |
| `room_id` / `room_name` | `u64` / `String` | The section that owns the ticket |
| `title` | `String` | Plain-text ticket name |
| `description` | `String` | Short one-sentence Markdown description; rendered into sanitized HTML (`description_html`) with clickable external links |
| `column` | `Column` | Current stage (`todo`, `blocked`, `investigation`, `implementation`, `testing`, `review`, `done`, `wont_work`) |
| `investigation_only` | `bool` | Set via `--investigation-only` at creation; allows jumping `investigation -> done` directly |
| `mr_url` | `Option<String>` | Separate MR/PR URL rendered distinctly from the description |
| `author_model` | `Option<String>` | Internal author model identifier (`--author-model`); not displayed on the board |
| `reviewer_model` | `Option<String>` | Internal reviewer model identifier (`--reviewer-model`); not displayed on the board |
| `notes` | `Vec<TicketNote>` | Timestamped transition/update notes |
| `created_at_ms` / `updated_at_ms` | `i64` | Epoch milliseconds of creation and latest update |
| `history` | `Vec<TicketEvent>` | Chronological audit log for the ticket |

### Description Markdown and Link Format

- Pass `--description` on `ticket add` or `ticket update` as a short one-sentence Markdown string.
- Both Markdown links (`[Feishu buglist #4821](https://example.feishu.cn/wiki/wikcnBugList4821)`, `[Product Spec](https://example.com/docs/spec)`) and bare `https://...` or `http://...` URLs are rendered into safe `<a href="..." target="_blank" rel="noopener noreferrer">` links that open in a new tab.
- Raw HTML inside `description` is escaped by the Rust `pulldown-cmark` sanitizer and never injected into the DOM. Non-HTTP(S) link schemes (such as `javascript:` or `data:`) are stripped to plain text.
- The MR/PR link is stored separately via `--mr <URL>` and rendered as its own badge at the bottom of the card.
- The badge shows `PR #N` for `github.com` pull links, and `MR #N` for Codeup `/change/N` and GitLab `/merge_requests/N` links. It shows `MR / PR` when the URL has no recognized change number.

---

## 3. Column Semantics and Allowed Transitions

Inside each section, eight columns run horizontally.

| Column | Meaning | Allowed Outgoing Transitions |
|---|---|---|
| `todo` | Work held or deferred | `investigation`, `blocked`, `wont_work` |
| `blocked` | Needs a developer decision or missing information or context | Any other column |
| `investigation` | Root-cause analysis or design exploration | `implementation`, `blocked`, `wont_work`, plus `done` **only if** `--investigation-only` |
| `implementation` | Active implementation | `testing`, `blocked`, `wont_work` |
| `testing` | Work is implemented and undergoing verification, including time spent waiting for a build, test, gate, or merge window | `review`, `implementation`, `blocked`, `wont_work` |
| `review` | Independent review in progress | `done`, `implementation`, `blocked`, `wont_work` |
| `done` | MR/PR has merged, or an investigation-only ticket has concluded | `blocked`, `wont_work` |
| `wont_work` | Work declined, or a report closed as unverified, unreproducible, or expected behavior | `todo`, `blocked` |

### Transition Rules

- **Standard flow:** `todo -> investigation -> implementation -> testing -> review -> done` (with rework loops `testing -> implementation` and `review -> implementation`).
- **Investigation-only rule:** Tickets created with `--investigation-only` may transition directly `investigation -> done`. Normal tickets attempting `investigation -> done` are refused with exit code `3` and a message listing the allowed targets (`[implementation, blocked, wont_work]`).
- **Universal `blocked` escape hatch:** Any ticket may move to `blocked` from any column, and from `blocked` to any column. Use it only for a decision or missing context. Record what is needed in `--description` or `--note`.
- **Closing and reopening:** Use `ticket move ID --column wont_work --note "Could not reproduce"` to close a ticket and record the reason. The CLI also accepts `wont-work`. Move the ticket to `todo` to reopen it, or to `blocked` when a decision or context is still needed.

---

## 4. Views (`day`, `week`, `session`)

Both the web UI `View` dropdown and `agile-board ticket list --view <VIEW>` use the same local-time filter on `touched_ms = max(created_at_ms, updated_at_ms)`:

- `day`: Tickets created or updated on or after **local midnight today** (`00:00:00` local time).
- `week`: Tickets created or updated within the **last 7 local calendar days** (`local_midnight_today - 6 * 86_400_000 ms`).
- `session`: **All tickets** persisted in the current board session.

---

## 5. Persistence and Clearing Session History

- **Data directory:** Every command requires `--data-dir <ABSOLUTE_PATH>`. Relative paths are rejected with exit code `2`.
- **Storage file:** State is persisted atomically under `<data-dir>/agile-board/board.json` (protected by advisory lock `<data-dir>/agile-board/board.lock`).
- **Clearing session history:** Running `agile-board history clear --confirm` (or clicking **Clear session history** in the web UI and completing both steps of the double confirmation popup) removes all persisted `tickets` and `session_history` events in `<data-dir>/agile-board/board.json` and resets ticket numbering to `T-1`. Canceling at either confirmation step leaves data untouched. Registered sections (`rooms`) are retained.

---

## 6. Worked CLI Examples

```sh
DATA_DIR=/tmp/agile-board-demo

# 1. Register sections
agile-board --data-dir "$DATA_DIR" room add --id 1 --name backend-api
agile-board --data-dir "$DATA_DIR" room add --id 2 --name web-client

# 2. Create a ticket with Markdown links in --description and a separate --mr link
agile-board --data-dir "$DATA_DIR" ticket add \
  --room backend-api \
  --title "Guard concurrent token refresh and retain failed snapshots" \
  --description "Fixes race in [Feishu buglist #4821](https://example.feishu.cn/wiki/wikcnBugList4821) per [Auth Spec v2](https://example.com/docs/auth-spec-v2)." \
  --column investigation \
  --mr "https://github.com/example/platform/pull/500"

# 3. Advance the ticket across columns
agile-board --data-dir "$DATA_DIR" ticket move T-1 --column implementation --note "Added snapshot guard"
agile-board --data-dir "$DATA_DIR" ticket move T-1 --column testing --note "Regression suite green"
agile-board --data-dir "$DATA_DIR" ticket move T-1 --column review --note "Independent review started"
agile-board --data-dir "$DATA_DIR" ticket move T-1 --column done --note "Merged #500"

# 4. Create an investigation-only ticket and move it directly to done
agile-board --data-dir "$DATA_DIR" ticket add \
  --room backend-api \
  --title "Audit stale socket cleanup after forced SIGKILL" \
  --description "Root-cause check for [Feishu buglist #4902](https://example.feishu.cn/wiki/wikcnBugList4902); no code change required." \
  --column investigation \
  --investigation-only
agile-board --data-dir "$DATA_DIR" ticket move T-2 --column done --note "Already handled by listener lock"

# 5. Record a blocked ticket
agile-board --data-dir "$DATA_DIR" ticket add \
  --room web-client \
  --title "Add upstream gateway health probe" \
  --description "Blocked on [Gateway PRD](https://example.com/docs/gateway-prd): refuse startup when primary upstream is missing or fall back?" \
  --column blocked

# 6. List tickets and inspect or clear session history
agile-board --data-dir "$DATA_DIR" ticket list --view day
agile-board --data-dir "$DATA_DIR" ticket list --room backend-api --column done --json
agile-board --data-dir "$DATA_DIR" history --json
agile-board --data-dir "$DATA_DIR" history clear --confirm

# 7. Serve the read-only web UI on 127.0.0.1:3847
agile-board --data-dir "$DATA_DIR" serve --port 3847 --mock
```
