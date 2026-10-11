# Bus TUI whole-app QA — draft

Basis: main worktree `6bdfe7c83c51c2611f7c717a3bc663c114ca29c8`, source rechecked 2026-10-12.
Read: [README](../../README.md), user-visible strings/actions in
[TUI help](../../src/client/rooms/help.rs), room forms, sidebar, history and settings
under `src/client/`, and `target/debug/bus --help`. No driver implementation or
driver documentation was used. This is an acceptance draft, not an executed run.

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
  corroborate room/agent IDs, names, membership, drafts, messages, attachments and
  settings; state alone cannot prove visible layout, colours or images.
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
workspace directory, press Tab → optional args; leave args empty, click `Add (Enter)`
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

### APP-01 — First start and empty conversation

**Preconditions/setup:** pristine scratch data/settings; no pre-created workers.

1. Launch scratch Bus and wait for startup → permanent `MASTER` and the default work room `bus` appear; `bus` opens. Its history and AGENTS list have no messages or workers; composer says `Message selected agents…`, with `@ Choose agents`.
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
2. Fill name `qa-master`, select Claude Code, set scratch PWD; select `qa-master-target` as Orchestrates room; click Add → fake terminal opens.
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

1. Type `line one`, press Ctrl+J, type `line two @ +` → two draft lines; literal @ and + are text. If Shift+Enter is supported, also verify it adds a newline.
2. Press Ctrl+A then Ctrl+K on the last line; type `line two revised` → only that line is replaced; first line remains. Press Enter once → one history message shows both lines, `You → alpha`; composer clears after acceptance.
3. Inspect scratch history → exact text `line one\nline two revised`, alpha as sole recipient. Press Enter again → no duplicate message or empty-send error.
4. Type `throwaway`, press Ctrl+C → draft clears; Bus remains open.

**Evidence:** before/after draft/history text, author/recipient cells, state message count/text.

**Pass criteria:** one Enter sends exactly the edited draft to the selected worker; no reply required.

### APP-08 — Attach local files, remove one, and send

**Preconditions/setup:** R(`qa-files`), A with alpha, P(alpha); fixture files exist.

1. Press Ctrl+O → `Local file path` form. Replace its path with absolute notes.txt; click Add → room returns with a notes.txt attachment chip.
2. Click `+` by the composer, attach `report with space.txt` the same way → both chips present; hover a chip → full path is visible. Use file-row arrows/wheel if needed. Click the report chip's × → only notes.txt remains.
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
2. Press Ctrl+F, type `find-marker` → Find panel/count and newest match highlight; Enter or Up moves to older match, Down returns newer. Change query to a known absent string → `no matches`; Esc closes and restores chat composition.
3. Drag over a short history text span, release → selected text is copied as plain text. Paste into an empty composer to inspect it, then Ctrl+C to clear.

**Evidence:** visible row text/order, match counts and highlighted cell backgrounds, copied text.

**Pass criteria:** history navigation, search direction and copying match the displayed text.

### APP-12 — Notes, isolated drafts, recall and stash

**Preconditions/setup:** R(`qa-notes-a`), A with alpha, P(alpha); R(`qa-notes-b`).

1. Click qa-notes-a, press F3, type `status board`, press Enter, type `second row`, press F3 → notes have two lines; no chat message was sent.
2. Type `draft-a`, switch to qa-notes-b, type `draft-b`, return → draft-a and qa-notes-a notes restored. Return to qa-notes-b → only draft-b, its own notes.
3. In qa-notes-a, press Ctrl+S → draft clears; Ctrl+S again → draft-a restored. Send it; press Up on empty draft → draft-a recalled; edit without sending.
4. Press Esc twice promptly → draft clears; Up → archived text can be restored.

**Evidence:** notes/draft text per room, scratch state, history count, recalled text.

**Pass criteria:** edits stay in their room; notes/stash/recall do not silently send messages.

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

