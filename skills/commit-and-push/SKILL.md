---
name: commit-and-push
description: Split current Bus changes into focused Conventional Commits and push them with an explicit refspec. Use when asked to commit, push, publish a branch, or commit-and-push.
---

# Commit and push

Commit only the changes the user authorized, without absorbing unrelated work.

1. Resolve the repository root and current branch. Refuse detached HEAD.
2. Read `git status --short`, unstaged diff, staged diff, and recent subjects.
3. Determine ownership. Preserve pre-existing or unrelated changes; if an
   overlapping file cannot be separated safely, stop and explain the conflict.
4. Group owned changes so every commit has one purpose. Tests that prove a
   change normally belong with that change.
5. Stage explicit file paths. Never use `git add .` or `git add -A`.
6. Review `git diff --cached --check` and `git diff --cached` before committing.
7. Use a lowercase Conventional Commit subject accepted by:

   ```sh
   python3 scripts/conventional_commits.py --message-file <message-file>
   ```

   Allowed types are the repository validator's types, including `feat`, `fix`,
   `test`, `docs`, `refactor`, and `chore`. Add a body when the reason or issue
   reference matters.
8. Commit without bypassing hooks. Re-read status and repeat for another
   independent group.
9. Fetch the push remote. Default to `origin`; never push to `upstream` unless
   the user explicitly requests that exact remote.
10. Push with an explicit same-name refspec:

    ```sh
    git push origin <branch>:<branch>
    ```

Main-branch policy: do not commit on `main` by default. If the user explicitly
asked to commit or push directly to `main`, that instruction authorizes the
normal commit and `git push origin main:main`; still run all ownership and
validation checks. Never force-push `main`.

Report commit hashes, subjects, pushed ref, validation run, and any changes left
untouched.
