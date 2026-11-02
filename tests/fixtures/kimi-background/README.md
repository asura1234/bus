# Kimi 2.4.x foreground/background captures

Captured on 2026-11-02 from Bus `62614986` running Kimi 2.4.1.

- `idle-tip.txt`: reduced to the final task notice, residual moon tip and empty
  composer of kimi-dispatch (agent 96398), reported Working with no current
  request and 13 queued sends. Devices master reported that its only activity
  was a native background Bash watch loop containing `sleep 30`.
- `foreground-queued.txt`: reduced control rows of cu-owner (agent 97513), whose
  foreground command was reported Idle while request 98749 awaited native start.
- `autonomous-turn-events.json`: lifecycle fields only from that agent's bound
  private main wire. Owned turn 8 completed, then native autonomous turns 9-11
  lacked `promptId`; turn 11 was open. Internal iteration IDs can also differ
  from the outer ID (captured autonomous turn 3 ended at agent iteration 4).

Full private evidence and the red-on-base receipt remain in
`temp/kimi-bg-working/rca.md`. No credentials, OAuth data or tool payloads are
included in these lifecycle fixtures.
