# Bus TUI whole-app QA — draft

Basis: main worktree `6bdfe7c83c51c2611f7c717a3bc663c114ca29c8`, source rechecked 2026-10-12.
Read: [README](../../README.md), user-visible strings/actions in
[TUI help](../../src/client/rooms/help.rs), room forms, sidebar, history and settings
under `src/client/`, and `target/debug/bus --help`. No driver implementation or
driver documentation was used. Results are tracked separately for each run.

## Run contract and setup recipes

- Run cases in the order below, each in a fresh scratch session with its own data
  and isolated settings/config. Never connect to the running Bus or read/write
  `~/.local/share/bus`. Record build/commit, scratch root, platform, terminal,
  terminal size and graphics capability. Fake Claude Code, Codex and Cursor
  launches must be available; no real account, model response or thinking required.
- Start at 120 columns × 40 rows unless a case specifies otherwise. Wait for UI
  transitions to settle; record elapsed time on a timeout. Locate controls by
  their current labels, not fixed coordinates; avoid accidental double-clicks.
- Every numbered step is **action → expected result**. Capture text/cells/state,
  a **screenshot judged by the runner**, or **recorded effect: notification/sound/open
  with target X**, as specified. For every step: **SKIP with reason if the runner
  cannot capture this evidence** or supply its fixture; continue independent checks.
  A captured mismatch is FAIL. Skipped assertions are neither failures nor passes;
  a whole case is PASS only when all its assertions were captured and matched.
- UI actions create/change test data. Read-only scratch Bus state/history may
  corroborate room/agent IDs, names, membership, accepted messages, attachments and
  settings; state alone cannot prove visible layout, colours or images. Judge live
  draft/notes edits from visible text: saved-state files may lag in-progress edits.
  Compare persisted draft/notes only after a completed save boundary (room switch
  or quit); stale saved text mid-edit is not a failed visible-draft assertion.
- Fixed incoming-message fixtures may be arranged through the supported scratch
  developer CLI described by `bus --help`, with a named agent author. They are
  test inputs, not model output; do not rewrite live storage to manufacture a pass.

**R(name), create a work room:** press Ctrl+Shift+R → `Room name` form; type the
name and press Enter → room appears and opens with `# name`. Verify its ID/name
in scratch state. For later cases, R is setup and does not depend on APP-02.

**A(room, name, provider), create a worker:** click the room; press Ctrl+N → fields
`Name`, `Agent`, `PWD`, `Additional launch args (optional)`; type the name; press
Tab → provider choices; use Up/Down to highlight the named provider and Enter →
provider selected, PWD focused. Press Ctrl+A then Ctrl+K, type the absolute scratch
workspace directory without a trailing slash; wait for its matching directory
suggestion and select it with Up/Down if needed. First Tab completes PWD (may append
`/`) and keeps PWD focused; second Tab focuses optional args. Confirm the focused
field, leave args empty, click `Add (Enter)`
→ agent appears and its fake terminal opens. If a hook-write consent form appears,
capture it and click Add only for the isolated scratch project → creation continues.
Wait for the fake terminal's identifiable ready screen, press F6 → original room.
Verify agent name/provider/PWD/membership in scratch state. Consent is not approval
of provider permissions; a real-provider prompt means setup is unsuitable.

**P(names), select recipients:** press Ctrl+P → picker; inspect current checkmarks;
click agent rows to make exactly the named set checked; press Esc → picker closes,
recipient chips name exactly that set. Do not assume newly added agents are selected.

**Fixtures:** use short absolute paths under the scratch workspace: `notes.txt`,
`report with space.txt`, two readable PNGs `red-diagonal.png` and `blue-circle.png`
with unmistakably different shapes/colours, and an absent `missing.txt`. Keep the
images small. Save reference images for visual comparison. Clipboard-image cases
need the actual PNG on the clipboard, not its path text. Graphics cases require a
terminal that really displays images and reports cell dimensions; retain screenshots.
Desktop banners are out of scope. Terminal-visible notices and recorded effects
are in scope; no unread badge is expected in today's room sidebar.

## Cases (run order)

Run APP-01..11, LINK-01..10, then APP-12..22 in that order. Each case is independent.

### APP-01 — First start and empty conversation

**Preconditions/setup:** pristine scratch data/settings; no pre-created workers.

1. Launch scratch Bus and wait for startup → permanent `MASTER` and the default work room `bus` appear; `bus` opens. Its history and AGENTS list have no messages or workers; composer says `Message selected agents…`, with `@  Choose agents`.
2. Inspect room notes → seeded text includes `Goal: stay in the room to coordinate selected agents.` and `Non-goals: automatic handoffs or permission approval.`
3. Press Enter on the empty composer → no message, error or agent is created.
4. Type `/help`, press Enter, scroll with PageDown, then Esc → local `Keyboard shortcuts` guide opens and closes; `/help` is not sent or retained as a message.

**Evidence:** startup/help text, screenshot, scratch rooms/agents/history before/after.

**Pass criteria:** seeded empty session is usable and local help/empty Enter create no chat.

### APP-02 — Create a room and cancel a room form

**Preconditions/setup:** fresh session; note initial room IDs.

1. Press Ctrl+Shift+R, type `discard-room`, press Esc → form closes; no such room.
2. Press Ctrl+Shift+R, type `qa-room`, press Enter → one new work room opens; sidebar and main header show its name, with empty history and no workers.
3. Click `+` beside ROOMS, type `mouse-room`, click `Add (Enter)` → second new room appears and opens; `qa-room` remains selectable.

**Evidence:** form/headers text and state IDs/counts at each transition.

**Pass criteria:** both creation routes work; cancellation does not create a room.

