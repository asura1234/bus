# Developer notification sound checkbox

## Run contract

Fresh scratch Bus from the own-worktree dev-tools driver, 160x55, graphics none.
Use fake providers and the driver's recorded-effects hooks only. Never play audio,
send real Feishu, automate the desktop, or use the live Bus. A closed PATH recording
lark-cli shim supplies Feishu. CLI fixtures may create rooms and a fake worker, mute
room sounds, and post agent/developer messages; UI settings use clicks and keys only.
Read-only state, saved settings, installed sound paths and effect records corroborate
results. Quiet windows use wait --stable 3s --rows 0-0 --timeout 8s. No skips or retries.
Capture cell screenshots and open them before accepting the report.

### DSOUND-01 — Choose, mute and restore developer-alert audio independently of Feishu

Preconditions: fake sound-worker in sound-work, another room sound-other; every room
sound off. Installed macOS Glass and Ping sounds, with the effect recorder intercepting
all playback. Setup settled and fake worker idle.

1. Open Settings and find the checked You sound row, initially System default, as the first row under Sound notifications in its YOU group, above MASTER and ROOMS. Confirm Feishu and its recipient/CLI rows remain under NOTIFICATIONS, Feishu remains always on, and room sounds remain off. Capture the default layout.
2. Click the sound's next arrow, use Right and Left to select sounds, then choose Glass through the UI. Assert each selection previews its installed system file. Confirm saved developer_sound is Glass, developer_sound_enabled is true, and bus state settings.developer_sound is {enabled: true, name: "Glass"}. Capture the selected Glass row.
3. Close Settings, select sound-other, type KEEP DRAFT and send focus out. Fixture: sound-worker posts a tagged --to you developer notice in sound-work. Assert exactly one new recorded Glass.aiff effect despite muted rooms and focus out, no terminal popup, no replay, and unchanged visible draft.
4. Open Settings and move Down once to You. Press Enter to uncheck it, confirm the selected row shows [ ] You, then close Settings. Assert the saved enabled flag is false while the Glass name is retained, bus state reports {enabled: false, name: "Glass"}, and Feishu stays on. Fixture: send a tagged developer notice from sound-worker. Assert no new sound through the quiet window but the closed-PATH lark-cli records its Feishu DM. Capture the muted row before closing Settings.
5. Reopen Settings, move Down once and press Enter to enable You again. Choose Ping with the arrows, then close Settings. Fixture: send another tagged developer notice and assert exactly one new Ping.aiff effect. Read saved settings and state to confirm enabled Ping persists and all room sounds remain off. Fixture: send a developer-authored waiting tag to sound-worker with fake:raw-reply done; assert no additional sound through the quiet window. Every preview or developer-alert sound must target an installed system file, with no bus-developer-alert MP3. Capture the enabled Ping row, preserve the ledger and stop the scratch session.

### DSOUND-02 — Keep checkbox, focus and hit targets aligned while scrolling

Preconditions: twelve work rooms named scroll-00 through scroll-11, with every room
sound off. Fresh Settings defaults. CLI fixtures may create these rooms and mute sounds.
This case resizes the scratch terminal to 160x24 after opening Settings.

1. Open Settings at 160x55 and press Down once from Color blind mode. Confirm the checked You row under YOU is selected, above MASTER and ROOMS. Capture the layout and selected-row cells.
2. Resize to 160x24. Click the You checkbox to mute it; confirm focus stays on You and Space is inert. Click the next arrow, then press Right and Left. Assert each action previews its installed sound and saves the displayed choice while You remains unchecked. Click the checkbox to enable it again. Press Down to MASTER and Enter to toggle only MASTER on; confirm the developer choice is unchanged.
3. Scroll down through the Settings list with the mouse wheel. Confirm NOTIFICATIONS still groups Feishu, recipient and CLI rows. Press Down until the final Performance overlay field, then Down again; confirm focus stops there and that field is visible. Capture the scrolled layout.
4. Press Up until Color blind mode, then Down once. Confirm the YOU heading and checked You row return into view with the row selected. Click its previous arrow and assert the new choice previews and persists without changing the checkbox. Capture the restored layout and preserve the effects before stopping the scratch session.
