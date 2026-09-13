# Repository skill rules

`skills/` is the canonical source for Bus workflows. Each skill lives in its own
directory and must have a `SKILL.md` with `name` and `description` frontmatter.
The top-level [skill-architecture.md](skill-architecture.md) defines the shared
workflow layering, artifact lifecycle, and agent communication conventions.

- Follow `skill-architecture.md` when deciding whether a rule belongs in the
  executable entrypoint, a guide, a deterministic helper, or a format contract.
- Keep a skill self-contained. Put deterministic helpers in `scripts/`, durable
  explanation in `guide.md`, and output contracts in `references/` only when
  they materially improve execution.
- Write workflows for this Rust repository. Use Cargo and the `justfile`; never
  import LibTV Desktop's `./run`, pnpm, Electron, submodule, or `build/deps`
  assumptions.
- Treat `origin` as the Bus fork and `upstream` as the Herdr source. A workflow
  must not push to `upstream` or open an upstream PR unless the user explicitly
  requests it and repository policy permits it.
- `CONTRIBUTING.md` is inherited from Herdr. Its approved-contributor rule and
  reference to a root `AGENTS.md` apply when submitting to `herdrdev/herdr`, not
  to ordinary work in `asura1234/bus`. The Bus fork intentionally has no root
  `AGENTS.md` or root `CLAUDE.md`.
- Prefer `origin/main` as the Bus integration base only when it resolves. Never
  infer a base from a remote's symbolic HEAD.
- Preserve unrelated and pre-existing worktree changes. Stage explicit paths,
  never `git add .` or `git add -A`.
- Use lowercase Conventional Commit subjects accepted by
  `scripts/conventional_commits.py`.
- Use `just test-one <filter>` for focused Rust iteration and `just ci` for the
  full pre-PR gate. Preflight `just` and `cargo nextest`; if unavailable, run
  direct focused Cargo/Python/Bun checks where possible and report that the full
  PR gate remains blocked rather than claiming equivalent coverage.
- Keep skill entrypoints concise and fail closed around destructive or
  publishing operations.

Discovery links are derived views: `.agents/skills` serves Codex and Cursor,
and `.claude/skills` serves Claude. Edit only the canonical copy here.
