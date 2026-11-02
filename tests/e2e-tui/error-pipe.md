# Dev error pipe

## Run contract

Fresh scratch Bus using the exact-head dev-tools binary copied into this run's
private artifact directory; 160x65, graphics none. Fake providers only. Never use
the live Bus or a global provider configuration. CLI fixtures may create work
rooms, a fake Claude MASTER orchestrator bound to pipe-work, mute room sounds,
and inspect state/history/settings. Settings changes must use the UI.
`fixture error --text EVENT` emits an actual client/coordinator ERROR with private context
and message fields; it never inserts a message directly. Each fixture emits one
event at the same real logging callsite. Quiet windows use `wait --stable 3s
--rows 0-0 --timeout 8s`; no skips or retries. Capture screenshots and open them.

### ERRPIPE-01 — Select an orchestrator, aggregate real errors, keep the UI quiet, and turn Off

Preconditions: fake Claude orchestrator pipe-lead in MASTER bound to pipe-work;
pipe-other is a separate work room. All room sounds off; fake provider setup
settled and pipe-lead idle. Default error reporting Off.

1. Open Settings, navigate to Developer and open the Bus errors dropdown. Confirm Off is selected and pipe-lead is an option; work-room agents are not options. Capture the dropdown.
2. Select pipe-lead through the dropdown. Read state and the private settings file to confirm error_orchestrator is pipe-lead. Close Settings, select pipe-other, and type KEEP DRAFT. Capture the quiet room before injection.
3. Invoke `fixture error --text bus.dev.pipe_test` twice. Wait for the targeted Bus request in pipe-work and its fake-provider answer. Confirm exactly one Bus-authored request to pipe-lead with `Bus error: bus.dev.pipe_test (x2 since last notice). Logs: <absolute client log path>`, and exactly one provider receipt of that notice. The referenced client log must contain both ERROR events and their private fields; the notice must contain none of those fields or excerpts. Capture state/history and the log evidence.
4. Invoke the same fixture once more. Through the quiet window confirm the request count is unchanged. Confirm pipe-other stays visible with KEEP DRAFT, no error toast/inline/sidebar/footer text, and no developer notification effects. Capture and inspect the screen and effects; only ordinary chat history in the recipient room may contain the short notice.
5. Reopen Settings and select Off through the dropdown. Confirm saved settings and state show null. Close Settings and inject the distinct event `bus.dev.off_test`. The client log must contain that ERROR, but after the quiet window no new Bus error request exists. Capture the Off selection and final state, then stop and remove the scratch session while preserving its evidence.
