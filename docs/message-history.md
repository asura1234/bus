# Permanent message history

Bus retains messages, requests, captured replies and posted notices for the life
of a session. Agent deletion retires a runtime and closes unfinished requests as
`agent_deleted`; deleting a conversation room closes its remaining cross-room
requests as `room_deleted`. Neither operation erases the audit record. Context
clear and provider compaction do not remove Bus messages.

Deleted participants render as `NAME [deleted]` in history and message status.
Historical identities cannot be recipients or resumed runtimes. Room history
accepts a deleted room's numeric ID, or its name when unambiguous. An old archive
that lacks identity tombstones remains readable with `Agent ID [deleted]` as the
fallback; Bus cannot infer erased names.

Settled requests and notices move out of `state.json` after ten minutes. Identity
tombstones go directly to `history.jsonl`, which is synced before the corresponding
state save. Startup compaction removes duplicate crash copies and an uncommitted
torn tail, preserving every logical message. It never filters by live room,
agent, or age. Empty-session cleanup also preserves sessions with audit history.

Routing and digest acknowledgement metadata are separate from messages. Completed
routing metadata can expire after seven days, with a persisted recovery floor
preventing old records from becoming new notifications. Pending notifications
retain their source. Digest acknowledgement discards its capped summary, while
the full original message/reply stays in the archive.

Permanent retention cannot have a fixed total disk limit for an unlimited number
of messages. The archive grows linearly with unique requests, notices and deleted
identities, plus retry duplicates that startup compaction removes. The active
state retains live work, recent settled records and recent/pending routing data.
Snapshots share the archive rather than cloning message bodies; the current
implementation still indexes the full archive in memory and scans it at startup.
A cold disk index would be needed to bound total RAM independently of session
length. There is deliberately no archive rotation that discards audit content.

State format 3 accepts formats 1 and 2 on upgrade. Older binaries do not understand
the new identity tombstones; do not downgrade against the upgraded data directory.

## Recovery from older versions

Earlier versions explicitly purged deleted recipients' requests, filtered archive
entries by live ownership, and expired records after seven days. The archive file
could still contain hidden records until startup compaction physically removed
them. Upgrading preserves surviving records; it cannot recreate already-erased
content.

`tools/recovery/deleted_messages.py` exports surviving raw records, embedded Bus
records in provider transcripts, receipt-linked send text, and clearly labelled
unconfirmed commands or correlated received payloads for the October 2026 incident.
It never executes a transcript command, connects to Bus, or imports recovered data.
Copy state/history, logs and relevant provider transcripts to an evidence folder
outside live/provider directories first, then run:

```sh
python3 tools/recovery/deleted_messages.py \
  --evidence /absolute/path/to/copied-evidence \
  --output /absolute/path/to/recovered
```

Each export includes its copied source and line. `summary.json` lists the current
run's files; a repeated run can leave older exports in the directory, so use that
manifest. Missing receipts, request state, delivery outcomes and dynamic shell
expansions are not invented. These exports are audit evidence, not importable
state documents.
