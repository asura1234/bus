# Developer notification system sound

## Run contract

Fresh scratch Bus from the own-worktree dev-tools driver, 160x55, graphics none.
Use fake providers and the driver's recorded-effects hooks only. Never play audio,
send real Feishu, automate the desktop, or use the live Bus. A closed PATH recording
lark-cli shim supplies Feishu. CLI fixtures may create rooms and a fake worker, mute
room sounds, and post agent/developer messages; UI settings use clicks and keys only.
Read-only state, saved settings, installed sound paths and effect records corroborate
results. Quiet windows use wait --stable 3s --rows 0-0 --timeout 8s. No skips or retries.
Capture cell screenshots and open them before accepting the report.

### DSOUND-01 — Choose a system sound without disabling developer alerts

Preconditions: fake sound-worker in sound-work, another room sound-other; every room
sound off. Installed macOS Glass and Ping sounds, with the effect recorder intercepting
all playback. Setup settled and fake worker idle.

1. Open Settings and find Notify you sound (always on), initially System default. Confirm Feishu remains always on and room sounds remain off. Capture the default settings row.
2. Click the sound's next arrow, use Right and Left to select sounds, then choose Glass through the UI. Assert each selection previews its installed system file through recorded effects and saves developer_sound in settings. Capture the selected Glass row.
3. Close Settings, select sound-other, type KEEP DRAFT and send focus out. Fixture: sound-worker posts a [waiting-on-you] --to you notice in sound-work. Assert exactly one new recorded Glass.aiff effect despite muted rooms and focus out, no terminal popup, no replay, and unchanged visible draft.
4. Open Settings, use Enter on the sound row to change its choice, and then select Ping through the UI. Assert the row stays always on and settings save Ping. Close Settings, send a tagged developer notice from sound-worker, and assert exactly one new Ping.aiff effect while room sound switches remain off. Capture the selected Ping row.
5. Read saved settings and confirm Ping persists and Feishu is still on. Fixture: send a developer-authored waiting tag to sound-worker with fake:raw-reply done. Assert no additional sound through the quiet window. Check every recorded developer sound or preview targets an installed system file, with no bus-developer-alert MP3 target; preserve the effect ledger and stop the scratch session.
