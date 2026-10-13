# Bus

# developer and agents equal participants in work

Bus is a fork of [Herdr](https://github.com/herdrdev/herdr) and inherits its
[Apache License 2.0](LICENSE).

Credit to the Herdr contributors. Existing third-party licenses and notices are retained.

Each work room has a hidden, managed Claude permission handler. It handles every
blocked episode first and sends orchestrators concise escalations, choices and
fallbacks. Inspect its role, assignments, latest decisions and worker frozen
`launch_cwd`/private `scratch_dir`/effective `launch_profile` through `bus state`;
room history keeps decision lines. New Codex workers receive scoped
workspace-write/on-request settings; overlapping live Bus paths use read-only.
Claude worker sandbox settings remain additive with regular approvals, without
a promise of exact isolation. Legacy unknown roots escalate; relaunch to obtain
new profiles. See [the CLI guide](orchestration/how-to-bus-cli.md#worker-launch-profiles-and-migration).
