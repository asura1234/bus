---
name: sitrep
description: Use when a developer asks for a sitrep, status report, or a summary of blockers and work in a Bus room.
---

# Sitrep

Resolve `SKILL_DIR` to this skill's directory. Read [guide.md](guide.md) and
[references/sitrep-format.md](references/sitrep-format.md) completely.

```text
INPUT = optional room ID or name; otherwise use the room identified by the
current Bus agent context. If the room cannot be established, ask for it.
repo = absolute path from git rev-parse --show-toplevel
room_id = numeric room ID from bus state (resolve a name uniquely)
state = a private temporary JSON file; remove it when the run ends

Read computed state FIRST, before gathering task or PR facts. Run:
  python3 "$SKILL_DIR/scripts/marker.py" read --repo "<repo>" > "$state"
If it reports no saved commit, stop and ask for an explicit starting point, or
derive one only from explicit evidence. Rerun with --start FULL_SHA; never guess
a starting point. For any other failure, stop and report it. Serialize sitrep
runs for the repository. Preserve the state JSON unchanged for rendering.

Run `bus state`, identify the room and read its `notes` field. `bus room notes`
is a write command in current Bus: never invoke it to read. Read recent
`bus history --room <room_id> --limit 100`; paginate with --before when needed.
Read only the orchestrator workflow files explicitly named by room notes.
Record unavailable sources; do not write room notes or workflow files.

Reconcile current room notes, workflow, recent history, and current Git state
with the computed state. Finished names and landed SHAs come only from the
state's ordered Git log; supply TLDR and context for exactly those entries.
Open PR links come only from the state's ordered open_prs list; supply their
task names, TLDR, context, and status without adding, dropping, or reordering
the links. Work without a PR comes from the orchestrator's input and uses the
format's branch notation. If evidence is missing, say so in Context.

Prepare the format's JSON object in memory and pipe it to:
  python3 "$SKILL_DIR/scripts/render.py" --state "$state"
If rendering fails, correct only agent-owned fields and rerun; never edit the
computed state to make a report pass. Send the renderer's output verbatim in
chat. Only after producing the report, run:
  python3 "$SKILL_DIR/scripts/marker.py" write --repo "<repo>"
If it fails, report the failure and do not claim the marker was saved. The
writer resolves the main worktree itself, including when repo is linked.
```