### APP-03 — Agent form validation and all three providers

**Preconditions/setup:** R(`qa-agents`); scratch provider launchers ready.

1. Press Ctrl+N, click Add without completing fields → validation names missing Name/Model; form remains editable and no agent is created. Press Esc → closes.
2. Use A to create `qa-codex` (Codex), `qa-claude` (Claude Code), `qa-cursor` (Cursor), returning with F6 after each → every fake terminal is identifiable; sidebar lists the matching name/provider, state records the supplied PWD.
3. Open Ctrl+N, type `cancel-agent`, press Esc → no extra agent or terminal.

**Evidence:** validation text, provider-choice text, ready terminal captures, state.

**Pass criteria:** required fields enforced; three provider choices create only their agents.

### APP-04 — MASTER agent form and room navigation

**Preconditions/setup:** R(`qa-master-target`), no orchestrator; fake Claude ready.

1. Click `# MASTER`, press Ctrl+N → form includes `Orchestrates room` and a populated system prompt. Tab to Agent field → menu offers Claude Code and Cursor.
2. Fill name `qa-master`, select Claude Code, set scratch PWD using A's completion recipe: first Tab completes the matching directory, second Tab focuses optional args. Leave args empty; one more Tab focuses `Orchestrates room`; use Left/Right until `qa-master-target` is shown, then click Add → fake terminal opens.
3. Press F6 → MASTER chat; click `# qa-master-target` beneath that agent → target room opens. Click MASTER, then agent name → its terminal opens, not target chat.
4. Press F6, press F2 on MASTER → a fixed-room error; name stays MASTER and no delete × is offered for it.

**Evidence:** form/options, navigation screenshots/text, state relationship and IDs.

**Pass criteria:** orchestrator belongs to MASTER, targets the chosen room, and routes correctly.

### APP-05 — Select rooms, terminals and agent details

**Preconditions/setup:** R(`qa-left`), A with `left-agent`; R(`qa-right`), A with `right-agent`; distinct identifiable fake terminal screens.

1. Click `# qa-left` → header/history/composer belong to qa-left; AGENTS lists left-agent. Click left-agent → its fake terminal replaces the main chat area.
2. Press F6 → qa-left chat returns. Click qa-right and right-agent → qa-right's worker terminal appears; press F6 → qa-right chat.
3. Click `>` beside right-agent's provider → PWD details expand; click `v` → collapse. Agent/room selection remains usable.

**Evidence:** labels and selection cell backgrounds, terminal screenshots, state IDs/PWD.

**Pass criteria:** each click selects its named target; details reveal the correct directory.

### APP-06 — Recipient picker and room-local selection

**Preconditions/setup:** R(`qa-pick`), A with `alpha` and `beta`; R(`qa-other`).

1. Click qa-pick; press Ctrl+P → picker lists All, alpha and beta with checkboxes.
2. Clear any selections, move with Up/Down to alpha, press Space, then Esc → only alpha's recipient chip appears; room draft recipients contain only alpha.
3. Click `@`, click beta's row, press Tab → both chips appear. Reopen picker, highlight All, press Enter → all are unchecked; Enter again → both checked.
4. Close picker; switch to qa-other then back → qa-other does not inherit these recipients; qa-pick restores its own selection.

**Evidence:** checkbox/chip text and colours, state recipient IDs per room.

**Pass criteria:** mouse and keyboard toggle the intended recipients without sending a message.

### APP-07 — Compose, edit and send a message

**Preconditions/setup:** R(`qa-send`), A with alpha/beta, P(alpha); empty draft.

1. Type `line one`, press Ctrl+J, type `line two @ +` → two draft lines; literal @ and + are text. If the host supports Shift+Enter, press it at the end of line two → third empty line; press Backspace once → exactly the original two lines restored, caret at the end of `line two @ +`, ready for step 2.
2. Press Ctrl+A then Ctrl+K on the last line; type `line two revised` → only that line is replaced; first line remains. Press Enter once → one history message shows both lines, `You → alpha`; composer clears after acceptance.
3. Inspect scratch history → exact text `line one\nline two revised`, alpha as sole recipient. Press Enter again → no duplicate message or empty-send error.
4. Type `throwaway`, press Ctrl+C → draft clears; Bus remains open.

**Evidence:** before/after draft/history text, author/recipient cells, state message count/text.

**Pass criteria:** one Enter sends exactly the edited draft to the selected worker; no reply required.

### APP-08 — Attach local files, remove one, and send

**Preconditions/setup:** R(`qa-files`), A with alpha, P(alpha); fixture files exist.

1. Press Ctrl+O → `Local file path` form. Replace its path with absolute notes.txt; click Add → room returns with a notes.txt attachment chip.
2. Click `+` by the composer, attach `report with space.txt` the same way → both chips present; hover that named chip → full report path is visible. Use file-row arrows/wheel if needed. Locate the chip by its full filename and click the × immediately beside `report with space.txt` inside that chip → only notes.txt remains.
3. Leave text empty, press Enter → attachment-only message appears in history; draft chips clear; state message files contains exactly notes.txt's absolute path.
4. Attach both files again, type `two files`, press Enter → second message has exact text and both distinct paths; the earlier message is retained.

**Evidence:** file form/chips/full-path text, state draft/files and history after each send.

**Pass criteria:** paths with spaces, removal and both send modes preserve correct attachments.

### APP-09 — Paste and send a clipboard image

**Preconditions/setup:** R(`qa-paste`), A with alpha, P(alpha); actual red-diagonal PNG on clipboard; scratch-owned image storage is writable. Steps 1–3: SKIP with reason if the runner cannot supply/capture clipboard pixels or the saved image evidence.

