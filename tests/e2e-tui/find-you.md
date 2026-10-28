# Find messages to and from the developer

## Run contract

Use a fresh scratch Bus through this worktree's dev-tools `bus --dev tui` driver,
120x40, graphics none. Build through the room's slot wrapper and use only the
worktree's target directory. Never attach to the live Bus, use desktop automation,
or launch real providers. CLI fixtures may create rooms and fake agents and send
messages as the developer or agents; search, navigation and help use real TUI keys.
No skips or failed-step retries. Capture snapshots for each step and cells for
highlights; open a cell screenshot to judge the bar's layout.

### FIND-01 — Find an incoming message buried in peer traffic

Setup: CLI fixtures create `find-busy` and two fake Claude agents `find-sender`
and `find-peer`, both using the driver's scratch work directory. Wait for idle.
Send 12 messages `Needle peer before NN` from find-sender to find-peer, one
`Needle lost developer ask` from find-sender to you, then 12 messages
`Needle peer after NN` from find-sender to find-peer. Wait for every peer
request to settle before judging the screen. Select `find-busy` through the UI.

1. Assert the latest peer traffic is visible and `Needle lost developer ask` is
   outside the viewport. Press Ctrl+F. Assert `Find:`, `[ ] → You` and `[ ] You →`
   are visible, the text box is highlighted and an empty query has no counter.
2. Press Tab. Assert only `→ You` is highlighted and both toggles remain off.
   Press Space then Enter. Assert `[x] → You`, `[ ] You →`, counter `1/1`, the
   `find-sender → You` header and `Needle lost developer ask` are visible.
   Assert the header has selected-match background `#f0be46` using cells.
   Capture and open a cell screenshot: both toggles, query field and hint are
   readable and do not overlap.
3. Press Shift+Tab to focus text. Type `lost`. Assert `1/1` and selected-match
   background on `lost`. Press Up and Down; both wrap to the same message.
   Append `x` and assert `no matches`; Backspace restores `1/1`.
4. Press Esc then Ctrl+F. Assert both toggles off, empty query and no counter.
   Type `Needle`; assert multiple matches across peer and developer messages.
   Press Tab then Space; assert `1/1` on the developer ask. Read-only history
   still contains all 24 peer prompts and the one developer post.
5. Press Esc. Open help by typing `/help` and pressing Enter. Assert Ctrl+F is
   chat find, Tab/Shift+Tab moves focus, Space toggles, `You →` finds sent
   messages, both filters include either direction, and empty text steps messages.
   Close help and stop the scratch session.

### FIND-02 — Find sent messages and combine directions in a work room

Setup: CLI fixtures create `find-outgoing` and fake Claude agents `out-agent`
and `out-peer` in the scratch work directory. Wait for idle. Send `Needle peer
noise` from out-agent to out-peer, `Needle outgoing older` and `Needle outgoing
newer` as the developer to out-agent, and `Needle incoming` from out-agent to you.
Wait for every request to settle. Select `find-outgoing` through the UI.

1. Press Ctrl+F then Shift+Tab. Assert only `You →` is highlighted, no query
   cursor, and both toggles off. Press Space. Assert `[x] You →`, `[ ] → You`,
   `2/2` and the newer developer message header selected with `#f0be46`.
2. With `You →` focused, press Enter, Up, Down. Assert counters `1/2`, `2/2`,
   `1/2` respectively, scrolling to each developer header. Press Tab to focus
   text and type `outgoing ` including the space. Assert two matches and the
   query contains a space. Append `newer`; assert `1/1` and a selected text match.
3. Press Tab then Space. Assert both toggles on and `2/2`: the developer prompt
   and its agent reply both match. Press Enter and Down with `→ You` focused;
   assert `1/2` then `2/2`. Press Tab; assert only `You →` highlighted. Press Up
   and Down; assert `1/2` then `2/2`. Capture and open a screenshot proving the
   focus highlight and layout.
4. Press Tab to text. Backspace five times to restore `outgoing `; assert four
   matches. Press Tab then Space to disable `→ You`; assert two matches.
   Press Tab then Space to disable `You →`; assert four unfiltered matches.
   Press Esc then Ctrl+F; assert empty text, both toggles off, text focus and no
   match counter. Stop the scratch session.

### FIND-03 — Developer authorship in MASTER

Setup: CLI fixtures create work room `find-work` and fake Claude orchestrator
`find-orch` in MASTER, bound to find-work, using the scratch work directory.
Wait for idle. Send `Needle master outgoing` as the developer to find-orch and
`Needle master incoming` from find-orch to you in MASTER. Wait for settlement.
Select MASTER through the UI.

1. Press Ctrl+F, Tab, Tab, Space. Assert only `You →` enabled, counter `1/1`
   and the developer-authored header selected with `#f0be46`. Press Enter, Up
   and Down; each wraps to that developer message.
2. Press Tab to focus text and type `Needle`. Assert `1/1` and only the outgoing
   prompt is a text match. Press Tab then Space. Assert both toggles on and
   `3/3`: outgoing prompt, its reply, and incoming post. Press Enter and Down;
   assert `2/3` then `3/3`. Press Esc and stop the scratch session.
