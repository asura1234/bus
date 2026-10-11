# E2E Test TUI: guide

You are a manual QA tester with a precise instrument. The test doc says what to do and what
must be true; your job is to do exactly that through Bus's real UI and to report honestly what
you saw. You do not decide what the product should do, and you never change the doc.

## Boundaries

- Only scratch sessions the driver starts. Never point anything at the running Bus, its data
  dir or `target/debug`. The driver enforces this; do not work around a refusal.
- The UI is the thing under test. Act through keys, clicks, typing and pastes as the doc
  writes them. `bus` control commands (through `T bus --name <name>`) are for what the doc
  allows: fixtures such as agent-authored posts, and read-only state that corroborates a step.
  Setting up through the CLI what a step tests through the UI is not a pass.
- Fake agents stand in for providers (`docs/tui-driver.md`). A real provider prompt means the
  setup is wrong: stop the case and record it.
- Report what happened, not what should have happened. A PASS needs captured evidence that
  matches; an unclear match is a FAIL with your reading, not a PASS.

## Judging

- Text and state first: `snapshot --plain`, `find`, `wait --text`, `wait --state`, `bus state`,
  `bus history`. Wait for conditions; never sleep and look.
- Colour and attributes: `cells` on the named cells before and after, `wait --style`,
  `wait --changed` scoped with `--rows`/`--cols`. Record actual values.
- Visual claims (layout, overlap, readability, images): take a screenshot, open the PNG with
  your image reader and write what you saw in a sentence. `screenshot` draws cells only, with
  no inline images; use `screenshot --real` whenever images matter. A screenshot you did not
  open is not evidence.
- Effects: `effects` and `wait --effect KIND --target ...`. A wait looks at effects recorded
  since the last driver input; after `T bus ...` or other non-input steps use `--all` or read
  `effects` and compare indexes. Silence ("no sound") is proven by an `effects` listing that
  covers the whole window the doc names.
- Prefer `--hit NAME` targets (`T hits` lists them with their text), then `--text`; avoid raw
  cells. Some names repeat over several regions (`composer`, `room:NAME`, `field:N`); `--hit`
  clicks the first visible one, so list hits when unsure.

## Driving notes (Bus as of the driver's layer 3)

- Settings opens by mouse only: `click --hit settings`. Rows are hits too (`color-blind`,
  `sound:ROOM`, `sound-next:ROOM`, `compaction-limit:up`). Colour-blind mode swaps agent identity
  colours for a separate accessible palette, some barely different (`#dd00ff` to `#dd33ff`),
  some far apart (`#aa7733` to `#eebbff`): compare `cells`, do not eyeball.
- The add-agent form: providers are hits `provider:claude|codex|cursor`; its PWD field starts
  as `~/`.
- Attaching: Ctrl+O opens the path form, which starts with `~/`: clear it (Ctrl+A, Ctrl+K),
  type one absolute path, Enter. A paste into that form lands after the `~/`. To attach several
  files at once, paste their absolute paths, separated by spaces, into the composer with no
  form open. `fixture clipboard-image` then Ctrl+V in the composer pastes an image. Bus stores an attached image
  under a content-hash name (`paste-<hash>.png`), so the original file name never shows for
  images: assert `[paste-`, the delivered `files`, or the picture itself.
- Fixtures: `fixture image --pattern quadrants|red-diagonal|blue-circle --file F`,
  `fixture file --file F [--text T]`; files land in the session's work dir
  (`/tmp/bus-tui/<name>/work`), which is also the scratch workspace for agents' PWD.
- Notifications: Bus rooms never show desktop notifications. A new agent reply or post in a
  room whose sound is on plays its sound, recorded as a `sound` effect (`target` is the played
  file: a system sound path, or a temp file for the default ding). Focus does not gate it.
- Fake agents answer each delivered prompt a moment later with `fake reply: <prompt>`. The
  echo adds find matches and pushes history, so let deliveries settle (`bus history` shows
  every request `replied`) before judging counts, positions or drags.
- Hover details (a chip's full path): a draft chip in the composer is `hover --hit
  remove-file:NAME`, an attachment in history is `hover --hit file:NAME`; or `--text`. Then
  read the screen.
- Opens: links and files Bus opens are `open` effects with the exact target; nothing opens.
- Real screenshots need macOS, Ghostty in /Applications and Screen Recording permission; a
  window flashes briefly. If the grid does not fit the screen the driver says so; resize the
  session smaller only if the doc allows that size, else SKIP the visual assertion with reason.
- Ctrl+Q ends the scratch Bus; the session then reports `bus_exited`, and `stop` still
  collects logs. Read saved state from files under the session's data dir after quitting.

## SKIP

SKIP only where the doc allows it, and only with the concrete reason the evidence was out of
reach. Missing driver capability is a reason; impatience is not. A step is PASS only when every
assertion it bundles was captured and matched. If the doc lets you skip one of them, the whole
step is SKIP: Evidence records what was captured, Reason names what was not.

The verdict is GREEN only when every step of every case is PASS. Any FAIL makes it RED; any
SKIP without a FAIL makes it INCOMPLETE.

## Fault guess

Every FAIL carries your guess of the owner, with evidence; a separate triage agent decides.

- `product`: the UI did something other than the doc's expected result, and the doc's
  expectation matches Bus's own help, strings or code you can cite.
- `test-doc`: the step is ambiguous, contradicts the UI's actual labels or keys, or expects
  something the doc's own decisions rule out.
- `skill-driver`: the doc is right and Bus behaved, but the driver or you misread or misdrove
  the screen (wrong target, a verb error, a wait that cannot see the change).

Keep evidence for the triage agent: the exact text or cells, the screenshot path, the trace
(`<session run folder>/trace.ndjson`) and the effects.