1. Press Ctrl+V in composer → image attachment chip appears; hover it → full path refers to a saved image within this scratch Bus's attachments storage.
2. Type `clipboard picture`, press Enter → history associates image with that message; draft chip clears. State records the saved copy, which is readable.
3. Replace clipboard with unrelated text/image after sending → sent image stays associated with the original message and its stored copy remains readable.

**Evidence:** steps 1–3: chip/hover text, scratch attachment path and saved pixels compared with reference; any screenshot is judged by the runner. SKIP with reason if the runner cannot capture this evidence.

**Pass criteria:** clipboard pixels are saved and sent; a clipboard change cannot alter history.

### APP-10 — History image rendering and overlay cleanup

**Preconditions/setup:** graphics-capable host with known cell dimensions; R(`qa-images`), A with alpha, P(alpha). Attach red-diagonal and blue-circle via APP-08's UI recipe (not its run), type `two pictures`, send; keep both thumbnails fully visible. Steps 1–3: SKIP with reason if the runner cannot capture rendered-image screenshots.

1. Inspect history → both actual pictures appear in attachment order under their message, recognisable against references; no blank placeholders, swapped or duplicated image, text overlap, or stretching that changes their shapes.
2. Open Ctrl+O → pictures disappear under the file form; press Esc → both return. Switch to agent terminal, then F6 → pictures return without stale copies elsewhere.
3. Scroll history until pictures leave view, then back → pictures draw in history only; no remnants cover sidebar, notes, composer or other messages.

**Evidence:** steps 1–3: screenshot judged by the runner at each transition against reference PNGs, with terminal dimensions; state paths corroborate identity only. SKIP with reason if the runner cannot capture this evidence; text alone cannot judge pixels.

**Pass criteria:** visible pixels, ordering, containment and removal/redraw all match.

### APP-11 — History scrolling, find and plain-text copy

**Preconditions/setup:** R(`qa-history`), A with alpha, P(alpha); send enough fixed developer messages to overflow history, including two `find-marker` messages separated by other text; distinct oldest/newest markers. No model response needed.

1. Wheel up over history → earlier messages appear without editing draft; wheel down to newest → latest marker appears and chronology is preserved.
2. Press Ctrl+F, type `find-marker` → Find panel/count and newest match highlight; Enter or Up moves to older match, Down returns newer. Press Backspace 11 times until the visible Find query is empty; type `zz-absent-qq` (absent from all history) → query is exactly `zz-absent-qq`, panel says `no matches`; Esc closes and restores chat composition. Ctrl+A/Ctrl+K are no-ops in Find; Backspace removes the final character.
3. Drag over a short history text span, release → selected text is copied as plain text. Paste into an empty composer to inspect it, then Ctrl+C to clear.

**Evidence:** visible row text/order, match counts and highlighted cell backgrounds, copied text.

**Pass criteria:** history navigation, search direction and copying match the displayed text.

## Chat history links

