# Disk space alerts

Settings has **Disk warning (GB free)** (default 20) and **Disk critical
(GB free)** (default 5). Values are whole GB, from 1 to 10,000, with critical
strictly below warning. The arrow keys and each row's ‹ / › buttons change
the value by one. GB means 1,000,000,000 bytes; messages show whole GB rounded
down, while crossings compare the full byte count.

The coordinator checks the volume containing its Bus data directory at
startup and every 60 seconds. The filesystem query runs on a background
thread; the coordinator only starts it and takes completed results, with at
most one query in flight. A failed query leaves the alert state unchanged
and retries on the next interval.

Each work room receives a grey **bus** notice addressed to its orchestrator,
or to the developer when it has no available orchestrator. MASTER and rooms
being deleted do not receive disk notices. Warning and critical each fire
once below their own threshold. A direct drop below both levels posts only
the critical notice and marks both levels as sent. Each level rearms only
when capacity is more than 1 GB above its threshold: exactly 21 GB for the
default warning or 6 GB for critical leaves that level latched. The notices
and per-room latches are saved together, including rearming, so restarting
Bus does not repeat an alert.

The threshold settings are shared like the compaction limit. The next
completed measurement uses the current saved settings.

Tests inject a free-space source and a monotonic sampling clock. Scratch
sessions created by `./run tui start` use synthetic capacity instead of the
real filesystem: the dev build recognizes the driver's `BUS_TUI_OWNER`
marker and reads the byte count from `disk-free-bytes` in that scratch Bus's
data directory. An absent file means healthy capacity. For example, write
`4000000000` to that file to trigger only the critical notice at the next
60-second sample. This source is compiled only with `dev-tools`; ordinary
Bus sessions use the operating system's available-space query.
