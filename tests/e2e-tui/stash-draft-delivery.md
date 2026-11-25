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

Use the integrated copy/send/restore delivery lane in
[APP-25](app.md#app-25--new-interrupt-delivers-immediately-and-preserves-native-input).
Capture automatic delivery following the last edit. Do not require a fixed
quiet period or resend to clear a queue. The readable-draft checks use exact
bytes. Opaque chips cannot be reconstructed from a visible label alone.

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
The native pane shows the fake provider's usual `fake reply:` output.
Every incoming completion below requires its exact request to
be `complete:true`, `stage:replied` with its native reply text, `reason:null`,
with a trusted start, and its matching native output exactly once. The stored
reply is the native final and appears under its original developer message.

Record PASS/FAIL for every step; no skips or failed-step retries. Capture the
native composer before sending, while a message waits, and after restoration.
For typing during the transfer itself, retain the deterministic runtime test
evidence as well. Timestamp keys relative to native delivery and restoration
so the check distinguishes the edit burst from the copy/send/restore operation.

### STASH-01 — Claude native stash preserves an image and a collapsed paste

Preconditions: all setup bootstrap requests are complete with `stage:replied`,
a non-empty reply, `reason:null` and trusted starts; each matching native output was observed. Prepare the driver's
fixture image path in the scratch workspace. Open `stash-claude`.

1. With driver `paste`, enter one draft containing the quoted absolute fixture
   image path on its own line, then `fake:draft-bytes RICH`, followed by twelve
   lines `rich line 01` through `rich line 12`. Do not press Enter. Assert the
   composer contains one `[Image #N]` and one `[Pasted text #M +K lines]` chip.
   Save the complete fixture text and the visible chip labels.
2. Fixture: send `STASH native delivery` to `stash-claude` while its pane stays
   open. Wait for that exact request to meet the native-completion checkpoint
   above and observe the incoming message's matching output in the native pane. Assert the original image and paste chip labels are back in the
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
2. Fixture: send `STASH codex delivery`. Wait for that request's native-completion checkpoint, then assert the original plaintext draft is back in the composer.
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
2. Fixture: send `STASH cursor delivery`. Wait for that request's native-completion checkpoint. Assert the original plaintext draft is back, with no end marker or
   occupied-input failure notice.
3. Type `END` and press Enter. Decode the `fake draft bytes:` JSON response and
   assert it equals D followed by `END` byte for byte. Assert the incoming Bus
   message is absent from this draft payload.

### STASH-04 — Opaque Codex and Cursor input does not hold delivery

Preconditions: repeat for idle Codex and Cursor with distinct message markers.
This case exercises the copy fallback when only an opaque paste chip is visible.

1. Paste `fake:draft-bytes OPAQUE PROVIDER` followed by twelve newline-separated
   fixture lines. Record bytes and the real provider chip in the native pane.
2. Send `STASH opaque delivery PROVIDER` through the scratch CLI → the incoming
   request reaches the pane and completes once. No persistent `composer_busy`
   hold or room failure notice. Check the provider input log for accidental
   inclusion or submission of the fixture text and retain any
   `bus.draft.copy_loss` diagnostic.
3. Inspect the restored composer → if the chip survives through a supported
   native preservation path, submit it manually and compare the exact bytes.
   If the fallback cannot recover the opaque bytes, record the loss with its
   diagnostic. Delivery of the incoming message and exact restoration of
   opaque data are separate observations.

### STASH-05 — Text typed around delivery survives the transfer

Preconditions: repeat for idle Claude, Codex and Cursor. For deterministic
typing while sending, use a scratch fixture that can pause native delivery at
the transfer boundary and record when it releases. Keep provider-specific
end markers for all payloads.

1. Type `fake:draft-bytes ACTIVE PROVIDER`, then append `-1` as the incoming
   send is issued. With a controlled pause, append `-2` while the transfer
   owns the composer → the fixture records where both inputs landed.
2. Release delivery; wait for the exact same incoming request to complete.
   Append `-END` to the restored draft, press Enter and decode the native
   JSON reply → it contains the original draft plus `-1-2-END` exactly,
   without delivery text or an end marker. The incoming request was accepted
   once and reaches the native-completion checkpoint.

### STASH-06 — Claude defers while both the composer and stash contain drafts

Preconditions: open idle `stash-claude`.

1. Type `fake:draft-bytes HIDDEN`, press Ctrl+S, then type
   `fake:draft-bytes VISIBLE`. Capture the visible draft and `› stashed`.
2. Send `STASH full native slot`. Verify the request stays queued with
   `agent_composer_busy`, the visible draft and stash marker stay in place,
   and the fixture receives no Bus input during this attempt.
3. Manually submit VISIBLE and verify its exact bytes in the decoded reply.
   If HIDDEN is visible after Enter, submit it directly. If the composer is
   empty and the stash marker remains, verify HIDDEN when Claude promotes it
   or after manually restoring it. Decode the reply and confirm both drafts
   were submitted exactly once with their original bytes.
4. Follow the original incoming request through retry and confirm it submits
   once when Claude can accept it. Stop scratch Bus.
