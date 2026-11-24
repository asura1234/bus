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

Claude Code speaks the Anthropic Messages API to Magpie. Set up the
`tgw-argon` entry in `~/.config/magpie/providers.json` with
`http://gw-us.tokengateway.cloud/api/stream/v1` as the sole TGW base URL.
Magpie appends `/chat/completions` and uses `gemini-4-argon` as the sole
upstream model. Set `gateway_context.caller.app_name` to `ai-frontier-radar`
and `gateway_context.audit.biz_no` to `magpie-${sessionId}` so Magpie supplies
the session ID.

Keep the providers file mode 0600. The upstream request uses the single static
header `Authorization: AK <access_key>:<secret_key_once>`. Use one space
after `AK` and join the two values from the local credential file with a
colon. The header is sufficient for authentication. HMAC, timestamps, nonces,
and mTLS certificates are unnecessary. The issuer uses the fingerprint
internally to audit and revoke the key pair. Keep all credential values out
of source, launch arguments, and logs.

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

Bus checks the TCP listener before adding, resuming, or restoring an Argon
agent. Bus reads `CLAUDE_ARGON_BASE_URL` from its process environment and
defaults to `http://127.0.0.1:3425` when the variable is unset or empty. It
passes the probed value to the wrapper after pane startup. Set the variable
before starting Bus and restart Bus to change it. The 750 ms
budget covers address resolution and connection attempts. A gateway that
accepts TCP connections can pass while its HTTP handlers or upstream models
are busy because the check sends no HTTP request. If the listener is
unreachable, start Magpie with `magpie serve` and retry.
