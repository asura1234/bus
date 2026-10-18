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

Feishu notifications start off. Install `lark-cli`, authorize its user identity
with `lark-cli auth login --scope 'im:message.send_as_user im:message'`, then
turn on **Settings → Feishu notifications** or run
`bus settings feishu-notifications --on`. Bus verifies authentication and
resolves the authenticated user's self ID before enabling; failure leaves it
off and shows the reason. Turn it off with the checkbox or
`bus settings feishu-notifications --off`. Toggles take effect without restart.

An orchestrator starts a MASTER report with `[needs_developer_attention]`
when work waits on the developer. Bus displays a **Needs developer** badge
and forwards eligible reports and detected mechanical blockers to that user's
own Feishu only. Every message starts `[Bus]` and names the machine, session,
room and agent. There is no destination setting or Feishu reply channel.
Repeats are deduplicated for ten minutes and delivery is limited to six
attempts per minute. Enabling skips old history; ordinary restart resumes
unattempted notifications, while uncertain or failed sends are never retried.

Bus is a fork of [Herdr](https://github.com/herdrdev/herdr) and inherits its
[Apache License 2.0](LICENSE).

Credit to the Herdr contributors. Existing third-party licenses and notices are retained.