**Preconditions/setup:** R(`qa-resize`), two workers selected; short text/history, notes, unsent draft, both sent PNGs, enough sidebar content to require scrolling at smaller size. Graphics host required for image assertions.

1. Resize 120×40 → 80×24 → 160×50 → layout reflows within terminal bounds; names may truncate/wrap, but controls do not overlap; draft/selection/data stay intact.
2. At each size scroll sidebar/history as needed, click a visible worker then F6 → hit targets follow current positions; images appear only when fully in view, redraw on return, and leave no stale pixels over other regions.
3. Open Ctrl+N, resize 80×24 then 120×40, cancel → fields/buttons remain usable; no unintended agent. Toggle Ctrl+Shift+E twice in room → full-height then compact composer, preserving draft; normal layout returns and images do not cover it.

**Evidence:** steps 1–3: current-size text/cell geometry, draft/state and hit outcomes; image/layout screenshots judged by the runner. SKIP with reason for visual assertions if the runner cannot capture this evidence; text/state checks remain independent.

**Pass criteria:** resized UI remains operable, preserves data and redraws text/images cleanly.

### APP-22 — Save and quit

**Preconditions/setup:** R(`qa-quit`), A with alpha; notes and unsent draft markers; record scratch session/server/agent process identities for lifecycle observation.

1. Edit notes and draft, press Ctrl+Q → Bus saves, restores the terminal, prints `Stopping Bus and its agents...`, stops its scratch server/agent panes and exits. `Saving before exit…` may be brief. Host returns to its shell with normal cursor/input and no Bus image remnants.
2. Inspect saved scratch state after exit → exact latest notes/draft/room/agent identities retained; markers were not sent. Recorded lifecycle outcome: the named scratch server is no longer reachable and its managed fake agent panes/processes have stopped; allow the product's 15-second stop wait, after its save phase. Do not accept a displayed stop error as successful shutdown.

**Evidence:** final shell text/exit result, saved scratch data and recorded lifecycle outcome for the pre-identified scratch server/agents; screenshot judged by the runner for cursor/image cleanup. SKIP with reason for any assertion whose evidence the runner cannot capture; do not substitute shutdown text for lifecycle evidence.

**Pass criteria:** saved data, terminal restoration, clean UI exit and scratch session shutdown match all captured expectations. Never force quit to claim a clean save.

## Decisions

- **Shortcuts / product finding:** test Ctrl+O files, Ctrl+P recipients, Ctrl+F find, literal @/+ typing and Ctrl+C draft-clear (terminal Ctrl+C interrupts); CLI help's conflicting shortcuts are a product documentation finding, not a reason to change these expectations (`rooms/help.rs`, `rooms/input/{composer,mod}.rs`, `cli/help.rs`).
- **Quit / product finding:** Ctrl+Q saves, restores the terminal, then stops server and agents; CLI help's claim that quit leaves them running is stale product documentation. Save failure exposes retry/force-exit after 5 seconds; stopping has a 15-second wait (`client/{effects,run}.rs`, `rooms/ui.rs`, `cli/stop.rs`).
- **Size:** source enforces no terminal minimum or small-window warning; layout shrinks/clips to available cells. APP-21's 80×24 is this suite's lower usability test size, not a claimed product minimum; below it, hidden/clipped controls alone are not failures (`rooms/render/{layout,view,paint}.rs`).
- **Clipboard / screenshots:** require actual clipboard/saved pixels where tested and screenshots judged by the runner for visual claims; affected assertions are SKIP with reason if the runner cannot capture evidence, not product failures.
- **Notifications / sound / unread:** terminal-visible notices and recorded effects are in scope; record sound with its chosen sound target and message/room context. Desktop banners are excluded; today's room sidebar renders no unread badge (`client/notifications.rs`, `rooms/{toast,ring}.rs`, `rooms/render/sidebar.rs`). Unavailable evidence is SKIP with reason.
- **Wording:** use developer for the person operating Bus; retain actual UI strings such as `You` unchanged.
