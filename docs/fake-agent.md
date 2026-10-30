# Scripted fake agent for Bus tests

The dev build can launch one `fake` agent in a disposable Bus session. It uses
Bus's prompt, callback, session, and reply contracts directly. It does not run
an AI model, load credentials, or imitate any provider. Production builds and
ordinary Bus sessions do not admit `--provider fake`.

## Test author workflow

1. Build the dev binary with `cargo build --locked --features dev-tools --bin bus`.
2. Create an owner-only scratch `HOME`, `BUS_DATA_DIR`, and project. Put the dev
   binary's `bus-fake-agent` alias first on the scratch `PATH`. Set
   `BUS_TUI_FAKE_AGENT=1` and a unique `BUS_TUI_OWNER` for that scratch run.
3. Write a JSON script and add the agent with `bus agent add --provider fake
   --args '--script /absolute/path/to/script.json'`. Use the same scratch
   `BUS_DATA_DIR` for every Bus control command.
4. Send a message and assert its `message status`, reply, dialog or exit state.
   Do not infer completion from a terminal snapshot alone.

The script format is versioned and matches one unused `turns` entry whose
`contains` token appears in the submitted prompt. Unmatched turns get a
default echo reply; this lets Bus's initial onboarding turn complete without
consuming a test case. Matched entries stay consumed across a cold resume.

```json
{
  "version": 1,
  "turns": [
    {
      "contains": "CASE_REPLY_7",
      "actions": [
        {"type": "sleep", "ms": 100},
        {"type": "reply", "text": "REPLY_7"}
      ]
    },
    {
      "contains": "CASE_DIALOG_7",
      "actions": [
        {"type": "dialog", "title": "Choose a path", "options": ["A", "B"]},
        {"type": "reply", "text": "selected {{choice}}"}
      ]
    },
    {
      "contains": "CASE_SEND_7",
      "actions": [
        {"type": "send", "room": "work", "to": "peer", "text": "from fake"},
        {"type": "reply", "text": "SENT"}
      ]
    },
    {"contains": "CASE_CRASH_7", "actions": [{"type": "crash", "code": 42}]}
  ]
}
```

`reply` commits one final callback. `fail` closes the turn without a reply.
`sleep` keeps the agent visibly working for a bounded number of milliseconds.
`dialog` blocks until a visible choice is made or dismissed; `{{choice}}` is
the selected label. `send` calls the scratch Bus as this agent and requires a
successful receipt. `crash` exits without a final callback. A script action
must finish before the next action starts. Only an explicit `reply` commits a
successful response.

Keep scripts and test artifacts under a private scratch root. Do not point a
test at the live Bus data directory or reuse a real provider's home. The fake
is for deterministic Bus protocol tests; it provides no provider acceptance.
