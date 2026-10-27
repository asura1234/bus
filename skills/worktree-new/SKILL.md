---
name: worktree-new
description: Create and provision an isolated Git checkout using a cached repository profile. Detect locked package managers, workspaces, submodules and native toolchains; report ready only after a build and headless smoke pass. Argument is the initial branch; optional path overrides the sibling directory name.
---

# Worktree new

Read [guide.md](guide.md) and [repo-profile-format.md](references/repo-profile-format.md)
completely before provisioning. Resolve script paths relative to this installed skill,
not the repository being provisioned.

```text
INPUT = <branch> [--path <destination>] [--base <integration-ref>]
        --build-slot <authorized-absolute-slot> [--tool-bin <pinned-tool-bin>]...

IF branch is absent
  STOP "Provide an initial feature branch."

repo = git rev-parse --show-toplevel
IF repo is absent
  STOP "Run inside a Git repository."

Read the repository's instructions and the caller's worktree/disk/build authority.
IF the build slot or worktree creation authority is missing
  STOP with the missing resource; never guess another worker's slot.

Run:
  python3 <skill-dir>/scripts/repo_profile.py scan --repo <repo>

Read the returned profile, including every unresolved item and hazard.
The canonical cache path is defined by repo-profile-format.md; other skills
call this same scanner to refresh it before consuming policy or recipes.

IF the repo needs an explicit recipe
  Use the documented .bus/repo.toml override for a build plus a headless run/test.
  Commit repository configuration before provisioning; do not fabricate a GUI target.

Set caller-required build environment (for example the authorized CARGO_TARGET_DIR).
Honor resource limits from the caller; the runner also limits Cargo jobs and niceness.
Run:
  python3 <skill-dir>/scripts/worktree_new.py create --repo <repo> \
    --branch <branch> --build-slot <authorized-absolute-slot> \
    [--path <destination>] [--base <integration-ref>] \
    [--tool-bin <pinned-tool-bin>]...

The runner resolves the main checkout, fetches origin's symbolic default branch
(or the explicit base), preserves existing branches, and creates new branches
with --no-track. An existing registered checkout is reconciled without resetting it.
It initializes pinned submodules when policy permits, rescans their manifests,
checks pins and disk admission, provisions dependencies, and runs smoke.
It never copies ignored files, secrets, build outputs, or provider memory stores.

IF the command fails
  Report NOT READY, the failing step and the retained worktree/evidence path.
  Fix the actual cause; do not delete the checkout or weaken the smoke recipe.
  For an occupied slot, wait for its owner to release it; never remove its marker.
  For a fresh-only failure, follow the profile's gitlink diagnostic before editing.
  After a committed repair, run:
    python3 <skill-dir>/scripts/worktree_new.py provision --repo <worktree> \
      --build-slot <authorized-absolute-slot> [--tool-bin <pinned-tool-bin>]...
ELSE
  Return the runner's Worktree ready path, exact head and evidence path.
  Include the disk estimate and any hazards requiring action in later workflows.
  GUI/simulator acceptance is separate and optional; do not imply it ran.
```
