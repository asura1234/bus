# PR #22 — clickable chat-history links QA (draft)

Contract: [PR #22](https://github.com/asura1234/bus/pull/22), head `db60322a2cac8efcbb9f893288a420bc3ac9f97b`; product code in `~/work/bus-fix-clickable-links/src/client/` rechecked 2026-10-12.
Current TUI help/forms provide setup actions. Unexecuted draft; run against the PR build.

## Setup and evidence rules

- Run cases in listed order, independently with fresh scratch data/settings and fake
  providers. Never touch running Bus or `~/.local/share/bus`; no model output.
- Use the UI room/worker/recipient/file recipes in [app.md](app.md), independently
  for each case. Fixed agent-authored fixtures need fake workers alpha and beta:
  `bus send --room ROOM --as alpha --to beta --text TEXT` in the scratch session.
  Developer messages are typed/sent through composer; no generated replies are required.
- Prepare existing absolute files `F`/`G`, author-agent PWD `W` with `relative.txt`,
  an existing scratch-owned `~/P` fixture beneath Bus's effective home `H`, and absent `W/missing.txt`.
  Record H and actual targets; keep tokens on one displayed row at 120×40; use only scratch-owned files.
- Each step is action → expected result. Capture text/RGB/underline and screenshots
  judged by the runner. Text-link blue is RGB(90,150,255), underlined. Allow 5 seconds
  after a path first becomes visible for existence-check styling; an initially unlinked
  path is allowed. This is a QA wait budget, not a product deadline; record elapsed time.
- Plain click: left press/release on the same visible text/filename/thumbnail cell, no drag/modifier. Require exactly one
  **recorded effect: open with target X**, no extra opens; compare recorded effects
  before/after each gesture. Browser windows/opener exit codes are insufficient.
- Capture scratch history/state for author, working directory and attachment identity. Every step: **SKIP with reason if the runner cannot capture this evidence** or supply
  its fixture; continue independent checks. Captured mismatches are FAIL; skipped
  assertions stay SKIP, never PASS. Desktop banners are out of scope.

## Cases (run order)

### LINK-01 — HTTP and HTTPS styling and plain-click opening

**Preconditions/setup:** own room/worker; send `link http://example.test/a end` and `link https://example.test/b?q=1 end` as separate short developer messages.

1. Inspect history → both complete URL tokens blue + underlined; no 📎 added.
2. Plain-click each URL → recorded effect: open with target that exact URL once, including scheme/query.
3. Click neighbouring ordinary text → no recorded open effect; composer remains usable.

**Evidence:** step 1: cells/screenshot judged by the runner; steps 2–3: recorded open effects/absence, history text. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** both schemes look clickable and open only their exact target.

### LINK-02 — Existing absolute paths, punctuation and line references

**Preconditions/setup:** own room/worker; send short rows containing `F`, `"F"`, `(F),`, `F:12` and `F:12:3`, substituting F's actual path.

1. Inspect settled history → path tokens blue + underlined, including :12/:12:3; surrounding quotes/brackets and trailing sentence punctuation stay outside target.
2. Plain-click each path/reference → recorded effect: open with target absolute F, excluding quotes/brackets, comma and line/column suffix.

**Evidence:** step 1: cells/screenshot judged by the runner, fixture existence; step 2: five recorded open effects with target F. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** decoration/suffixes never change the file opened.

### LINK-03 — Relative and home paths use the correct base

**Preconditions/setup:** own room with alpha (PWD W) and beta (different PWD); relative.txt exists in W and scratch Bus's launch directory. Post `./relative.txt` and `~/P` as alpha to beta; P identifies the scratch-owned home fixture above.

1. View that history with beta selected as recipient → both existing paths become blue + underlined; selection does not change the message's author.
2. Plain-click each → recorded effect: open with target `W/./relative.txt` and absolute `H/P` respectively; never beta's directory. Preserve lexical `./` in the recorded target; source does not canonicalize it.
3. Type `./relative.txt` in composer, send as developer, then click that history token → no blue/underline path-link style and no recorded open effect, even though the file exists in the launch directory. Developer messages have no relative-path base.

**Evidence:** steps 1–3: author/PWD state and cells; steps 2–3: recorded open effects/absence with named targets. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** agent paths use author PWD, ~/ uses scratch Bus home, developer-relative tokens remain unlinked.

### LINK-04 — Individual paths inside files: [...] lists

**Preconditions/setup:** own room/worker; send `files: ["F", "G"]` as message text, substituting short existing absolute paths; this is not the attachment picker.

1. Inspect history → F and G separately blue + underlined; list remains readable, no 📎 or attachment control added below composer.
2. Plain-click each path → recorded effect: open with target F then G; neither opens the entire list, a quoted string or the other file.

**Evidence:** step 1: cells/screenshot judged by the runner including composer bottom; step 2: recorded open effects with targets F/G. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** each list member has its own correct target.

### LINK-05 — Link style wins inside inline and fenced code

**Preconditions/setup:** own room with alpha and beta; post Markdown as alpha to beta with inline code `plain F` and a fenced block containing a URL and F, substituting the real path; tokens fit on rows.

1. Inspect settled history → URL/path cells blue + underlined in both contexts; non-path inline code `plain` keeps its cyan code style without link underline.
2. Plain-click inline path, fenced path and fenced URL → recorded effect: open with target F, F and exact URL respectively; no backticks/fence markers included.

**Evidence:** step 1: cells/screenshot judged by the runner; step 2: recorded open effects with F/F/URL targets; raw history. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** code formatting preserves correct links and non-link code style.

### LINK-06 — Sent file attachments: style, position, hover and opening

**Preconditions/setup:** own room/worker selected; attach F and G via Ctrl+O, type `attached files`, send. Fake worker need not reply.

1. Inspect history → attachment names directly under their message, blue + underlined, with no square brackets or 📎; blank row separates attachments from any reply/placeholder. No new link UI appears beneath composer.
2. Hover F's attachment name → full F path visible; move away, plain-click name → recorded effect: open with target actual F; repeat G → actual G.

**Evidence:** step 1: cells/screenshot judged by the runner; step 2: hover text and recorded open effects with F/G targets; state files. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** attachments are placed with their message and open their real files.

### LINK-07 — Image thumbnail and filename fallback open the saved image

**Preconditions/setup:** independent subruns with their own room/selected worker: graphics-enabled host and host without image support. Paste a known PNG with Ctrl+V and send; state identifies saved copy I inside scratch Bus attachments. Retain reference PNG; no generated answer needed.

1. Graphics subrun: inspect history → expected thumbnail under its message, no caption, blue border, underline or 📎; plain-click inside picture → recorded effect: open with target I once. Evidence: screenshot judged by the runner and recorded open effect; SKIP with reason if either cannot be captured.
2. Text-only subrun: inspect history → image filename blue + underlined, no brackets or 📎; hover reveals I; plain-click filename → recorded effect: open with target I once. Evidence: filename cells/hover text and recorded open effect; SKIP with reason if the runner cannot capture this evidence or supply clipboard pixels.

**Evidence:** step-specific evidence above and saved-copy pixel identity. **Pass criteria:** both presentations open the saved image; blue/underline applies to filename fallback, never the thumbnail pixels. Unavailable subruns remain SKIP with reason.

### LINK-08 — Dragging a link selects and copies without opening

**Preconditions/setup:** own room/worker; send short `before F after` and `https://example.test/copy` as developer messages.

1. Drag across F with distinct press/release cells → text selection visible, no recorded open effect; release copies selected text as plain text.
2. Drag across entire URL → clipboard contains exact URL without markup/📎; no recorded open effect. Plain-click URL afterward → recorded effect: open with target that URL once.

**Evidence:** steps 1–2: selection cells/screenshot judged by the runner, exact copied text, recorded open effects/absence per gesture. SKIP with reason for assertions whose clipboard/effect evidence cannot be captured. **Pass criteria:** dragging copies without opening; plain click still opens.

### LINK-09 — Negative targets and links after navigation

**Preconditions/setup:** own room/worker; send developer messages containing missing.txt's absolute path, `[hidden](https://example.test/hidden)` and an existing short F, plus enough messages to overflow history.

1. After the path-probe wait, click missing path and hidden label → no recorded open effect; missing path has no confirmed path-link style. Do not reject Markdown's own label styling.
2. Scroll F out of view/back, resize 120×40 to 100×30 keeping F on one row → F remains blue + underlined; click its new cell position → recorded effect: open with target F only.
3. Type `/help`, Enter, scroll guide → `Click a link` guidance present; Esc returns.

**Evidence:** steps 1–2: cells/screenshots judged by the runner and recorded open effects/absence; step 3: guide text. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** unsupported targets do not open; visible links keep correct click locations.

## Decisions

- **Thumbnail style:** #22 renders the picture without filename/caption or link decoration; its rows open the saved image. Only filename fallback gets blue RGB(90,150,255) + underline (`history/exchange.rs`, `links.rs`); no 📎 in either presentation.
- **Relative paths:** agent-authored paths join author PWD without canonicalizing; relative paths authored by a developer or Bus remain unlinked. Absolute and ~/ paths work for either author; ~/ uses the scratch Bus process's effective home (`history/exchange.rs::link_scope`, `links.rs::candidate`).
- **Timing:** a background existence check completes before path styling/opening; results appear on the UI tick (100 ms idle, 10 ms pending commands). There is no product deadline; use a 5-second QA wait after first visibility, then report a captured styling timeout with elapsed time (`links.rs::PathProbe`, `ui.rs`). URLs need no existence check.
- **Evidence:** "opened" means a recorded opener request with the unchanged URL or absolute path X, not a `file://` conversion or proof that an external app displayed it (`effects.rs`). Screenshots are judged by the runner; unavailable clipboard/pixels/effects → affected step SKIP with reason. Desktop banners are out of scope; terminal-visible notices/recorded effects remain in scope.
- **Inherited TUI decisions:** use app.md's actual shortcuts and save/stop/quit expectations; conflicting CLI help is a product finding. Source has no terminal minimum; link cases keep tokens on one row at their stated sizes. Sidebar has no unread badge; none is required.
- **Boundaries / wording:** hidden-label Markdown links and paths split across rows are outside clickable-link acceptance; refer to the person using Bus as developer, preserving visible `You` labels.
