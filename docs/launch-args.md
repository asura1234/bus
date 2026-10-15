# Agent launch arguments

The add-agent form's **Additional launch args** field and `bus agent add --args`
accept provider arguments as literal argv. Quotes group values and are removed;
Bus performs no shell expansion. Permission, sandbox, configuration, workspace,
model and unknown provider options pass through, in their original token order.
For example:

```text
codex:  --yolo -c 'model_reasoning_effort="high"' --sandbox danger-full-access --ask-for-approval never
claude: --dangerously-skip-permissions --permission-mode bypassPermissions
cursor: --yolo --force --sandbox disabled --approve-mcps
```

Bus saves the original user tokens in the launch's `launch-options.json` and
replays permission/configuration options on resume. It never invents a bypass
flag or removes an original one. A bound conversation has already consumed its
initial prompt and native session selector, so those one-time inputs are not
sent again. Options after an initial prompt are retained. An unstarted launch
retains all initial input. Unknown option arity preserves the remaining tokens
without guessing which of them are values or one-time input.
Bus separately supplies its capture arguments (`--no-daemon` for Codex and
`--settings <capture-file>` for Claude) and the saved conversation selector.
When the user supplies Codex `--no-daemon`, Bus omits its generated copy:
Codex 0.162.0 rejects the same flag appearing twice.
An explicit UUID resume selector can be normalized and reserved for ownership;
on add, native pickers, names and other selectors remain the provider's input.
Bus-generated capture and orchestration options precede a literal `--` tail,
preserving the original user token order and literal prompt.

Bus does not validate provider option names, required values or choices. Unknown
syntax passes through; when its arity is unclear, Bus stops inspecting it rather
than guessing that a value is a command or selector. Known option values and
the literal tail after `--` are never treated as Bus mode flags. The provider
remains responsible for accepting its own syntax and configurations.

Bus rejects only these evidenced launch incompatibilities, when recognized in
command/option position:

| Provider | Arguments | Why |
| --- | --- | --- |
| Codex | `exec`, `e`, `review` | These run noninteractively instead of hosting the persistent interactive terminal used for room prompts. |
| Claude | `-p`, `--print`, `--bg`, `--background` | Print mode exits; background mode returns immediately, leaving Bus without its interactive worker. |
| Claude | `--bare`, `--safe-mode` | These explicitly disable hooks required for session ownership and request completion. |
| Cursor | `-p`, `--print` | These select script/noninteractive output instead of the interactive terminal Bus drives. |
| All | A NUL byte inside a token | Operating-system process argv cannot contain NUL. Unclosed quotes or a trailing escape also cannot be tokenized. |

Evidence checked locally on 2026-10-15: `codex --help` (0.162.0) labels `exec`
and its `e` alias, plus `review`, noninteractive; `claude --help` (2.1.300)
documents print-and-exit, background-and-return, and hooks disabled by bare/safe
mode; `cursor-agent --help` (2026.10.14-0f9edde) documents print as noninteractive.
Bus's dependency is in `src/messaging/coordinator/agents.rs` (interactive
`agent.start`), `src/agents/providers/hook_json.rs` (required observation events)
and `src/messaging/coordinator/callbacks.rs` (SessionStart ownership attestation).
These are operational checks, not a permission policy. Configurations that
change hook behavior are still forwarded to the provider.

Resume also verifies Bus's stored ownership, capture manifest and hooks before
reattaching a managed worker. An unstarted Codex record containing a native
session selector cannot be restored as a new empty conversation. These checks
protect conversation identity; they do not filter the saved permission flags.
