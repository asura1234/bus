# skills — Bus project skill SOT

## Directory responsibilities

`skills/` is the single source of truth for project-level AI skill content. Each top-level skill directory directly contains `SKILL.md` (with `name` and `description` frontmatter); optional scripts, references, and assets are maintained alongside it.

Judgment principles and formats shared across skills live in `docs/guides/`, shared templates in `docs/templates/`, and shared mechanical implementations in the repo-root `cli_extensions/`. Skill directories do not hold generated copies, nested skills, or placeholder structures.

## Content boundaries

- Skill names use kebab-case and match the directory name.
- `SKILL.md` keeps only the executable flow; judgment principles, strict formats, and mechanical work go into the guide, format documents, and scripts respectively (see [skill-architecture.md](skill-architecture.md)). Skill-local helpers live in `scripts/`, local principles in `guide.md`, and single-skill formats in `references/`.
- Frontmatter uses only portable fields; do not write agent-private invocation, model, context, or tool authorization.
- Engineering capabilities are invoked through the repo's Cargo and `justfile` entrypoints; do not write low-level diagnostic commands as everyday entrypoints. Never import LibTV's `./run`, pnpm, Electron, submodule, or `build/deps` assumptions.
- Project skills do not download external skills, do not operate on user-level agent directories, and do not perform implicit Git writes. Preserve unrelated and pre-existing worktree changes; stage explicit paths, never `git add .` or `git add -A`.
- Treat `origin` as the Bus fork and `upstream` as the Herdr source. A workflow must not push to `upstream` or open an upstream PR unless the user explicitly requests it and repository policy permits it.
- The Bus fork intentionally has no root `AGENTS.md` or root `CLAUDE.md`.
- The Bus integration base is `origin/master`. Never infer a base from a remote's symbolic HEAD.
- Use lowercase Conventional Commit subjects accepted by `scripts/conventional_commits.py`.
- Use `just test-one <filter>` for focused Rust iteration and `just ci` for the full pre-PR gate. `gate-and-fix` preflights `just` and `cargo-nextest`; when either is unavailable, its Bus adapter expands `just ci` into the corresponding direct Cargo and Python gates and records every exact command in the round artifact.
- Keep skill entrypoints under 250 lines and fail closed around destructive or publishing operations.
- Only `skills/pr/scripts/pr_goal_context.py` produces review Goal/Non-goals locks.

## Installation and verification

Discovery links are derived views: `.agents/skills/<skill>` (Codex and Cursor) and `.claude/skills/<skill>` (Claude) are directory symlinks to `../../skills/<skill>`; derived directories must not maintain content directly. Edit only the canonical copy here.

`workflow-create` is the exception: it has no discovery links, because `src/bus/orchestrator.rs` embeds its `SKILL.md` into the Bus binary and writes it to `<BUS_DATA_DIR>/docs/workflow-create.md` for MASTER orchestrators. Editing it changes what Bus ships.

The final available set in a session is determined only by the injected `Available skills`.

Installation state itself is **mechanically checkable**: `scripts/test_skill_migration_contract.py` (run by `just maintenance-test`) verifies the canonical inventory, entrypoint length, and the registry symlinks, and fails when anything is off. When the injected `Available skills` is clearly smaller than the enabled set (for example only one or two remain), run it first to get the on-disk facts before judging whether a skill is really unavailable — the injected list answers "can this session use it", that test answers "is it installed in the repo"; the two are not the same question, and taking an anomaly in the former as a conclusion about the latter leads to misjudgments like "the repo is missing a skill".

## Current skills

Each skill's full trigger conditions and usage are defined by the frontmatter `description` of its `SKILL.md`; the table below only locates them.

| Skill | Role |
|-------|------|
| `delete-dead-code` | Dead-code and duplication tracks clean up PR-touched modules or explicit directories; delete or converge safely after mechanical scope/artifact validation |
| `gate-and-fix` | Run the applicable Bus lint, test, and diff gates in parallel, fix from complete failure evidence, and commit and push round by round |
| `commit-and-push` | Split commits by "each commit does one thing" and push |
| `rebase-origin-main` | Rebase onto the latest `origin/master`, triage conflicts by tier, then force-with-lease push |
| `worktree-new` / `worktree-close` | Create and clean up isolated worktrees |
| `pr` | PR entrypoint for the current feature branch: land changes → rebase → publish a Draft → converge gates via `gate-and-fix` and sync docs → refresh the final body |
| `merge-pr` | Watch an open PR's CI and review comments until mergeable; attribute red checks, then squash / admin merge |
| `split-pr` | Split a multi-purpose PR into single-purpose branches, publish them as a parallel / stacked / mixed PR graph by dependencies proven by git and builds, and report each part's size; restack when a parent changes or lands |
| `best-of-n` | Converge conflicting technical proposals from N agents into a ranked verdict |
| `review-plan` | Iterative read-only review of a plan (Round 1 full, Round 2+ CLOSED WORLD) |
| `review-pr` | Iterative review of the committed diff excluding plan documents; behavior suspicions may be proven by writing a failing test, other dimensions are read-only |
| `address-review-comments` | Author-side disposition of the review artifacts above: mechanical sanitization, root-cause dedup, per-item verification, then APPLY / REJECT / FLAG / HOUSEKEEPING |
| `update-docs` | Recursively audit and update the `AGENTS.md` files affected by Git change leaves (including `CLAUDE.md` symlinks); no commit, no push |
| `workflow-create` | Draft or revise a Bus room's `workflow.md` with the human; shipped inside Bus as an orchestrator doc, not linked for repo agents |

The review skills (`review-plan` / `review-pr` / `address-review-comments`) share `docs/guides/review-format.md`, `cli_extensions/review_artifact*.py`, and `cli_extensions/review_round_common.py`: the agent writes the artifact per the format, a script validates it fail-closed, and a script then deterministically renders it into the chat response; the lane ownership, round claiming, and triage ledger discovery of `/review-plan` and `/review-pr` are implemented once, in `review_round_common.py`. When changing this chain, verify these skills together.