Basis: [PR #22](https://github.com/asura1234/bus/pull/22), product head `aad7e4e3` (base `6ee03203`), source rechecked 2026-10-12.
Historical run `runner-20261011-192530` at `a61d26bc`: the prior nine LINK cases PASS. The reviewed assertions below await a new run.

**Shared setup/evidence for LINK-01..10:** apply the run contract and R/A/P recipes above to each case's own room/selected worker(s). Fixed agent-authored fixtures use fake alpha and beta: `bus send --room ROOM --as alpha --to beta --text TEXT` in that scratch session. Developer messages use the composer; no generated reply is required.

- Prepare short existing absolute non-executable text documents F/G, with a space in G's filename (e.g. report with space.txt), author-agent PWD W with relative.txt, an existing scratch-owned `~/P` fixture beneath Bus's effective home H, and absent W/missing.txt. Record H and actual targets; keep link tokens on one displayed row at 120×40; use only scratch-owned files.
- Text-link blue is RGB(90,150,255), underlined. Allow 5 seconds after a path first becomes visible for existence-check styling; initial unlinked styling is allowed. This is a QA wait budget, not a product deadline; record elapsed time.
- Plain click means left press/release on the same visible text/filename/thumbnail cell, without dragging or modifiers. Compare recorded effects before/after each gesture: each positive click records exactly one opener request with the specified target and operation (normal open or reveal); negative gestures record neither. Normal document opens have no reveal flag. Browser windows/opener exit codes alone are insufficient. Capture scratch author/PWD/attachment state and each step's specified evidence; unavailable evidence or fixtures remain SKIP with reason under the run contract.

### LINK-01 — HTTP and HTTPS styling and plain-click opening

**Preconditions/setup:** own room/worker; send `link http://example.test/a end`, `link https://example.test/b?q=1 end`, and `https://en.wikipedia.org/wiki/Function_(mathematics)` as separate developer messages; keep the complete URL on one row.

1. Inspect history → all three complete URL tokens blue + underlined, including both balanced parentheses; no 📎 added.
2. Plain-click each URL → recorded effect: open with target that exact full URL once, including scheme/query and `(mathematics)` for the third.
3. Click neighbouring ordinary text → no recorded open effect; composer remains usable.

**Evidence:** step 1: cells/screenshot judged by the runner; steps 2–3: recorded open effects/absence, history text. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** both schemes look clickable; balanced parentheses remain in the exact opened URL.

### LINK-02 — Existing paths, quoted spaces, punctuation and line references

**Preconditions/setup:** own room/worker; send short rows containing `F`, `"F"`, `(F),`, `F:12`, `F:12:3`, and `"G"`, substituting actual paths; G has spaces in its filename and its whole quoted path fits on one row.

1. Inspect settled history → path tokens blue + underlined, including :12/:12:3 and all of G's characters/spaces as one link; surrounding quotes/brackets and trailing sentence punctuation stay outside target.
2. Plain-click each path/reference, including within each word of G's filename → the five F variants each record one normal open of absolute F; each click within G records one normal open of the entire absolute G, with spaces intact and quotes excluded. Line/column suffixes are excluded from F's target.

**Evidence:** step 1: cells/screenshot judged by the runner, fixture existence; step 2: exact recorded targets and normal-open operation per click. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** quoted spaces stay in one file target; decoration/suffixes never change that target.

### LINK-03 — Relative and home paths use the correct base

**Preconditions/setup:** own room with alpha (PWD W) and beta (different PWD); relative.txt exists in W and scratch Bus's launch directory. Post `./relative.txt` and `~/P` as alpha to beta; P identifies the scratch-owned home fixture above.

1. View that history with beta selected as recipient → both existing paths become blue + underlined; selection does not change the message's author.
2. Plain-click each → recorded effect: open with target `W/./relative.txt` and absolute `H/P` respectively; never beta's directory. Preserve lexical `./` in the recorded target; source does not canonicalize it.
3. Type `./relative.txt` in composer, send as developer, then click that history token → no blue/underline path-link style and no recorded open effect, even though the file exists in the launch directory. Developer messages have no relative-path base.

**Evidence:** steps 1–3: author/PWD state and cells; steps 2–3: recorded open effects/absence with named targets. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** agent paths use author PWD, ~/ uses scratch Bus home, developer-relative tokens remain unlinked.

### LINK-04 — Individual paths inside files: [...] lists

**Preconditions/setup:** own room/worker; send `files: ["F", "G"]` as message text, substituting short existing absolute paths, with spaces in G's filename. Keep the entire list on one displayed row; this is not the attachment picker.

1. Inspect history → F and the whole G (including spaces) separately blue + underlined; list remains readable, surrounding quotes/punctuation outside the links, no 📎 or attachment control added below composer.
2. Plain-click F, then each word of G's filename → one recorded normal open per click, with exact absolute F or entire G respectively; neither opens the list, a partial word, quoted string or the other file.

**Evidence:** step 1: cells/screenshot judged by the runner including composer bottom; step 2: recorded normal-open requests with complete F/G targets. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** each list member has its own complete target, including quoted filename spaces.

### LINK-05 — Link style wins inside inline and fenced code

**Preconditions/setup:** own room with alpha and beta; post Markdown as alpha to beta with inline code `plain F` and a fenced block containing a URL and F, substituting the real path; tokens fit on rows.

1. Inspect settled history → URL/path cells blue + underlined in both contexts; non-path inline code `plain` keeps its cyan code style without link underline.
2. Plain-click inline path, fenced path and fenced URL → recorded effect: open with target F, F and exact URL respectively; no backticks/fence markers included.

**Evidence:** step 1: cells/screenshot judged by the runner; step 2: recorded open effects with F/F/URL targets; raw history. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** code formatting preserves correct links and non-link code style.

### LINK-06 — File attachment names: hover, selection and bounded opening

**Preconditions/setup:** own room/worker selected; attach F and G one at a time via Ctrl+O, type `attached files`, send. Keep each full attachment name visible with blank cells beside it; fake worker need not reply.

1. Inspect history → attachment names directly under their message, blue + underlined, with no square brackets or 📎; blank row separates attachments from any reply/placeholder. No new link UI appears beneath composer.
2. Hover F's name → full F path visible; move to blank space beside it on the same row → full-path hover disappears. Plain-click that blank space → no recorded open/reveal. Repeat for G.
3. Drag across each complete attachment name with distinct press/release cells → selection visible; release copies exactly that filename as plain text, including G's spaces; no recorded open/reveal.
4. Plain-click each name → one recorded normal open with target actual F or G respectively; the target matches its stored attachment path.

**Evidence:** step 1: cells/screenshot judged by the runner; step 2: pointer/name/blank-cell positions, hover text/absence and recorded effects/absence; step 3: selection cells, exact copied text and effects/absence; step 4: recorded normal opens with F/G targets; scratch attachment state. SKIP with reason for unavailable evidence. **Pass criteria:** only names show their hover and open; blank space opens nothing; dragging copies without opening.

### LINK-07 — Image thumbnail and filename fallback have bounded click areas

**Preconditions/setup:** independent subruns with their own room/selected worker: graphics-enabled host and host without image support. Paste a known PNG with Ctrl+V and send; state identifies saved copy I inside scratch Bus attachments. Keep the whole thumbnail visible, at least two cells wide, with blank cells beside its right edge on the same rows; retain reference PNG.

1. Graphics subrun: inspect history → expected thumbnail under its message, no caption, blue border, underline or 📎. Plain-click inside the picture's own cells → one recorded normal open with target I. Plain-click blank space beside the picture on the same row → no recorded open/reveal. Drag from one picture cell to a different cell on that same picture row, staying inside its bounds → no recorded open/reveal. Plain-click the picture afterward → exactly one normal open of I for that click. Evidence: screenshot judged by the runner identifying picture bounds and click/drag positions, plus recorded opener requests/absence before/after each gesture; SKIP with reason if unavailable.
2. Text-only subrun: inspect history → image filename blue + underlined, no brackets or 📎; hover over the name reveals I; plain-click filename → one recorded normal open with target I. Evidence: filename cells/hover text and recorded open effect; SKIP with reason if the runner cannot capture this evidence or supply clipboard pixels.

**Evidence:** step-specific evidence above and saved-copy pixel identity. **Pass criteria:** each plain click on picture cells or filename fallback opens the saved image once; dragging within the picture and clicking beside it open nothing. Blue/underline applies to filename fallback. Unavailable subruns remain SKIP with reason.

### LINK-08 — Dragging copies; a press outside a link cannot open it

**Preconditions/setup:** own room/worker; send short `before F after` and `https://example.test/copy` as developer messages.

1. Press the left mouse button on ordinary `before` text, then release over F with no intervening drag → no recorded open/reveal. Repeat with a press on ordinary text and release over the URL → no recorded open/reveal.
2. Drag across F with distinct press/release cells → text selection visible, no recorded open/reveal; release copies selected text as plain text.
3. Drag across entire URL → clipboard contains exact URL without markup/📎; no recorded open/reveal. Plain-click URL afterward → recorded effect: open with target that URL once.

**Evidence:** step 1: press/release cell positions and recorded effects/absence for each gesture; steps 2–3: selection cells/screenshot judged by the runner, exact copied text and recorded effects/absence per gesture. SKIP with reason for unavailable evidence. **Pass criteria:** opening requires a click starting and ending on that link; dragging copies without opening.

### LINK-09 — Negative targets and links after navigation

**Preconditions/setup:** own room/worker; send developer messages containing missing.txt's absolute path, `[hidden](https://example.test/hidden)` and an existing short F, plus enough messages to overflow history.

1. After the path-probe wait, click missing path and hidden label → no recorded open effect; missing path has no confirmed path-link style. Do not reject Markdown's own label styling.
2. Scroll F out of view/back, resize 120×40 to 100×30 keeping F on one row → F remains blue + underlined; click its new cell position → recorded effect: open with target F only.
3. Type `/help`, Enter, scroll guide → `Click a link` guidance present; Esc returns.

**Evidence:** steps 1–2: cells/screenshots judged by the runner and recorded open effects/absence; step 3: guide text. SKIP with reason if the runner cannot capture a step's evidence. **Pass criteria:** unsupported targets do not open; visible links keep correct click locations.

### LINK-10 — Executable and app paths reveal without launching

**Preconditions/setup:** own room/selected worker; prepare scratch-owned absolute paths: E, a harmless script with no launcher extension and a Unix execute bit; C, a .command file without an execute bit; A, a minimal .app bundle; L, a symlink named app-link.txt (no launcher suffix) pointing to A; T/U/V, small .terminal/.webloc/.fileloc files without execute bits; and F, a plain text document without execute bits. Record L's exact link path and resolved bundle target. Any runnable fixture would only create its own initially absent scratch marker if executed; record fixture identities, permissions and marker paths. Send their paths as developer messages, keeping each on one row. On non-Unix hosts SKIP the execute-bit fixture with reason; keep other assertions independent. Require captured opener arguments/operation; no real application needs to launch.

1. Inspect settled history → all existing fixture paths blue + underlined; markers absent before clicking.
2. Plain-click E → exactly one recorded opener request: reveal E in the file manager, with no normal-open request for E and no execution marker. On macOS the recorded `open` arguments are `-R` followed by the entire absolute E path.
3. Plain-click C then A → one reveal request for each complete path, no normal-open request for either and no execution markers; macOS records `open -R PATH` for each. The .command fixture remains protected even without an execute bit.
4. Plain-click L, T, U and V individually → one reveal request for each exact absolute clicked path, with macOS `open -R PATH`, no normal-open request and no execution markers. L reveals the original app-link.txt path (not a normal open of it or a substituted A target), even though its own suffix looks like a document. The .terminal/.webloc/.fileloc files reveal without opening Terminal settings or following their locations.
5. Plain-click F → one normal-open request for exact absolute F (on macOS `open F`, without `-R`); no launch/reveal request for another fixture and all execution markers stay absent.

**Evidence:** step 1: styled cells, fixture permissions/identity/marker state and L's symlink-to-A identity; steps 2–5: before/after recorded opener operation, ordered arguments and exact clicked targets, plus absent execution markers. A target alone cannot distinguish reveal from launch; SKIP with reason if the runner cannot capture arguments/operation or a fixture. **Pass criteria:** executable scripts, .app/.command paths, a symlink to an app, and .terminal/.webloc/.fileloc paths only reveal; none is opened/run or followed, and the ordinary document still requests a normal open without `-R`. On Windows reveal selects the path in Explorer; on Linux it opens the containing directory rather than the file. Record that platform's equivalent reveal request.

## Remaining whole-app cases

### APP-12 — Notes, isolated drafts, recall and stash

**Preconditions/setup:** R(`qa-notes-a`), A with alpha, P(alpha); R(`qa-notes-b`).

1. Click qa-notes-a, press F3, type `status board`, press Enter, type `second row`, press F3 → notes have two lines; no chat message was sent.
2. Click qa-notes-a's composer, type `draft-a`; switch to qa-notes-b, click its composer, type `draft-b`, return → draft-a and qa-notes-a notes restored. Return to qa-notes-b → only draft-b, its own notes.
3. Click qa-notes-a and its composer, press Ctrl+S → draft clears; Ctrl+S again → draft-a restored. Send it; press Up on empty draft → draft-a recalled; append ` edited` → unsent draft is exactly `draft-a edited`.
4. With the composer focused, press Esc twice less than 800 ms apart → draft clears; the first Up restores the archived draft `draft-a edited`, rather than the older sent `draft-a`. No message is sent.

**Evidence:** visible notes/draft text per room and each stash/recall transition; history count and exact first-Up text `draft-a edited`. Saved scratch state may corroborate completed saves; it is not required to reflect each live edit.

**Pass criteria:** edits stay in their room; notes/stash/recall do not silently send messages; the first Up after archival restores the newer archived draft.

### APP-13 — Actionable notification in the composer

**Preconditions/setup:** R(`qa-notice`), no selected recipients; graphics fixture optional. Note original message count.

1. Type `notice draft`, press Enter → visible notice `Choose agents with @ before sending.` titles composer; draft stays editable and no message is submitted.
2. Observe for at least 16 seconds without repeating the error → transient notice expires; draft persists; no repeated persistent footer is added below composer.
3. Press Ctrl+P, then Esc → UI responds; type/edit draft → normal input still works.

**Evidence:** steps 1–2: terminal-visible notification text with timestamps, or screenshot judged by the runner; step 3: input/draft text and state message count. SKIP with reason if the runner cannot capture the required evidence for a step.

**Pass criteria:** notice is visible, transient, preserves input and does not count as a send.

### APP-14 — Settings and real colour-blind palette change

**Preconditions/setup:** isolated default settings; R(`qa-colours`), at least two workers with differing normal/accessibility palettes, P(both); send `palette sample`.

1. Capture each worker's sidebar name, recipient chip and history recipient/name foreground cells in normal mode → identities are distinct; record actual RGB values.
2. Click Settings → `Color blind mode` is unchecked. Press Enter on that row → checked; press Esc → same room. Compare the same named identity cells → at least one foreground actually changes; workers remain distinct/readable and their corresponding chips/history names use their changed palette consistently.
3. Reopen Settings → checked persists. Click the checkbox, Esc → unchecked and each sampled identity foreground returns to its original value.

**Evidence:** steps 1–3: same named cells' text/foreground RGB before/on/off, scratch setting; screenshot judged by the runner for readability. SKIP with reason for colour/readability assertions if the runner cannot capture this evidence; checkbox checks remain independent.

**Pass criteria:** both state and painted identity colours change/revert; checkbox-only proof fails.

### APP-15 — Sound settings, new-room defaults and compaction limit

**Preconditions/setup:** default isolated settings; R(`qa-sound-a`), R(`qa-sound-b`).

1. Click Settings → MASTER sound on; work rooms/All rooms off; sound names shown. Move with Up/Down; Enter on All rooms → both work-room checkboxes on.
2. Click qa-sound-a's checkbox → it alone turns off; All rooms becomes mixed `[-]`. Enter on All rooms → both on. Close, R(`qa-sound-new`), reopen Settings → new room also on; MASTER's separate setting is unaffected.
3. Focus a sound row, press Right then Left → installed sound choice changes and returns, with previews. If only Default exists, record that inventory; no different sound name is expected. Preview evidence: recorded effect, sound with target selected sound name; SKIP with reason for the preview assertion if the runner cannot capture this evidence.
4. Move to `Max compactions per agent (1–100)`, record value, change with Left/Right within bounds → displayed integer changes; close/reopen → value persists.

**Evidence:** checkbox/sound/limit text at each step, scratch settings and per-room sound state; step 3 preview: recorded effect, sound with target selected sound name. SKIP with reason for preview evidence unavailable to the runner; settings assertions remain independently scored.

**Pass criteria:** toggles/default inheritance/mixed state/limit edits work; no model compaction required.

### APP-16 — New-message sound notifications

**Preconditions/setup:** R(`qa-ring`), A with **alpha and beta, two distinct fake workers in qa-ring**; R(`qa-view`). Arrange each fixed incoming post through the supported scratch CLI as `bus send --room qa-ring --as alpha --to beta --text TEXT`, with TEXT respectively `ring-one`, `silent-two`, and `ring-three`, triggered individually at the numbered steps. Alpha is the author and beta is the non-author recipient; no reply is required to satisfy the post assertion. Start observation after startup has settled for >2 seconds; leave >2 seconds between triggers so sounds are not grouped. Let prior deliveries settle before each observation window, and attribute any fake-worker reply separately from the authored post or developer send. For the final developer send, select **only alpha** with P(alpha). Steps 1–3 sound assertions: SKIP with reason if the runner cannot capture recorded sound effects; view/message checks remain independent.

1. In Settings enable qa-ring sound, close and view qa-view; allow ring-one to arrive → one notification sound; current view is not stolen. Click qa-ring → fixed message is visible, attributed to alpha.
2. Disable qa-ring sound, return to qa-view; allow silent-two → message reaches qa-ring history; no notification sound from trigger through two seconds after confirmed arrival.
3. Enable sound, allow ring-three → sound returns. In qa-ring use P(alpha), type `developer-no-ding`, press Enter → one's own message produces no new-message ding.

**Evidence:** steps 1/3: recorded effect, sound with target qa-ring's selected sound name, correlated to room/message/author; steps 2/3 silence: a complete recorded-effects window showing no new-message sound after the specified post. Capture settings/history/view text and scratch state separately. SKIP with reason if the runner cannot capture sound evidence; checkmarks alone cannot prove it.

**Pass criteria:** enabled incoming posts ring, disabled and developer posts do not; view stays usable.

### APP-17 — Rename a room, including cancellation

**Preconditions/setup:** R(`qa-rename-room`), A with alpha; notes/draft/message fixtures.

1. Click room, press F2, replace name with `cancel-name`, Esc → old name retained.
2. Press F2, Ctrl+A then Ctrl+K, type `renamed-room`, Enter → sidebar/main header update; same room ID, workers, notes, draft and history remain.
3. Double-click its room name, replace with `mouse-renamed`, Enter → same room renamed again; navigating away and back retains it.

**Evidence:** editable/committed labels and scratch ID/data before/after.

**Pass criteria:** both routes rename the selected room only; cancellation preserves it.

### APP-18 — Rename an agent, including cancellation

**Preconditions/setup:** R(`qa-rename-agent`), A with alpha/beta; P(alpha).

1. Click alpha's name to open its terminal, press F2, edit, Esc → name stays alpha.
2. Press F2, replace name with `renamed-alpha`, Enter; F6 → sidebar and recipient chip update; same agent ID/provider/PWD/room; beta stays beta.
3. Double-click renamed-alpha's sidebar name, replace with `mouse-alpha`, Enter → renamed again; terminal remains selectable by the new name.

**Evidence:** name/chip/terminal text and scratch identities/recipients before/after.

**Pass criteria:** keyboard and mouse rename the intended agent without replacing its identity.

### APP-19 — Delete an agent after confirmation

**Preconditions/setup:** R(`qa-delete-agent`), A with alpha/beta, P(both).

1. Click × beside alpha → warning names alpha and describes closing/deleting its session data. Press Esc → dialog closes; both agents and recipients remain.
2. Click alpha's × again, press Enter → after completion alpha disappears from sidebar/picker/chips and state; beta and room remain. No deleted terminal stays open.
3. Click beta, press F6 → surviving worker and room still usable.

**Evidence:** warning text/screenshot, state agent/recipient IDs before/cancel/confirm.

**Pass criteria:** cancellation is harmless; confirmation removes exactly the target agent.

### APP-20 — Delete a room and its agents after confirmation

**Preconditions/setup:** R(`qa-delete-room`) with two workers; R(`qa-keep-room`) with one worker. Also create a MASTER orchestrator for qa-delete-room using APP-04's UI recipe; no case dependency. Add identifiable room notes/history/draft.

1. Click qa-delete-room, click its × → warning names room, worker count and its MASTER orchestrator. Click Cancel → room and all associated agents remain.
2. Reopen warning, click OK → room, its workers and its orchestrator disappear from UI/state; qa-keep-room and its worker remain selectable; no stale deleted view.
3. Click MASTER → permanent room remains; deleted orchestrator is absent.

**Evidence:** complete warning, state room/worker/orchestrator IDs and screenshots before/after.

**Pass criteria:** confirmed deletion removes the named room's group and preserves the other room.

### APP-21 — Resize with draft, history, images and forms

**Preconditions/setup:** create eight extra work rooms `qa-extra-1` through `qa-extra-8` using R, then R(`qa-resize`), A with alpha/beta, P(both). With default MASTER and bus, there are exactly 11 rooms total and two workers in qa-resize; keep worker PWD details collapsed. Add short history, notes, an unsent draft and both sent PNGs. This count forces sidebar scrolling at 80×24. Graphics host required for image assertions.

1. Resize 120×40 → 80×24 → 160×50 → layout reflows within terminal bounds; names may truncate/wrap, but controls do not overlap; draft/selection/data stay intact.
2. At each size scroll sidebar/history as needed, click a visible worker then F6 → hit targets follow current positions; images appear only when fully in view, redraw on return, and leave no stale pixels over other regions.
3. Open Ctrl+N, resize 80×24 then 120×40, cancel → fields/buttons remain usable; no unintended agent. Toggle Ctrl+Shift+E twice in room → full-height then compact composer, preserving draft; normal layout returns and images do not cover it.

**Evidence:** steps 1–3: current-size text/cell geometry, visible draft/notes/recipient text, state room/agent identities (11 rooms, two workers), and click outcomes before/after sidebar scrolling; image/layout screenshots judged by the runner. SKIP with reason for visual assertions if unavailable; live draft proof comes from the screen, with saved state only corroborating completed saves.

**Pass criteria:** resized UI remains operable, preserves data and redraws text/images cleanly.

### APP-22 — Save and quit

**Preconditions/setup:** R(`qa-quit`), A with alpha; notes and unsent draft markers; record scratch session/server/agent process identities for lifecycle observation.

1. Edit notes and draft, press Ctrl+Q, wait for the Bus process to exit → Bus saves, emits its terminal-restore sequence, prints `Stopping Bus and its agents...`, stops its scratch server/agent panes, and exits with code 0 (no terminating signal). `Saving before exit…` may be brief. A shell prompt is not required when Bus runs directly in a terminal/PTY.
2. After exit and terminal-output capture finalization, inspect the complete captured output and saved scratch state → restore output disables enabled mouse/focus/bracketed-paste modes and keyboard enhancements, leaves the alternate screen (`ESC[?1049l` on VT hosts), shows the cursor (`ESC[?25h`) and resets its shape (`ESC[0 q`); no Bus image remnants on a captured terminal view. Latest notes/draft/room/agent identities are saved exactly; markers were not sent. The named scratch server is unreachable and its managed fake panes/processes have stopped; allow the product's 15-second stop wait after saving. A stop error is a failure. Do not judge restore bytes from a capture still being written.

**Evidence:** step 1: recorded Bus process exit status 0 with no signal and final status text; step 2: finalized raw terminal output containing the platform's restore sequence, saved scratch data and lifecycle outcome for the pre-identified server/agents; screenshot judged by the runner for cursor/image cleanup. SKIP with reason for unavailable evidence; neither a shell prompt nor shutdown text substitutes for process/restore/lifecycle evidence.

**Pass criteria:** saved data, terminal restoration, clean UI exit and scratch session shutdown match all captured expectations. Never force quit to claim a clean save.

### APP-23 — Dev performance overlay toggle

**Preconditions/setup:** a dev variant (debug build or `--dev` session, as the driver starts); fresh scratch session at 120×40; default room `bus` open.

1. Click Settings → a `DEVELOPER` group below `AGENTS` lists `[ ] Performance overlay (CPU, memory, FPS)`; the top row shows no `CPU … MEM … FPS …` label.
2. Click the `Performance overlay` row (or move to it with ↓ and press Enter) → it reads `[x]`; within 2 s the top-right of row 0 shows ` CPU <n>% MEM <n>MB FPS <n> ` (CPU/MEM may read `n/a` only for the first second).
3. Wait 3 s, capturing row 0 at least twice → the label stays on row 0 and refreshes (at least one of CPU/MEM/FPS changes, or the snapshot frame `seq` advances while the label is present); FPS shows a number once a full second has passed.
4. Press Esc, then click a room and type a few characters → the label stays visible over every screen, including outside Settings.
5. Reopen Settings and toggle the row again → `[ ]`; within 2 s no `CPU … MEM … FPS` text remains anywhere on screen, and nothing else on row 0 is left damaged.

**Evidence:** steps 1–5: row 0 text and the Settings row text from snapshots/find, with frame `seq`; screenshot judged by the runner for step 2 and step 5. The overlay is client-local and not saved; scratch settings files must not change.

**Pass criteria:** the overlay appears only while toggled on, shows CPU, memory and FPS on row 0, refreshes about once a second, and disappears completely when toggled off.

## Decisions

- **Shortcuts / product finding:** test Ctrl+O files, Ctrl+P recipients, Ctrl+F find, literal @/+ typing and Ctrl+C draft-clear (terminal Ctrl+C interrupts); CLI help's conflicting shortcuts are a product documentation finding, not a reason to change these expectations (`rooms/help.rs`, `rooms/input/{composer,mod}.rs`, `cli/help.rs`).
- **Quit / product finding:** Ctrl+Q saves, restores the terminal, then stops server and agents; CLI help's claim that quit leaves them running is stale product documentation. Save failure exposes retry/force-exit after 5 seconds; stopping has a 15-second wait (`client/{effects,run}.rs`, `rooms/ui.rs`, `cli/stop.rs`).
- **Size:** source enforces no terminal minimum or small-window warning; layout shrinks/clips to available cells. APP-21's 80×24 is this suite's lower usability test size, not a claimed product minimum; below it, hidden/clipped controls alone are not failures (`rooms/render/{layout,view,paint}.rs`).
- **Clipboard / screenshots:** require actual clipboard/saved pixels where tested and screenshots judged by the runner for visual claims; affected assertions are SKIP with reason if the runner cannot capture evidence, not product failures.
- **Notifications / sound / unread:** terminal-visible notices and recorded effects are in scope; record sound with its chosen sound target and message/room context. Desktop banners are excluded; today's room sidebar renders no unread badge (`client/notifications.rs`, `rooms/{toast,ring}.rs`, `rooms/render/sidebar.rs`). Unavailable evidence is SKIP with reason.
- **Wording:** use developer for the person operating Bus; retain actual UI strings such as `You` unchanged.
- **Thumbnail style:** #22 renders the picture without filename/caption or link decoration; only its picture cells open the saved image; blank cells beside it do not. Only filename fallback gets blue RGB(90,150,255) + underline (`history/exchange.rs`, `links.rs`); no 📎 in either presentation.
- **Relative paths:** agent-authored paths join author PWD without canonicalizing; relative paths authored by a developer or Bus remain unlinked. Absolute and ~/ paths work for either author; ~/ uses the scratch Bus process's effective home (`history/exchange.rs::link_scope`, `links.rs::candidate`).
- **Timing:** a background existence check completes before path styling/opening; results appear on the UI tick (100 ms idle, 10 ms pending commands). There is no product deadline; use a 5-second QA wait after first visibility, then report a captured styling timeout with elapsed time (`links.rs::PathProbe`, `ui.rs`). URLs need no existence check.
- **Evidence:** a normal open records the unchanged URL or absolute path X; a reveal records the platform's reveal operation/arguments (macOS `open -R X`). Capture arguments so reveal and normal open are distinguishable; neither requires an external application display (`effects.rs`, `platform/desktop/{macos,linux}.rs`, `platform/windows/desktop.rs`). Screenshots are judged by the runner; unavailable clipboard/pixels/effects → affected step SKIP with reason. Desktop banners are out of scope; terminal-visible notices/recorded effects remain in scope.
- **Link boundaries:** hidden-label Markdown links and paths split across rows are outside clickable-link acceptance; LINK-09 checks that hidden labels do not open.
- **Click boundaries / launchers:** filename hover/click areas cover the name, thumbnail clicks cover picture cells, and opening requires a press/release on the same cell without movement or selection; even a drag entirely inside a picture cancels opening. Quoted paths retain spaces and URLs retain balanced parentheses (`render/history_links.rs`, `selection.rs`, `links.rs`). Executable regular files on Unix and launcher extensions such as .app/.command/.terminal/.webloc/.fileloc reveal in the file manager. Symlinks are also judged by their resolved target; a symlink to an app reveals the clicked link path instead of opening/running its target (`effects.rs`, `platform/fs.rs`).
- **Runner-neutral edits / quit:** Find uses Backspace to remove query characters; PWD completion consumes the first Tab while a matching suggestion is shown (`chat_search.rs`, `input/forms.rs`). Observe live drafts on screen; saved files may lag. Judge quit from exit status and finalized terminal-restore output; a shell prompt is optional (`client/run.rs`, `client/host_terminal/setup.rs`).
- **Recall acceptance:** APP-12 requires the first Up to restore the archived draft. This includes the separately landing recall-order product fix; an older train build still fails that assertion until rebased onto the fix from master. Keep the precise expectation when judging that build.
