# Gate-and-Fix Round Format

`scripts/gate_and_fix.py` is the sole producer, validator, failure-gate lister, and log decoder for
this UTF-8 Markdown artifact. Only after `verify` validates the artifact structure and base may a
consumer use `list` to enumerate failed gates or `show` to read a selected gate log.

````markdown
# Gate-and-Fix Round

- Schema: `3`
- Round: `<positive integer>`
- Base: `<immutable commit SHA>`
- Head: `<commit SHA>`
- Outcome: `PASS|FAIL`

## Changed files

- `<sorted, unique repository-relative path>`

## Gate results

### <lowercase-kebab gate name> — PASS|FAIL

- Command argv: ["<executable>", "<argument>"]
- Exit code: `<integer|launch-error>`
- Duration: `<non-negative integer>ms`

#### stdout

- Encoding: `base64-utf8`
- Bytes: `<UTF-8 byte count>`

```base64
<base64 encoding of the complete captured stdout>
```

#### stderr

- Encoding: `base64-utf8`
- Bytes: `<UTF-8 byte count>`

```base64
<base64 encoding of the complete captured stderr>
```
````

Changed files must be nonempty, sorted, unique and repository-relative. Gate results preserve
selection order. A gate is PASS exactly when its exit code is `0`; every other exit, including a
launch error, is FAIL. Round Outcome is PASS exactly when every gate is PASS. `Bytes` is the length
of the UTF-8 bytes before base64 encoding; consumers must reject invalid base64, an unequal length,
or invalid UTF-8. This makes the captured stream exactly recoverable, including empty output, a
missing final newline, final newlines, and all trailing whitespace.

Consumers must run `verify` first, then run `list --artifact <path> --base <base>` to receive one
failed gate name per line. For each listed gate, run
`show --artifact <path> --base <base> --gate <name> --stream stdout|stderr`. `show` writes only the
selected complete captured stream to stdout, without a separator or final newline; consumers must
not hand-decode the Base64 Markdown.

Command argv is a single-line JSON array of strings, with a nonempty executable as its first
argument. JSON escapes preserve newline, backtick, quote, Unicode and empty-argument values
losslessly. Invalid JSON, a non-array, an empty array, a non-string member or an empty executable
is malformed. There is no schema-2 compatibility path.
