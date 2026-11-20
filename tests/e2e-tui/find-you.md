# Find messages to and from the developer

## Run contract

Use a fresh scratch Bus per case at 120×40 with the worktree's dev-tools binary,
fake providers, graphics none, isolated HOME/data and the worktree's Cargo target.
Never use the live Bus or desktop automation. CLI setup and deterministic sends
are allowed. Use TUI input for Find, filters and navigation. Capture every step's
snapshot, use cells for highlights and open the cited screenshots. No skips or
failed-step retries.

Use distinct nonmatching native answers for developer prompts: append
`fake:raw-reply ACK` to each developer fixture. Verify their requests settle
as replied with no failure reason and their rendered answers are present with
filters off. Find counts occurrences in rendered history, including answer text.
The query words below occur only in the intended prompts/posts. The selected
text match uses background `#f0be46`.

### FIND-01 — Find an incoming message buried in peer traffic

Setup: two idle fake Claude agents in `find-busy`. Send 12 `Needle peer before NN`
messages between them, one `Needle lost developer ask` post to You, then 12
`Needle peer after NN` messages. Wait for settlement. Open the room at the tail.

1. Verify the latest peer traffic is visible and the ask is out of view. Open
   Ctrl+F → empty Find field and both room-header filters unchecked. Verify
   neither direction appears in the Find panel itself. Capture and open a screenshot.
2. Type `Needle` → multiple matches once history has loaded. Press Alt+I →
   the incoming filter is checked, the peer traffic disappears, and the ask is
   the only text match (`1/1`). Capture its selected text cells.
3. Clear the query with Backspace, then type `lost`. Press Up and Down → each
   wraps to `1/1`. Append `x` → `no matches`; Backspace restores `1/1`.
4. Close and reopen Find → query is empty, no counter, incoming filter still
   checked. Turn it off → peer history returns. Verify all 24 peer prompts
   and the one developer post remain in scratch history.
5. Open `/help` → Ctrl+F documents text Find/navigation and Alt+I/Alt+O
   document the separate direction controls. Close help and stop scratch Bus.

### FIND-02 — Find sent messages and combine directions in a work room

Setup: use two idle fake Claude workers in `find-outgoing`. Create one peer
message `Needle peer noise`, two developer prompts `Needle outgoing older`
and `Needle outgoing newer`, and one direct post `Needle outgoing newer incoming`
in that order. Wait for settlement and open the room.

1. Press Alt+O, then Ctrl+F and type `Needle` → only outgoing is checked;
   two matches, newest selected. Up, Up, Down → `1/2`, `2/2`, `1/2`.
2. Clear the query and type `outgoing ` including the space → two matches.
   Append `newer` → `1/1` on the developer prompt.
3. Press Alt+I while Find is open → both filters are checked; the developer
   prompt and direct post both match, with no peer traffic. Use Up and Down
   to visit both distinct rows. Capture and open a union screenshot.
4. Backspace five times → three matches for `outgoing `. Press Alt+I → two
   outgoing matches. Press Alt+O → three unfiltered `outgoing ` matches.
   Escape and reopen Find → both filters are off and the query is empty.

### FIND-03 — Developer authorship in MASTER

Setup: a fake Claude orchestrator in MASTER, bound to `find-work`. Send a
developer prompt `Needle master outgoing`, then post `Needle master incoming`
from that orchestrator to You in MASTER. Wait for settlement and open MASTER.

1. Press Alt+O and open Find with query `Needle` → only `You → *` checked,
   `1/1` on the developer prompt. Its answer remains in the outgoing exchange.
   Up and Down each wrap to the same match.
2. Press Alt+I → both controls checked, with two matches. Navigate Up/Down
   across both and capture a screenshot showing the filters in the MASTER
   header and the selected text in history. Escape and stop scratch Bus.

### FIND-04 — Mouse and keyboard filters with a narrow Find panel

Setup: in `find-toggles` use two idle fake Claude workers to stage `Needle peer
noise`, developer prompts `Needle outgoing older` and `Needle outgoing newer`,
and an agent's direct post `Needle outgoing incoming`. Wait for settlement.
Start at 100×40 and resize to 60×40 in step 5.

1. Open Find and type `Needle` → four matches and separate, unchecked
   direction controls in a single header row above the Find panel. Capture
   and open a screenshot.
2. Click `* → You` → incoming only, `1/1` on the direct post. Press Alt+I →
   both off and four matches. Click `You → *` → outgoing only and two
   matches. Capture each direction's header cells and selected text.
3. Click `* → You` → both checked with three matches. Use Up and Down
   → the highlight moves across the union. Capture and open a screenshot.
4. Focus the query and append ` outgoing` → three matches. Navigate with
   Enter, Shift+Enter and Down. Click incoming to disable it → two matches;
   click outgoing to disable it → three unfiltered matches for the query.
5. Resize to 60×40 → both header checkboxes and the Find query remain
   readable and separately clickable. Click incoming → one match. Click
   outgoing → three. Capture and open a narrow screenshot, then close Find.

**Pass criteria:** text Find navigates matches in the current filtered
conversation, room filters work by shortcut and click, and reopening Find
resets its query while preserving each room's filter choices.
