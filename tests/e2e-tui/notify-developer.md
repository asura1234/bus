# Developer message notifications

## Run contract

Use only fresh scratch Bus sessions built with dev-tools at 120x40, graphics none. No desktop automation, real terminal window, live Bus, real providers or developer data. Use the driver for all UI actions. CLI fixtures may create rooms and fake orchestrators, and post `send --room master --as "libtv-app master" --to human` messages. Read-only state, settings, effects and pty.raw corroborate results. Every assertion is required; no SKIP allowance. Do not retry failed steps.

Create the fixture orchestrator with `agent add --room master --name "libtv-app master" --provider claude --pwd <scratch work dir> --orchestrates bus --consent-hooks`. The existing bus work room must be visible when a case says another room. Wait for hook setup/idle before testing. Preserve a composer draft `KEEP DRAFT` in the bus work room. A quiet-window assertion uses `wait --stable 3s --rows 0-0 --timeout 8s`, then effects and raw output covering the entire interval; the notification coalescing window is 2 seconds.

### DEV-01 — default-on notification from another room

Precondition: fresh scratch session and fixture above; bus room visible.

1. Open Settings by its hit. Assert Developer message notifications and Freeze recovery notifications are separate and both checked by default. Capture and inspect a cell screenshot. Close Settings and ensure KEEP DRAFT remains in the composer.
2. The scratch orchestrator posts `Need Figma access\nPRIVATE body must stay hidden` with --to human. Wait for a terminal_notification effect. Assert exactly one effect and exactly one complete OSC in pty.raw with target `libtv-app master: Need Figma access`, no body, no PRIVATE text, no system notification/osascript effect. Verify MASTER unread count increments and KEEP DRAFT remains unchanged, with no notification text in the composer.

### DEV-02 — a burst becomes one notification

Precondition: fresh scratch session and fixture above; bus room visible.

1. The orchestrator posts two --to human messages, First update and Second update, less than 2 seconds apart. Wait for a terminal_notification effect; assert exactly one effect and one raw OSC with target `libtv-app master and 1 more`. Wait through a quiet window and assert no second notification. Verify MASTER unread count reflects both messages and KEEP DRAFT stays in the composer.

### DEV-03 — viewing MASTER, focus and independent toggle

Precondition: fresh scratch session and fixture above.

1. Click MASTER, send focus in, post `Already viewing` with --to human, then wait through a quiet window. Assert zero developer terminal notifications in effects and raw output. Keep the existing sound behavior (a recorded sound is allowed).
2. Send focus out while MASTER stays visible; post `While unfocused` with --to human. Wait for exactly one terminal notification and raw OSC, with target `libtv-app master: While unfocused`. Send focus in again.
3. Open Settings, click developer-notifications, and assert its unchecked row and persisted developer_notifications false while freeze_notifications remains true. Capture and inspect a cell screenshot. Close Settings and click bus. Post `Disabled alert` with --to human, wait through a quiet window, and assert counts remain exactly one, KEEP DRAFT unchanged and no notification text in the composer.
