# Missing agent working directory

## Run contract

Use the dev TUI driver, fake providers, a fresh scratch session per case, and a
160x45 terminal with `--graphics none`. Never drive a live Bus. Scrub inherited
`BUS_*` and `HERDR_*` variables for every driver invocation. No SKIP is allowed.
This checks log-only directory failures and the allowed Unavailable state; it
does not prove a real provider can execute a hook after its cwd disappears.

## Fixture setup

For each case, use the provider named in its title. Under that driver's scratch
work directory create an empty `launch` directory X and initialize a Git repo
there. Through `T bus`, create room `cwd-lab` and add agent `worker` with the
case's provider, `--pwd X --consent-hooks`. Wait for interactive readiness. The
CLI is allowed for this setup and read-only state inspections. Copy X to a
backup under the same scratch session before deleting it. The directory removal
and restoration in the numbered steps are authorized scratch fixtures.

### CWD-01 — Codex

1. Complete setup with `codex`, then click `room:cwd-lab`. `T bus state` shows
   worker idle, no actionable_error, and details_disclosed false.
2. Remove only X. Within 10 seconds, `T bus state` corroborates actionable_error
   and working_directory_error. The agent row shows `Unavailable` and no error
   text appears anywhere in the sidebar, including with details expanded. The
   client log names worker, its room, X and the directory failure. Take and open
   a screenshot; `Working directory is unavailable`, `Restore the worktree` and
   `--pwd` must not be drawn.
3. Restore X from its scratch backup. Within 10 seconds both directory errors
   clear from `T bus state`, and the row returns to its healthy state without
   error text or a recovery notice. Agent ID and runtime identity are unchanged. Take and open a screenshot.

### CWD-02 — Claude

1. Complete setup with `claude`, then click `room:cwd-lab`. `T bus state` shows
   worker idle, no actionable_error, and details_disclosed false.
2. Remove only X. Within 10 seconds, `T bus state` corroborates actionable_error
   and working_directory_error. The agent row shows `Unavailable` and no error
   text appears anywhere in the sidebar, including with details expanded. The
   client log names worker, its room, X and the directory failure. Take and open
   a screenshot; `Working directory is unavailable`, `Restore the worktree` and
   `--pwd` must not be drawn.
3. Restore X from its scratch backup. Within 10 seconds both directory errors
   clear from `T bus state`, and the row returns to its healthy state without
   error text or a recovery notice. Agent ID and runtime identity are unchanged. Take and open a screenshot.

### CWD-03 — Cursor

1. Complete setup with `cursor`, then click `room:cwd-lab`. `T bus state` shows
   worker idle, no actionable_error, and details_disclosed false.
2. Remove only X. Within 10 seconds, `T bus state` corroborates actionable_error
   and working_directory_error. The agent row shows `Unavailable` and no error
   text appears anywhere in the sidebar, including with details expanded. The
   client log names worker, its room, X and the directory failure. Take and open
   a screenshot; `Working directory is unavailable`, `Restore the worktree` and
   `--pwd` must not be drawn.
3. Restore X from its scratch backup. Within 10 seconds both directory errors
   clear from `T bus state`, and the row returns to its healthy state without
   error text or a recovery notice. Agent ID and runtime identity are unchanged. Take and open a screenshot.

### CWD-04 — MASTER orchestrator (Claude)

1. Create the same scratch directory and room, but add `worker` to `--room master`
   with `--provider claude --orchestrates cwd-lab --pwd X --consent-hooks`.
   Click `room:MASTER`. Worker is idle with no actionable_error. Its healthy
   row remains compact and the orchestrated room line opens `cwd-lab`.
2. Remove only X. Within 10 seconds `T bus state` corroborates both directory
   errors. The MASTER agent row shows `Unavailable`; no error or recovery
   instruction is drawn anywhere in the sidebar. The client log names worker,
   its room, X and the directory failure. Take and open a screenshot: neither
   `Working directory is unavailable`, `Restore the worktree` nor `--pwd` is
   visible.
3. Restore X from its scratch backup. Within 10 seconds both directory errors
   clear and the MASTER row returns to its compact layout. Agent ID and runtime
   identity remain unchanged. Take and open a screenshot.
