# Recognised provider UI

`claude-extra-thought.txt` transcribes the title and three options from two
actual structured native dialog observations (2026-10-22, 14:50 and 15:11 UTC).
Its native shape id is independently confirmed by the copied server log:
`6105abde0622e6a34d10c0743d1ad884f13286da890fb7882cf0618c7fbf83a8`.

The blank spacing and final key hint are replay scaffolding, not a claim of a
new raw-screen capture. They do not determine the allowlist identity; the
normal guarded writer still checks the fresh complete digest, including hint
and current selection, before sending keys. Runtime recognition uses provider
and this exact native shape id, never a substring of terminal output or text.

Provenance is retained in `/Users/dylanliu/work/bus/temp/rca-auto-answer-allowlist/seed.md`:
the copied orchestrator transcript lines 42011 and 42276, and the copied archive
`herdr-server.log.20261022T191506.875844000Z.gz:21476`.
No live pane or provider launch was used to create this semantic fixture.

`codex-extra-thought.json` records the exact parsed choice supplied by the
orchestrator from a real `bus agent dialog 21828` observation on 2026-10-23:
the full two-line text, the three numbered options, option 1 selected, and
`hint: null`. It is a real Bus observation, not a native PTY capture. The ID and
digest are derived with the existing native hash contract; fixture transport
identity is supplied by the test rig. Its semantic shape ID is the same as
Claude's above: the hint changes the exact digest but is not part of the shape
ID. The fixture regression feeds this parsed observation through the Codex
automatic path, without parsing the Claude screen scaffold. Unknown text or
option variants fail closed.
