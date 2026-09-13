---
name: pr
description: Prepare, validate, push, and create or update a focused Bus pull request against an explicit target repository and base. Use when asked to open, create, update, or prepare a PR.
---

# Pull request

Read [references/pr-template.md](references/pr-template.md).

Input may include `--base <ref>`, `--plan <path>`, and
`--delete-dead-code`. Default base is `origin/main` only when it resolves.

1. Resolve repository, current feature branch, immutable base SHA, push remote,
   and target GitHub repository. Refuse detached HEAD and a branch equal to the
   base branch.
2. Determine the authenticated GitHub account. If the target is
   `herdrdev/herdr`, enforce `CONTRIBUTING.md`: verify maintainer or
   `.github/APPROVED_CONTRIBUTORS` status and any required prior alignment. Do
   not treat access to a fork as authority to open an upstream PR.
3. Inspect all status/diffs and preserve unrelated work. If authorized changes
   are uncommitted, use `commit-and-push`; otherwise do not manufacture a commit.
4. Use `rebase-origin-main` only when the selected base is exactly
   `origin/main`. For another explicit base, apply the same safe rebase rules
   without pushing to `upstream`.
5. Recompute `<base>...HEAD`. Ensure it solves one accepted problem, contains no
   secret or generated noise, and includes focused regression proof.
6. Run `delete-dead-code` only when explicitly requested. Run `update-docs` only
   when behavior changed or the user asked. Do not broaden the minimal README.
7. Run `gate-and-fix`; preflight `just` and `cargo nextest`. Before opening or
   updating the PR, `just ci` must pass or the exact tooling/test blocker must be
   reported. Never equate a `cargo test` fallback with the full gate or hide a
   failing check.
8. Push the feature branch through `commit-and-push` using an explicit refspec.
9. Draft the title and body from current diff and validated behavior. Title must
   be lowercase Conventional Commit style, such as `fix: handle pane focus`.
   Use `refs #N`, not closing keywords, when referencing Herdr issues.
10. Create or update the PR with `gh`. Keep it draft while material gates or
    decisions remain. Do not post review replies or request reviewers unless
    asked.
11. Verify the resulting PR URL, base/head, title, body, and remote checks.

Plans constrain goal and non-goals but do not override current code evidence.
Report the PR URL and any outstanding risk or unrun validation.
