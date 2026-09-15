# Room Orchestrator Content Interface Format

`ROOM_AGENT_CONTENT_INTERFACE_V1` is the harness-owned format for validated,
versioned room-orchestrator content. It packages content and exposes it through
typed `ReadContent` observations; it does not define tools, capabilities,
workflow transitions, or a next action.

## Selectors

The selector is a closed enum:

- `test-agent-led` is the embedded scripted-provider fixture.
- `production` is `ProductionUnavailable` until a separately reviewed bundle
  satisfying this format is present.

Paths, URLs, environment-derived selectors, and repository shadowing are not
selectors. Unknown values are `UnknownSelector`.

## Manifest

Each selector owns one fixed-tree `manifest.json` with exactly these fields:

```json
{
  "interface": "ROOM_AGENT_CONTENT_INTERFACE_V1",
  "content_version": 1,
  "compatibility": 1,
  "system": {"path": "system.md", "sha256": "<64 lowercase hex>"},
  "agent": {"path": "agent.md", "sha256": "<64 lowercase hex>"},
  "skills": [],
  "references": []
}
```

`system` and `agent` are required singleton entries. `skills` and `references`
are arrays of `{name,path,sha256}`; `[]` is their only empty representation.
Names are nonempty, at most 64 bytes, and contain only lowercase ASCII letters,
digits, or `-`. Names are unique across both arrays. An entry path is one
relative `.md` filename in the selector directory: it is never absolute,
nested, traversing, symlink-selected, or caller supplied.

Unknown manifest fields, missing or duplicate entries, invalid UTF-8, digest
mismatch, unsupported compatibility, and any manifest field purporting to
declare a tool or capability fail the build before content is exposed. Each
content file is at most 64 KiB and the sum of content bodies is at most 256 KiB.

The bundle digest is lowercase SHA-256 over the compact JSON serialization of
the validated typed manifest, preserving manifest array order and omitting no
field. Each entry digest is lowercase SHA-256 over its exact file bytes. Prompt
bodies and credentials are never included in validation errors.

## Packed bundle and reads

The build packs the validated bodies; runtime does not reopen arbitrary paths.
The packed shape contains `interface`, `content_version`, `compatibility`,
`digest`, `system`, `agent`, `skills`, `references`, and `index`. Named packed
entries contain only `name`, `sha256`, and `body`.

Layer order is deterministic: `system`, `agent`, each `skill:<name>` in manifest
order, each `reference:<name>` in manifest order, then `index`. The index contains
only interface/name/digest facts, never implicit capability grants. Adding a
valid named entry changes manifest content, not Rust branches.

`ReadContent { kind, name }` accepts only these pairings:

- `system/system`
- `agent/agent`
- `skill/<registered-name>`
- `reference/<registered-name>`
- `index/index`

An unregistered kind/name is `UnknownEntry`; it never falls back to a raw path,
artifact, workflow draft, or production selector. The intelligent Orchestrator
chooses every read and every subsequent workflow action. Loading content cannot
change its registry or grants.

## Boundary

This format contains no production wording or SOP meaning. Production content
is owned by its successor plan. The harness owns only schema validation,
packing, deterministic layering, typed-unavailable selection, and factual reads.

