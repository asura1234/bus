# Find selection across filtered lazy pagination

Train tip for #365. One scratch session. This case locks the Find contract while older filtered pages load.

## Run contract and setup recipes

- Fresh isolated fake-provider scratch Bus at 100×32. Never connect to live Bus, never use `target/debug/bus`, never start a real provider. Open every evidence screenshot. Graphics may stay the driver default.
- Setup may use scratch CLI only: `bus room create`, `bus agent add --consent-hooks` with the session work directory as PWD, and `bus send`. Drive the filter toggle and every Find query through the TUI. Read-only `bus history` may corroborate that all 270 directs settled; it does not replace the screen.
- Header checks read `[x] * → You` and `[ ] You → *`. The incoming hit is `filter-to-you`. Find is Ctrl+F. A selected Find match uses background `#f0be46`.
- The 270 direct posts are `stage:posted` with `requests:[]`; no provider turn or reply is required. Wait for their durable receipts before judging match counts. Do not press Up or Down except where a step says so.
- Numbered steps are action, then expected result. A mismatch is FAIL. Do not skip.

## Cases

### FIND-01 — Find stays on the visible newest match while older matches load

**Preconditions/setup:** CLI-create room `finds` and fake Claude worker `alpha` in `finds`. Post 270 directs from alpha to you, in order, `TOK-000` through `TOK-269` (three digits). Wait until all 270 have settled. Open `finds` at the tail through the UI. Click `filter-to-you` and leave it checked.

1. At the tail, before any scroll, assert `TOK-269` is visible and `TOK-000` is not. Press Ctrl+F and type `TOK-` → the selected match (`#f0be46`) contains `TOK-269`. The incoming check stays checked. Capture **find-newest**.
2. Do not press Up, Down, or edit the query. Wait until the Find counter is `270/270` → the selected match still contains `TOK-269` and stays in the viewport, `TOK-000` stays off screen, and `[x] * → You` remains. Capture **find-held**.
3. Press Esc, then Ctrl+F, and type `TOK-000` → exactly that oldest direct is the selected match and is in the viewport. `TOK-269` need not be visible. The incoming check stays checked. Capture **find-oldest**.

**Evidence:** opened screenshots, snapshots, cells for `#f0be46` on the named token, scratch history of all 270 directs.

**Pass criteria:** opening Find on a shared prefix selects the newest visible match and keeps that selection while older matches load; the oldest match stays off screen until a new query asks for it, and that query selects it.
