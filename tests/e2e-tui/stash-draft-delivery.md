# Deliver into an agent pane without losing its draft

## Run contract

Use a fresh scratch Bus through this worktree's dev-tools TUI driver, 160x45,
graphics none. Use fake providers only. Unset inherited Bus/HERDR variables and
keep this worktree's single Cargo target. Never attach to the developer's Bus.

Create room `stash-qa` and agents `stash-claude`, `stash-codex`, and
`stash-cursor`, using the session's scratch workspace and hook consent. The
scratch CLI may create these fixtures, bootstrap each provider with `STASH
bootstrap PROVIDER`, and send the incoming message named in a step. All draft
creation, editing, submission, and pane selection must use driver input. Open a
pane with `agent:NAME` and confirm its native composer before typing; these
cases do not edit the Bus room composer. CLI state, message status, history,
and agent reads are read-only corroboration. Retain every send receipt and
inspect its one request, rather than treating CLI exit zero as delivery.

The quiet window is three seconds since the last TUI key, text, or paste to
that pane. `composer_busy` is a queued wait, with no failure notice. Allow that
window when waiting for the incoming message to settle. Never resend it.

The fake CLIs preserve real draft data behind a large-paste chip. A bracketed
paste containing at least ten newline characters collapses; `type` creates
plain text, including its LF and tab bytes. Claude lifts a quoted image path
on its own line. Codex and Cursor open the complete draft with Ctrl+G and the
managed VISUAL editor; Codex trims trailing whitespace on editor readback,
while Cursor removes one final newline. The delivery exchange uses a temporary
end marker and verifies the complete draft before removing that marker.

`fake:draft-bytes` is a narrow acceptance fixture: when the developer submits
such a draft directly in the native pane, its fake reply is `fake draft bytes:`
followed by a JSON string of the exact submitted prompt, before whitespace
trimming. Compare the decoded JSON string, including every space, tab, and LF.
The ordinary incoming messages use their usual `fake reply:` response.

Record PASS/FAIL for every step; no skips or failed-step retries. Capture the
native composer before sending, while a message waits, and after restoration.
For typing during the transfer itself, retain the deterministic runtime test
evidence as well: the UI steps below prove the quiet-window behavior and the
end cursor after restoration without depending on a scheduler race.

### STASH-01 — Claude native stash preserves an image and a collapsed paste

Preconditions: all setup bootstrap messages have replied. Prepare the driver's
fixture image path in the scratch workspace. Open `stash-claude`.

1. With driver `paste`, enter one draft containing the quoted absolute fixture
   image path on its own line, then `fake:draft-bytes RICH`, followed by twelve
   lines `rich line 01` through `rich line 12`. Do not press Enter. Assert the
   composer contains one `[Image #N]` and one `[Pasted text #M +K lines]` chip.
   Save the complete fixture text and the visible chip labels.
2. Fixture: send `STASH native delivery` to `stash-claude` while its pane stays
   open. Wait for that exact request to reply with the incoming message's fake
   reply. Assert the original image and paste chip labels are back in the
   composer, and history contains no occupied-input failure notice.
3. Type `-END` and press Enter. Read the native fake reply and decode its JSON
   string. Assert it contains the original image chip and every original rich
   text line in order, followed by `-END` at the end. The incoming Bus message
   must be absent from this draft payload. Assert no end-marker text remains.

### STASH-02 — Codex plaintext round trip is exact and leaves the cursor at the end

Preconditions: open idle `stash-codex`. Define D as the decoded JSON string
`"  fake:draft-bytes CODEX\t中 文\nsecond  line\n\n  tail\t  \n"`.

1. Use driver `type` to enter D, without submitting it. Assert the native
   composer is occupied and is not an image or pasted-block chip. Record D's
   exact bytes; its leading spaces, tabs, blank line, and trailing LF matter.
2. Fixture: send `STASH codex delivery`. Wait for that request's ordinary fake
   reply, then assert the original plaintext draft is back in the composer.
   Assert no `__BUS_DRAFT_END_` marker or failure notice is present.
