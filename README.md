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

Installing skills is a separate add-on, not part of the `bus` binary.
`./run dev`, `./run prod`, `bus`, and `bus resume` never ask about or install
skills. `./run install-skills` copies this checkout's `skills/` tree, and
`./run remove-skills` deletes only the copies that add-on recorded. Add `--yes`
to skip the prompt. Neither command starts Bus or reads the Bus data directory.
Each prints one line per provider and exits non-zero if any provider failed:
`installed N skills to <path>`, `removed N skills from <path>`, `failed: <reason>`,
or `skipped: <reason>`.

Skills are copied into each provider's user skill directory, as
`<directory>/<name>/SKILL.md`:

- Claude: `~/.claude/skills`
- Codex: `~/.agents/skills`
  ([Codex discovery](https://learn.chatgpt.com/docs/build-skills#where-codex-loads-local-skills))
- Cursor: `~/.cursor/skills`
  ([Cursor skills](https://cursor.com/help/customization/skills))
- Grok: `~/.grok/skills`
  ([Grok skills](https://docs.x.ai/build/features/skills-plugins-marketplaces))
- Kimi Code: `~/.kimi-code/skills`, which is `$KIMI_CODE_HOME/skills` when that
  variable is unset
  ([Kimi Code skills](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/skills.html))

Cursor, Grok, and Kimi also read `~/.agents/skills`. The add-on still writes each
provider's own directory. It prints to its own terminal and does not post to Bus
rooms. Ownership and SHA-256 hashes live in `~/.config/bus-skills/manifest.json`.
Upgrades replace matching owned copies; existing user skills and edited copies
stay in place, and remove deletes a skill only when its bytes still match that
manifest. Shared scripts and guides live in the owned `_bus` directory inside
each skills directory. The add-on reads `skills/` from this checkout.

The canonical sources remain in `skills/`. This repo has no `.claude/skills` or
`.agents/skills` registry. Bus does not scan or post about same-name repo and
user skills.
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
