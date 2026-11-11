# Bus TUI room-history direction filters QA

## Run contract and setup recipes

- Run FILTER-01 and FILTER-02 in order, each in a fresh scratch session at **80×24** with isolated data/settings. Never connect to the running Bus or read/write `~/.local/share/bus`. Use fake providers only. Record the commit and open every screenshot used as evidence.
- The room header must show `# ROOM` on one row and, directly beneath it, right-aligned `[ ] * → You` followed by `[ ] You → *`. Checked controls read `[x]`. These filters hide nonmatching history directly; Ctrl+F contains only text search and narrows the already-filtered history.
- Every numbered step is action → expected result. Capture snapshots/hits and a cell screenshot judged by the runner for each named visual state. State/history read through the scratch CLI may corroborate fixtures, not replace visible filter evidence. A captured mismatch is FAIL; unavailable required evidence is SKIP with reason.
- **R(name), create a work room:** press Ctrl+Shift+R, type the name, press Enter, and verify `# name` opens.
- **A(room, name), add a fake Claude worker:** click the room, press Ctrl+N, type the name, Tab to Agent, select Claude Code, replace PWD with the absolute scratch workspace and a trailing slash, leave additional args empty, and press Enter. Accept hook setup only for the isolated scratch project if asked. Wait for the fake terminal, then press F6.
- **P(names), select recipients:** press Ctrl+P, make exactly the named workers checked, then Esc.
- Fixed incoming messages may be arranged through the scratch CLI as `bus send --room ROOM --as AUTHOR --to RECIPIENT --text TEXT`. They are test inputs, not model output. Let fake native turns settle with `complete:true`, `stage:ended`, `reply:null` and no failure reason before judging history; no reply row is synthesized.

## Cases

### FILTER-01 — Work-room filters, search, room state and restart

**Preconditions/setup:** R(`filter-a`); A(`filter-a`, `alpha`) and A(`filter-a`, `beta`); P(`alpha`); send `Needle outgoing` as the developer and verify its ended/null completion and matching native output. Post `Needle direct` as alpha to you and `Needle peer` as alpha to beta. R(`filter-b`). Return to `filter-a`.

1. Inspect the work-room header and unfiltered history → both checkboxes are unchecked, in the required order and right-aligned below `# filter-a`; outgoing, direct and peer messages are visible. Capture the **off** screenshot and named hits `filter-to-you` / `filter-from-you`.
2. Press Alt+I → only `* → You` is checked. Only the direct message remains; the developer's outgoing prompt and ordinary peer traffic disappear. Capture the **incoming-on** screenshot.
3. With Alt+I still on, press Ctrl+F and type `Needle` → the shorter Find panel contains text search/navigation only, with no duplicate direction controls; matches are limited to the incoming history. Press Alt+O while Find remains open → both header checkboxes become checked, outgoing messages join the incoming matches, and peer traffic remains hidden. Capture the **combined-search** screenshot.
4. Press Esc, then Alt+I and Alt+O until both filters are off. Click `filter-to-you` → it becomes checked and the incoming-only history returns. Click `filter-b` in ROOMS → both of its checkboxes are unchecked and its empty history is unfiltered. Capture the **room-B-unchecked** screenshot. Click `filter-a` → its checked incoming filter and filtered history are restored. Capture the **room-A-restored** screenshot.
5. Restart the scratch Bus with the driver's `restart` verb → after startup `filter-a` still has only `* → You` checked and its incoming-only history. Switch to `filter-b` and back → B remains unchecked and A's checked state is again restored. Capture the **restart-restored** screenshot.

**Evidence:** snapshots, hits and opened cell screenshots for off, incoming-on, combined-search, room-B-unchecked, room-A-restored and restart-restored; scratch state/history before and after restart.

**Pass criteria:** filters work by shortcut and click, combine with text search, stay room-local, and survive both client and Bus restart at narrow width.

### FILTER-02 — MASTER filters and text search

**Preconditions/setup:** R(`master-target`). Click MASTER and use A to add `master-agent` (Claude Code), orchestrating `master-target`; return to MASTER with F6. P(`master-agent`); send `Master Needle outgoing` as the developer and verify its ended/null completion and matching native output. Post `Master Needle direct` as master-agent to you. Stay on MASTER.

1. Inspect MASTER at 80×24 → both filters are present, unchecked, correctly ordered and right-aligned directly below `# MASTER`; the outgoing prompt and direct post are visible, with no synthesized reply. Capture the **master-off** screenshot.
2. Press Alt+O → only `You → *` is checked and only developer-authored history remains; the direct message disappears. Capture the **master-outgoing-on** screenshot.
3. Press Alt+I so both filters are checked, open Ctrl+F and type `Master Needle` → Find searches the union of incoming and outgoing history and contains no direction toggles of its own. Press Alt+O to leave only incoming filtering → only the direct-post match remains and the developer prompt is excluded. Capture the **master-combined-search** screenshot. Press Esc and Alt+I to return to both filters off → the unfiltered MASTER history returns.

**Evidence:** snapshots, hits and opened cell screenshots for master-off, master-outgoing-on and master-combined-search; final unfiltered snapshot.

**Pass criteria:** MASTER has the same direct filters and text-search combination as work rooms at narrow width, with no duplicate Ctrl+F controls.
