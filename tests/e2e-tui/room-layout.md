# Room layout after removing the status header

## Run contract

Run in a new scratch Bus with private HOME/data and fake provider launchers. Use 120 columns by 40 rows initially, graphics none. Never access the live Bus. All creation and navigation is through the TUI. Read-only scratch state and hit regions corroborate layout. Each visual assertion requires a cell screenshot that the runner opens and judges. No agents or real models are needed. Record commit and binary identity before starting. Coordinates below are zero-based terminal rows.

### ROOM-LAYOUT-01 — Work room and MASTER share the full conversation area

**Preconditions/setup:** a fresh scratch Bus; no workers required.

1. Press Ctrl+Shift+R, type `qa-layout`, and press Enter → the work room opens with `# qa-layout` on row 1, a single divider on row 2 and the conversation beginning on row 3. No editable status box or F3 hint appears. Capture and judge a screenshot; record composer hit geometry.
2. Click MASTER → `# MASTER` occupies row 1, with the same divider, conversation and composer rows as qa-layout. Capture and judge a screenshot; compare geometry with step 1.
3. Click qa-layout and press Ctrl+F → the Find panel begins on row 3 directly below the room divider; its full controls fit above the history area and the composer remains in its original position. Type `needle` → the query is visible with no matches. Capture and judge a screenshot and query hit geometry.
4. Press Esc, click MASTER and press Ctrl+F, then type `needle` → the same Find panel starts on row 3 with the same controls, history boundary and composer rows as qa-layout. Capture and judge a screenshot and compare query geometry with step 3.
5. Press Esc, resize to 80×24, click qa-layout and press Ctrl+F → the Find panel still starts on row 3, with visible history beneath and composer controls at the bottom. Capture and judge a screenshot. Press Esc, click MASTER and press Ctrl+F → identical vertical structure at 80×24. Capture and judge a second screenshot.
6. Press Esc, click qa-layout, press F3, then type `draft stays here` → text goes into the composer and no extra editor opens. Capture text and a screenshot. Read scratch state → rooms contain no `notes` field. Run scratch `room --help` → no retired subcommand is offered.

**Evidence:** opened screenshots for both room kinds at normal and compact sizes, snapshots, composer/search hits and read-only state/help.

**Pass criteria:** every work-room header/history/search geometry matches MASTER, the reclaimed rows belong to the conversation/search, and the removed editor/command/state field do not return.
