# Find messages addressed to the developer

## Run contract

Use a fresh scratch Bus through this worktree's dev-tools `bus --dev tui` driver,
120x40, graphics none. Build through the room's slot wrapper and use only the
worktree's target directory. Never attach to the live Bus, use desktop automation,
or launch real providers. CLI fixtures may create rooms and fake agents and send
agent-authored messages; search, navigation and help use real TUI keys. No skips
or failed-step retries. Capture snapshots for each step and cells for highlights;
open a cell screenshot to judge the bar's layout.

### FIND-YOU-01 — Find one developer message buried in peer traffic

Setup: CLI fixtures create `find-busy` and two fake Claude agents `find-sender`
and `find-peer`, both using the driver's scratch work directory. Wait for idle.
Send 12 messages `Needle peer before NN` from find-sender to find-peer, one
`Needle lost developer ask` from find-sender to you, then 12 messages
`Needle peer after NN` from find-sender to find-peer. Wait for every peer
request to settle before judging the screen. Select `find-busy` through the UI.

1. Assert the latest peer traffic is visible and `Needle lost developer ask` is
   outside the viewport. Press Ctrl+F. Assert Find is open, `Tab → You only: off`
   is visible, and an empty query has no match counter.
2. Press Tab then Enter. Assert `→ You only: on`, counter `1/1`, the
   `find-sender → You` header and `Needle lost developer ask` are visible.
   Assert the header has selected-match background `#f0be46` using cells.
   Capture and open a cell screenshot: the filter label, query field and
   navigation hint are readable and do not overlap.
3. Type `lost`. Assert `1/1` and the selected-match background on `lost` in
   `Needle lost developer ask`. Press Up and Down; both wrap to this same
   message. Append `x` and assert `no matches`; Backspace restores `1/1`.
4. Press Esc, then Ctrl+F. Assert `Tab → You only: off`, an empty query and no
   counter. Type `Needle`; assert multiple matches across peer and developer
   messages. Press Tab and assert `1/1` on the developer ask. History read-only
   evidence still contains all 24 peer prompts and the one developer post.
5. Press Esc. Open help by typing `/help` and pressing Enter. Assert Ctrl+F
   remains the chat find key and `Tab in Find` documents `→ You only` and empty
   query behavior. Close help and stop the scratch session.
