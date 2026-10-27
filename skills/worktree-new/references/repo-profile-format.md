# Repository profile format

The UTF-8 JSON object at `<main worktree>/temp/.bus/repo-profile.json` is the
shared repository profile. `repo_profile.py scan --repo PATH` is its producer.
Consumers call the scanner and consume its returned JSON (or verify the hash before reading the file): linked worktrees can have different
manifests, and the cache holds the last scanned checkout. This is a cache, never
proof of provisioning or readiness. There is no legacy format.

Read-only consumers may use `scan --no-cache --repo PATH`: it returns the same
schema with freshly inferred values, never reads or writes the cache, and never
creates `temp/`. This keeps repositories without a `temp/` ignore rule clean.

The `policy --repo PATH` command returns exactly `{ "policy": <policy object> }`
using the same defaults and validation of policy-relevant TOML fields. It reads
no manifests, inventory, hazards or cache, and makes no repository/Git metadata
writes. Unrelated worktree overrides do not block this policy-only read.

## Schema 1

All listed fields are required; no additional fields are allowed. JSON object
key order is insignificant. Lists are deterministic, paths are slash-separated,
relative to the scanned repository (root is `.`), without `..`. SHA values are
lowercase 64-character SHA-256, except Git commits (Git's native object ID).
An empty collection is `[]`; an unavailable scalar is `null`.

| Field | Value |
| --- | --- |
| `schema_version` | integer `1` |
| `main_worktree` | absolute main checkout path |
| `manifest_hash` | SHA-256 of canonical JSON `manifests` plus submodule identities |
| `manifests` | sorted `{path, sha256}` objects; includes locks, toolchain pins, build entry points and override file; gitlinks are hashed through submodules |
| `package_managers` | sorted `{path, manager, version, lockfile, workspaces}`; manager is `npm`, `pnpm`, `yarn`, or `bun`; version/lockfile nullable; workspaces is a string list |
| `submodules` | sorted `{path, url, commit, initialized}`; commit nullable; initialized boolean |
| `toolchains` | sorted `{kind, path, pin}`; kind `rust`, `node`, `cmake`, `xcode`, `gradle`, or `android-ndk`; pin nullable |
| `entry_points` | sorted `{kind, path, commands}`; kind `cargo`, `package`, `cmake`, `make`, `just`, `run`, `xcode`, or `gradle`; commands string list |
| `policy` | `{gate_command, commit_style, require_exact_head_gate, submodule_update}`; first two nullable strings, require_exact_head_gate boolean (default false), update `recursive` or `none` |
| `recipe` | object below |
| `hazards` | sorted `{id, paths, reason, action}`; IDs `shared-deps`, `fresh-gitlink`, `submodule-pins`, `node-abi`, `disk-floor`, `rerere`, `worktree-budget`, `nested-target`; paths string list; reason/action nonempty strings |

`recipe` has `prepare` and `smoke` command lists, `unresolved` (string list),
`shared_dependencies` (relative directory list), `target_paths` (relative directory
list), `disk_estimate_gib` (positive number), and `disk_estimate_basis` (string).
A command is exactly `{cwd, argv}`; cwd is a relative directory and argv is a
nonempty list of nonempty strings, executed directly, without a shell. An explicit
override may use `sh -c` when a shell is intentional. Every smoke recipe must
build and execute a non-GUI run or test target. An unresolved recipe cannot run.
The disk estimate is conservative planning input, not measured allocation.

## Optional checked-in override

`.bus/repo.toml` accepts only these optional keys; unknown keys, wrong types,
escaping paths and malformed commands fail scanning. Overrides participate in
the manifest hash. Empty smoke overrides are invalid.

```toml
gate_command = "just ci"
commit_style = "conventional"
require_exact_head_gate = true

[submodules]
update = "recursive" # or "none"; no fetching branch tips or copying ignored files

[worktree]
disk_estimate_gib = 8
prepare = [{cwd = ".", argv = ["npm", "ci"]}]
smoke = [
  {cwd = ".", argv = ["npm", "run", "build"]},
  {cwd = ".", argv = ["npm", "run", "test:ci"]},
]
shared_dependencies = ["node_modules"]
```

Shared dependencies are opt-in, limited to `node_modules` paths. The source
must exist in the main checkout and Node manifests and locks must match. Only
that package's dependency-install command is skipped; smoke still runs. Arbitrary
`build/deps` sharing is refused because runtime receipts can be writable.

## Complete example

Hashes below represent illustrative fixture contents, not a hash to copy.

```json
{
  "schema_version": 1,
  "main_worktree": "/work/example",
  "manifest_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "manifests": [],
  "package_managers": [],
  "submodules": [],
  "toolchains": [],
  "entry_points": [],
  "policy": {"gate_command": null, "commit_style": null, "require_exact_head_gate": false, "submodule_update": "recursive"},
  "recipe": {
    "prepare": [], "smoke": [], "unresolved": ["No headless build and test recipe inferred; configure worktree.smoke"],
    "shared_dependencies": [], "target_paths": [],
    "disk_estimate_gib": 15, "disk_estimate_basis": "15 GiB planning allowance; native Rust targets add 4 GiB each"
  },
  "hazards": [
    {"id": "disk-floor", "paths": [], "reason": "Builds and apps can fail before logging when disk is full.", "action": "Check free space before every provisioning command; require at least 25 GiB."},
    {"id": "worktree-budget", "paths": [], "reason": "Each checkout owns build outputs and consumes the disk budget.", "action": "Budget the estimate against free space and existing worktree count; reuse an authorized lane for replacement work."}
  ]
}
```

A profile with a relative `main_worktree`, unknown enum, empty argv, or a
`../other` cwd is malformed. A profile with no smoke is valid discovery output
only when unresolved explains why; it never means ready.

## Readiness evidence

Provision writes `<worktree>/temp/.bus/worktree-ready.json` with exactly
`schema_version` (`1`), `head` (Git commit), `manifest_hash`, `status`
(`ready` or `failed`), `commands` (executed command objects), `free_gib`
(number at admission), and `error` (null on ready, string on failure).
An old ready file is removed before provisioning. Ready is written only after
every command exits zero and HEAD, manifests, and tracked files remain unchanged.
Consumers recheck those identities; the file alone does not prove current readiness.
