# Sticky room-history filters QA

## Run contract and setup recipes

- Use a fresh isolated fake-provider scratch Bus at 100×32. Never connect to live Bus or use a live agent project. Open every evidence screenshot. No real provider usage.
- Setup may use scratch CLI room creation and fake Claude agent creation in the driver's own work directory. Use the UI's hook-consent choice for this isolated project if prompted, then F6 to return to history. Create `sticky` and `other`, and fake workers `alpha` and `beta` in `sticky`.
- CLI sends are allowed as deterministic new-message fixtures: agent-authored direct posts to you, agent peer messages, and developer-authored messages to alpha. Let fake native turns settle as `ended` with `reply:null` and no failure reason before judging a state; no native final becomes a chat reply.
- Drive every filter toggle, room switch and restart with TUI keys/clicks or the driver restart verb. Read-only scratch state/history may corroborate but does not replace visible evidence.

## Cases

### STICKY-01 — Filters survive arrivals, refresh, room changes and restart

**Preconditions/setup:** In `sticky`, arrange `initial direct` from alpha to you, `initial peer` from alpha to beta, and `initial outgoing` from the developer to alpha. Require each recipient request to end with no stored reply. Open `sticky` at the tail.

1. Click `filter-to-you` → incoming checked, outgoing unchecked; `initial direct` visible, `initial peer` and the developer's `initial outgoing` body hidden. Capture **incoming-on**.
2. Without touching the filter, send `new peer hidden` as alpha to beta and `new direct visible` as alpha to you → after native completions/refresh settle, incoming remains checked, the new direct post is visible, and the peer body is hidden. Capture **incoming-after-arrivals**.
3. Resize to 90×32 and back to 100×32, switch to `other` and back, then restart the scratch Bus → incoming remains checked in `sticky` and the same filtering holds. `other` has both unchecked. Capture **incoming-restored** after restart.
4. Click `filter-to-you` to uncheck it → both unchecked and new peer traffic visible in history. Capture **all-after-uncheck**. Scroll if needed to inspect the earlier peer message.
5. Press Alt+O, then send `outgoing visible` as the developer to alpha, `outgoing peer hidden` as alpha to beta, and `outgoing direct hidden` as alpha to you → outgoing remains checked, incoming unchecked; only developer-authored message bodies remain. Capture **outgoing-after-arrivals**.
6. Press Alt+O to uncheck it → both unchecked; direct, peer and developer-authored history are all restored. Capture **all-restored**; scroll as needed to inspect each message body.

**Evidence:** opened screenshots for every named state, snapshots, named filter hits, scratch history and trace.

**Pass criteria:** both filters remain enabled across matching/nonmatching arrivals; incoming survives background layout, room switches and restart; unchecking restores every direction.
