# Claude Argon

Bus launches `claude-argon` through the Claude Code harness. The launch command
and saved session namespace stay specific to Argon. Claude Code handles the
composer, permissions, callbacks, and resume.

## Prerequisites

- An installed Claude Code CLI and an executable named `claude-argon` on the
  PATH inherited by Bus.
- A Magpie gateway listening on `http://127.0.0.1:3425` with the approved
  `tgw-argon` provider.
- Local credentials in `~/.config/claude-arg/credentials.json`. The private
  directory uses mode 0700 and the file uses mode 0600.

## Gateway and wrapper

Claude Code speaks the Anthropic Messages API to Magpie, which sends OpenAI Chat
Completions requests to the approved TGW upstream. Set up the `tgw-argon` entry
in `~/.config/magpie/providers.json` with the TGW chat base URL, required
`gateway_context`, and `gemini-4-argon` as the sole model. Keep the file mode
0600. Build the upstream header as `Authorization: AK <access_key>:<secret_key_once>`
using the two fields from the local credential file. Keep all credential values
out of source, launch arguments, and logs.

The wrapper selects `tgw-argon/gemini-4-argon`. Set `CLAUDE_ARGON_BASE_URL` for a
different local Magpie listener. The wrapper forwards Claude Code arguments,
including Bus-supplied `--settings` and `--resume`. Once the gateway is
configured, verify it before launching an agent.

```sh
claude-argon -p 'Reply exactly ARGON_OK'
```

The expected reply is `ARGON_OK`.

## Launch in Bus

Choose **Claude Argon** in the Add Agent form. The option appears when Bus can
find the executable on PATH. You can also add a worker from the CLI.

```sh
bus agent add --room '<room>' --name argon --provider claude-argon --pwd '<project>'
```

Use the same wrapper configuration when resuming a saved Argon session.
