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
repo = git rev-parse --show-toplevel
main = first worktree in `git worktree list --porcelain` for repo
room_id = numeric room ID from bus state (resolve a name uniquely)
marker = <main>/temp/.bus/sitrep-<room_id>.json

Read marker FIRST, before gathering task or PR facts. Run:
  python3 "$SKILL_DIR/scripts/marker.py" read --main "<main>" --room "<room_id>"
Capture `since`, `cutoff`, `initial`, `items`, and `revision` from the JSON.
If it fails, stop and report the error; never silently reset the marker. Serialize
sitrep runs for a given room. The initial flag means the last 24 hours.

Run `bus state`, identify the room and read its `notes` field. `bus room notes`
is a write command in current Bus: never invoke it to read. Read recent
`bus history --room <room_id> --limit 100`; paginate with --before when needed
to cover since, or record that the history window is incomplete. Read only the
orchestrator workflow files explicitly named by room notes. Record unavailable
sources; do not write room notes or workflow files.

Inspect Git state and the current branch read-only. Determine the host from
`git remote get-url origin`. On GitHub, query open and merged PRs authored by
the authenticated user in this repo using `gh pr list --repo <owner/name> --state
open --author @me --limit 1000` and the equivalent `--state merged`, requesting
number, title, url, state, headRefName and mergedAt where available. Resolve the
repository from origin, never guess it. Limit merged entries to since < mergedAt
<= cutoff; if either query reaches its limit, disclose possible truncation. Never fetch,
checkout, alter Git config, or create/update PRs. On Codeup, use the shared
PR host supplied by the installed pr skill if it supports read-only listing;
inspect its help first. If unavailable or listing is unsupported, skip and note
it in the report. For other hosts or any failed query, note the limitation.

Reconcile current room notes, workflow, recent history, Git and host evidence.
Use the guide's evidence and status rules. Include completed work only when an
evidenced completion or merge occurred after since and no later than cutoff.
Do not infer a completion time from an undated note. Keep task names, PR titles,
URLs and branches grounded in their sources. For a missing marker, say the
finished window is the last 24 hours in an appropriate Context field. If no
other task can carry a required source or window note, include the current status
report task in Ongoing with status `in-development` and that note in Context.

Prepare the format's JSON object in memory and pipe it to:
  python3 "$SKILL_DIR/scripts/render.py"
If rendering fails, correct the input and rerun; send the renderer's output
verbatim in chat. Only after producing the report, pipe a JSON object containing
`timestamp`: cutoff, `items`: stable identifiers of all reported tasks/PRs, and
`revision`: the read result's revision to:
  python3 "$SKILL_DIR/scripts/marker.py" write --main "<main>" --room "<room_id>"
If it fails, report the failure and do not claim the marker was saved.
```
