# Bus

Bus is an app that lets agent CLIs delegate, orchestrate, communicate and collaborate with each other and with the developer. Bus lets an average developer keep many (10+) agents working concurrently toward the same set of goals.

Start Bus with `bus --yolo` (or `./run dev --yolo`) to bypass provider
permission prompts for every worker and MASTER orchestrator launched during
that run. Resume with `bus resume --last --yolo` or
`./run dev resume --last --yolo` to apply it to resumed agents too.
It is off by default and is never saved: a later run without `--yolo` uses
normal permissions. For this run, bypass overrides Codex's conflicting approval
options; the original `--args` remain saved for later normal runs.
See [agent launch arguments](docs/launch-args.md) for provider flags and details.

Installing skills is a separate command. `./run dev`, `./run prod`, `bus`, and
`bus resume` never ask about or install skills. Use `./run install-skills` to
install this checkout's default skills, or add `--yes` to install without a
prompt. `bus skills install --yes` installs the bundle embedded in that binary;
`bus skills status` inspects copies without a running session. The install
report's action for each skill is installed, updated, unchanged, or
preserved-conflict, with a warning when a user skill is preserved.
`./run install-skills` runs only a binary built from this checkout: if a shared
Cargo target returns another checkout's binary, it rebuilds into
`target/install-skills` and refuses to install when that binary still does not
match.

Skills are copied into `~/.claude/skills` and `~/.agents/skills` for Claude and
Codex. Codex's current [skill discovery documentation](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills)
identifies `$HOME/.agents/skills` as its user location. Bus records ownership,
version and SHA-256 hashes in `~/.local/share/bus/skills-manifest.json`.
Upgrades replace matching Bus-owned copies; existing user skills and edited Bus
copies are preserved with a warning. Shared scripts and guides live in the
Bus-owned `_bus` directory inside each skills directory. The binary embeds the
bundle, so it does not need the source checkout at runtime.

The canonical sources remain in `skills/`. This repo has no `.claude/skills` or
`.agents/skills` registry. Bus warns in room history when a repo skill with the
same name duplicates a user skill as an agent launches or reconnects.
[Claude Code gives personal skills precedence](https://code.claude.com/docs/en/skills#resolve-skills-that-share-a-name);
[Codex lists both same-name copies](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills).

Feishu notifications are on by default. Install `lark-cli` on PATH and configure
its bot identity. In **Settings → Feishu notifications**, choose the recipient
user open_id and optionally override the CLI path. No credentials are stored by
Bus. Use the checkbox or `bus settings feishu-notifications --off` to disable.

An orchestrator puts `[waiting_on_you]` or `[waiting-on-you]` before the action
it needs from the developer in a MASTER message. Bus sends a bot DM containing
`name is waiting on you: action`, combining arrivals within two seconds. The
existing case-insensitive whole phrases `waiting on you` and `still waiting on you`
also qualify. `[needs_developer_attention]` still renders the internal badge;
by itself it does not send a DM. Failed sends warn without blocking Bus or
retrying. See [Feishu notification settings and behavior](docs/developer-notifications.md).

Settings includes disk warning and critical thresholds (20/5 GB free by
default). Bus checks its data volume every minute and posts one grey `bus`
notice per level in each work room. See [disk space alerts](docs/disk-space-alerts.md).

Bus is a fork of [Herdr](https://github.com/herdrdev/herdr) and inherits its
[Apache License 2.0](LICENSE).

Credit to the Herdr contributors. Existing third-party licenses and notices are retained.
