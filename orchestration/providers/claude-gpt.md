# Claude GPT setup

`claude-gpt` runs the Claude Code harness with GPT through Magpie's local
Anthropic-compatible gateway. Bus gives it a separate provider label,
executable name and saved-session namespace. It uses the same Claude Code
hooks, permissions and session handling as the `claude` provider.

## Start the gateway

Install the Magpie CLI and sign in to Codex with a ChatGPT subscription.
Check that `magpie accounts codex` discovers the subscription and that
`magpie models` lists the model you want to use. [Magpie downloads](https://github.com/yetone/magpie-releases/releases)

In a dedicated terminal, run the gateway in the foreground and leave it
running while Claude GPT agents are active. Press Ctrl+C to stop it.

```sh
MAGPIE_ADDR=127.0.0.1:3425 magpie serve
```

## Install the wrapper

Save this script as `$HOME/.local/bin/claude-gpt`, run
`chmod +x "$HOME/.local/bin/claude-gpt"`, and make sure the PATH Bus inherits
includes `$HOME/.local/bin`. Adjust the last line if your Claude Code CLI is
installed elsewhere.

```sh
#!/bin/sh
set -eu

model=${CLAUDE_GPT_MODEL:-codex/gpt-6.2-sol}
unset CLAUDE_CODE_OAUTH_TOKEN ANTHROPIC_CUSTOM_HEADERS ANTHROPIC_API_KEY_HELPER
unset CLAUDE_CODE_USE_BEDROCK CLAUDE_CODE_USE_VERTEX CLAUDE_CODE_USE_FOUNDRY
export ANTHROPIC_BASE_URL=http://127.0.0.1:3425
export ANTHROPIC_API_KEY=magpie ANTHROPIC_AUTH_TOKEN=magpie
export ANTHROPIC_MODEL="$model" ANTHROPIC_DEFAULT_OPUS_MODEL="$model"
export ANTHROPIC_DEFAULT_SONNET_MODEL="$model" ANTHROPIC_DEFAULT_HAIKU_MODEL="$model"
export CLAUDE_CODE_SUBAGENT_MODEL="$model"
exec "$HOME/.local/bin/claude" --model "$model" "$@"
```

Set `CLAUDE_GPT_MODEL` in the environment inherited by the agent to use
another Magpie model ID. In Bus, select **Claude GPT** in the Add Agent form
or run the following with your room name and project directory.

```sh
bus agent add --room ROOM --name gpt-worker --provider claude-gpt --pwd "$PWD"
```

## First launch

A fresh Claude configuration may ask you to choose a theme, approve the
gateway key, acknowledge the security screen and trust the project.
Claude Code may warn about both authentication variables being set. Both
carry the same local gateway value. Claude Code assumes a 200k context
window for an unfamiliar model ID.

If Claude Code cannot write to its default temporary directory, create a
private writable directory and export `CLAUDE_CODE_TMPDIR` to that path in
the wrapper. [Claude Code environment variables](https://code.claude.com/docs/en/env-vars)

Cross-client use of a subscription is governed by the plan's terms.

Bus checks the TCP listener at `http://127.0.0.1:3425` before adding,
resuming, or restoring a Claude GPT agent. It waits up to 750 ms for a TCP
connection. Magpie can pass the check while its HTTP handlers or upstream
models are busy because the check sends no HTTP request. If the listener is
unreachable, start Magpie with `magpie serve` and retry.
