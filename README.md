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

Bus is a fork of [Herdr](https://github.com/herdrdev/herdr) and inherits its
[Apache License 2.0](LICENSE).

Credit to the Herdr contributors. Existing third-party licenses and notices are retained.