3. Type `END` and press Enter. Decode the `fake draft bytes:` JSON response and
   assert it equals D followed by `END` byte for byte. This proves both exact
   whitespace restoration and the end cursor. The Bus payload is absent.

### STASH-03 — Cursor plaintext round trip is exact and leaves the cursor at the end

Preconditions: open idle `stash-cursor`. Define D as the decoded JSON string
`"  fake:draft-bytes CURSOR\t中 文\nsecond  line\n\n  tail\t  \n"`.

1. Use driver `type` to enter D without submitting it. Assert the native
   composer is occupied and contains no image or pasted-block chip. Record the
   exact fixture bytes, including its trailing whitespace and newline.
2. Fixture: send `STASH cursor delivery`. Wait for that request's ordinary fake
   reply. Assert the original plaintext draft is back, with no end marker or
   occupied-input failure notice.
3. Type `END` and press Enter. Decode the `fake draft bytes:` JSON response and
   assert it equals D followed by `END` byte for byte. Assert the incoming Bus
   message is absent from this draft payload.

### STASH-04 — Opaque Codex and Cursor drafts wait without a failure notice

Preconditions: run each step once for `stash-codex` and once for
`stash-cursor`, with the corresponding pane open and idle. Use unique incoming
message text for each provider.

1. With driver `paste`, enter `fake:draft-bytes OPAQUE PROVIDER` followed by
   twelve newline-separated lines `opaque line 01` through `opaque line 12`.
   Assert Codex shows `[Pasted Content N chars]` and Cursor shows a
   `[Pasted text #N +K lines]` chip. Record the raw draft and chip label.
2. Fixture: send `STASH opaque delivery PROVIDER`. After the quiet window,
   inspect the request twice across another three seconds: it stays queued
   with `composer_busy`, the chip is unchanged, and neither history nor the
   native pane contains a submission failure notice. Bus has not cleared or
   submitted the developer's opaque draft.
3. Press Enter to submit the developer's opaque draft. Decode its native
   `fake draft bytes:` reply and compare it with the complete raw fixture.
   Then wait for the original queued incoming message to reply automatically.
   Assert it occurs exactly once, has no occupied-input failure notice, and
   leaves an empty composer. No resend is permitted.

### STASH-05 — Every provider waits through active typing and restores the full draft

Preconditions: repeat all steps for Claude, Codex, and Cursor with their pane
open and idle. Use `PROVIDER` in both fixtures to keep receipts distinct.

1. Type `fake:draft-bytes ACTIVE PROVIDER`. Fixture: send `STASH typing
   delivery PROVIDER`, then continue native typing: append `-1`, append `-2`
   one second later, and append `-3` one second after that. After each append,
   inspect request status and screen: the request remains queued and the draft
   contains every typed suffix. No stash, clear, submit, or failure notice has
   interrupted the typing burst.
2. Stop typing. Wait for the same incoming request to reply automatically.
   Assert the original draft plus `-1-2-3` is restored, without Bus message text
   or an end marker. Type `-END` and press Enter; decode the native JSON reply
   and assert the exact payload is `fake:draft-bytes ACTIVE PROVIDER-1-2-3-END`.
   There is one incoming message and one incoming reply, without a resend.

### STASH-06 — Claude keeps an existing native stash while another draft is visible

Preconditions: open idle `stash-claude`.

1. Type `fake:draft-bytes HIDDEN`, press Ctrl+S, and type
   `fake:draft-bytes VISIBLE`. Assert the visible draft is present alongside
   the native `› stashed` footer marker.
2. Fixture: send `STASH full native slot`. Across two quiet windows, assert the
   incoming message stays queued with `composer_busy`, the visible draft and
   stash marker remain, and no failure notice appears. Bus must not overwrite
   the already occupied native stash slot.
3. Press Ctrl+U to clear only the visible single-line fixture, then Ctrl+S to
   restore the hidden fixture. Assert it is `fake:draft-bytes HIDDEN`, press
   Enter, and decode its native reply to prove the hidden draft survived.
   Wait for the original incoming message to reply once automatically; assert
   the composer is empty and no end marker or failure notice remains. Stop
   the scratch session after recording status and cleanup evidence.
