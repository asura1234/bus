# Freeze recovery alerts

## Run contract

Use only a scratch Bus built with dev-tools, 120x40 cells, graphics none. No real terminal window or desktop automation. Settings contains a dev-only 3.5-second stall action. Read-only scratch state, perf files, and pty.raw are permitted corroboration. Notifications are judged from terminal_notification effects and the exact output stream. Do not retry failures. All assertions are required; no SKIP allowance.

### FRZ-01 — recovery timing and rate limit

Precondition: new scratch Bus. Open Settings by its settings hit.

1. Assert the Freeze recovery notifications row is on by default. Take and inspect a cell screenshot, then click freeze-test. While perf records the ongoing client.ui_loop stall, assert the output/effects contain no freeze notification.
2. Wait for recovery and a terminal_notification effect. Assert exactly one effect and exactly one complete OSC notification in pty.raw, matching Bus froze for N.N s at HH:MM; details: bus perf. Confirm a matching ended stall record and that the composer contains no alert text.
3. Trigger freeze-test again within one minute, wait for the second ended stall record, and assert notification effect and raw OSC counts remain exactly one. Confirm run_start, event backlog and machine sample fields exist in scratch perf records.

### FRZ-02 — Settings disables and persists alerts

Precondition: new scratch Bus. Open Settings by its settings hit.

1. Click freeze-notifications; assert the unchecked row and scratch settings show freeze_notifications false. Capture and inspect a cell screenshot.
2. Trigger freeze-test, wait for an ended client stall longer than 3 seconds, and assert zero freeze terminal notifications in effects and pty.raw. Confirm the composer contains no alert text.
