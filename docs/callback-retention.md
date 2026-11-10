# Callback capture retention

Each provider launch gets a directory under the session's
`callbacks/<launch-id>/`. Its manifest records the agent ID, provider and launch
ID. Preparation can create a capture before the agent joins the roster, and
recovery or replacement can leave older launch captures behind.

The coordinator collects attributable captures after an agent's deletion is
durable. Room deletion includes its MASTER orchestrator. Deferred save batches
finish their callback acknowledgements before collecting directories. A failed
terminal close or state save retains the captures for retry.

At coordinator startup, collection runs under the data-directory lease after
state has been loaded, validated and saved. An unreadable state file prevents
collection. Collection errors are logged and retain uncertain data; they do not
reverse a committed deletion or prevent a valid session from starting.

The following captures are retained:

- Every capture whose manifest owner remains in state, including unavailable
  agents, pending deletions and older launches of an existing agent.
- Every current launch named by state, even if its manifest has a contradictory
  owner.
- Captures with missing, invalid, oversized or mismatched manifests, and
  directory or manifest symlinks.
- Kimi captures containing a `kimi-home` entry. This entry contains native
  provider conversations and session indexes. Kimi uses an absolute
  `KIMI_CODE_HOME`, and its index contains
  absolute session-directory paths. A successful Bus deletion may also leave a
  moved native session running in its terminal. Keeping the original paths
  preserves that session and later native conversation access. Uncertain or
  linked home entries are retained without following them.

Callback-only Kimi captures without a native home remain collectible. Native
homes are never moved by collection. Other providers' external conversation
stores, shared authentication links, exit snapshots and persisted message
history are outside the collected directories and remain intact.

The shared Codex hook bridge is a file in `callbacks/`, not a launch directory;
collection leaves it in place. No manual production-session sweep is required.
