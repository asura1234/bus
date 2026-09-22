# Repository skill rules

`skills/` is the canonical source for Bus workflows. Each skill lives in its own
directory and must have a `SKILL.md` with `name` and `description` frontmatter.
The top-level [skill-architecture.md](skill-architecture.md) defines the shared
workflow layering, artifact lifecycle, and agent communication conventions.

- Follow `skill-architecture.md` when deciding whether a rule belongs in the
  executable entrypoint, a guide, a deterministic helper, or a format contract.
- Keep skill-local helpers in `scripts/`, local principles in `guide.md`, and
  single-skill formats in `references/`. Put shared guides/formats in
  `docs/guides/`, shared templates in `docs/templates/`, and shared Python in
  `cli_extensions/`.
- Write workflows for this Rust repository. Use Cargo and the `justfile`; never
  import LibTV Desktop's `./run`, pnpm, Electron, submodule, or `build/deps`
  assumptions.
- Treat `origin` as the Bus fork and `upstream` as the Herdr source. A workflow
  must not push to `upstream` or open an upstream PR unless the user explicitly
  requests it and repository policy permits it.
- The Bus fork intentionally has no root `AGENTS.md` or root `CLAUDE.md`.
- The Bus integration base is `origin/master`. Never infer a base from a
  remote's symbolic HEAD.
- Preserve unrelated and pre-existing worktree changes. Stage explicit paths,
  never `git add .` or `git add -A`.
- Use lowercase Conventional Commit subjects accepted by
  `scripts/conventional_commits.py`.
- Use `just test-one <filter>` for focused Rust iteration and `just ci` for the
  full pre-PR gate. `gate-and-fix` preflights `just` and `cargo-nextest`; when
  either is unavailable, its Bus adapter expands `just ci` into the
  corresponding direct Cargo, Python, and Bun gates and records every exact
  command in the round artifact.
- Keep skill entrypoints under 250 lines and fail closed around destructive or
  publishing operations. `execute-plan` preserves the canonical evidence/state
  machinery but adapts Desktop-specific gates to `just lint`, `just test`, and
  optional `just build`.
- Workflows that consume Goal and Non-goals inside a Bus room follow
  [orchestrated-room-brief.md](../docs/guides/orchestrated-room-brief.md)
  through `cli_extensions/room_assignment_context.py`; only
  `skills/pr/scripts/pr_goal_context.py` produces review Goal/Non-goals locks.
  Orchestrator-only skills live in the embedded production content bundle,
  never under `skills/`.

Discovery links are derived views: `.agents/skills` serves Codex and Cursor,
and `.claude/skills` serves Claude. Edit only the canonical copy here.
