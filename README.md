# Bus

# developer and agents equal participants in work

Start Bus with `bus --yolo` (or `./run dev --yolo`) to bypass provider
permission prompts for every worker and MASTER orchestrator launched during
that run. Resume with `bus resume --last --yolo` or
`./run dev resume --last --yolo` to apply it to resumed agents too.
It is off by default and is never saved: a later run without `--yolo` uses
normal permissions. For this run, bypass overrides Codex's conflicting approval
options; the original `--args` remain saved for later normal runs.
See [agent launch arguments](docs/launch-args.md) for provider flags and details.

Bus also [watches each work room’s GitHub repositories](docs/github-watch.md) for
merges and owned-PR review, CI, and conflict updates. It uses your existing `gh`
authentication; `bus room github ROOM` selects automatic discovery, a repo list, or off.

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
