# Dialog parser inventory before deletion

Request: Bus message 36801. Owner: codexdriver2 / 36737. Read-only baseline: 22dc11b7, which contains #284 7127454f and #285 9c931314. No parser has been deleted when this inventory is published.

## Existing driver and boundary

`src/terminal/driver` already provides a styled Grid with cursor and cell attributes, coherent observation, protocol-aware keys/paste, native input, and label selection with one arrow per observed redraw and pending-arrow deduplication. `src/server/api/agents/driver.rs` exposes observe/press; `src/devtools/tui/screen.rs` and input.rs consume the same Grid and input codecs. It reads arbitrary terminal prompts without knowing provider dialog vocabulary. Audit found that current driver::press does not hold the content lock or recheck a dialog digest through enqueue; the migration must add that guard inside the shared driver. It does not currently project a Grid into the old numbered dialog schema or infer Blocked from generic menu/hint evidence. That compatibility projection must live in the driver, and dialog choose/answer must call its guarded input instead of a second input implementation.

## Modules

| Module / piece | Detects or implements | Direct callers | Driver coverage / disposition |
| --- | --- | --- | --- |
| agents/dialog.rs parse, numbered, unnumbered, option_line, numbered_options | Last numbered menu, marker/styled focus, wrapping, borders, headings and hints | server/api/agents/dialog.rs; server/terminals/agents.rs; prompt/{composer,kimi,grok}.rs; tests | Driver selection already reads labels/focus from styled cells. Replace with one generic Grid projection in terminal/driver. No provider option-list parser retained. |
| agents/dialog/claude.rs | AskUserQuestion tabs, Next/Submit/Chat rows, custom text fields, checkboxes, selected background | agents/dialog.rs parse and Dialog methods | Generic styled cells preserve every visible tab, checkbox, field and instruction. Remove provider recognition and synthetic actions; use explicit driver keys for controls that cannot be uniquely inferred. |
| agents/dialog/kimi.rs | Trust and cache-expiry menus, exact headings and four hardcoded labels | agents/dialog.rs parse | Generic menu selection covers marker, equal indentation, descriptions and instructions above menu. Remove. |
| agents/dialog/cursor.rs | Workspace trust card and [a]/[q] accelerators | agents/dialog.rs parse | Driver reads bracketed accelerators and menu focus generically. Remove. |
| agents/dialog/grok.rs | Startup trust y/n, permission/question cards, parked focus, custom answer, checkbox actions, changing task chrome | agents/dialog.rs parse and composer.rs | Generic screen and explicit keys cover all layouts; menus must use shared selection and fresh screen guards. Remove card vocabulary and synthetic row actions. |
| agents/dialog/grok_login.rs | Login/Connecting blocker and l/q options derived from exact login detector | agents/dialog.rs parse | Driver reads full login screen and visible shortcut rows; existing provider lifecycle detector explains authentication status, not option parsing. Remove dialog parser. |
| agents/dialog/composer.rs | Suppresses false menus in Claude/Codex/Grok drafts using provider chrome | agents/dialog.rs parse | Grid cursor, menu scope and generic action-hint evidence distinguish an actionable menu from text drafts. Remove provider-specific suppressor. |
| agents/dialog/title.rs | Backward title extraction, special unlimited command heading | agents/dialog.rs, claude/kimi/cursor modules | Driver supplies full visible screen and generic menu scope; no provider title parser required. Remove. |
| agents/dialog/style.rs | ANSI decoding and selected-style inference | agents/dialog.rs, claude/grok modules | Grid already carries native styles and avoids a second ANSI interpretation. Remove. |
| agents/dialog/fingerprint.rs | Hash-only observation/write diagnostics and option/tab/input component hashes | agents/dialog.rs re-export; runtime/dialog.rs | Preserve safety/diagnostics through generic driver observation identity; remove dependency on provider Dialog. |
| agents/dialog.rs CODEX_PENDING_QUESTION_ID, codex_question_pending | Collapsed Codex queue, hardcoded Shift+Left and synthetic pending identity | server/terminals/agents.rs; server/api/agents/dialog.rs; prompt/composer.rs; coordinator/{dialogs.rs,control/dialogs.rs}; fake question tests | Generic driver can read shortcut hint and explicit keys. Remove Codex queue parser, special ID and automatic opening; keep inspection read-only and expose visible control generically. |
| agents/dialog.rs Dialog identity, keys_for, free_text_* | Provider-aware digest, hidden Claude rows, navigation, draft, skip semantics | runtime/dialog.rs and API conversion | Replace provider identities and bulk arrows/delayed Enter with generic driver frame guard and one-step selection; text/skip must use visible generic field/control evidence. |
| terminal/runtime/dialog.rs | Locked parse/digest check, redraw wait, key encoding, delayed confirm/text submit, Grok Esc+Shift+X | server/api/agents/dialog.rs; runtime tests | Driver owns guarded input, key protocol and paste already. Remove file after migrating to shared driver. Preserve stale/redraw/identity protection in compatibility API. |
| agents/tests/option_list_test.rs and coordinator/tests/control/option_list_test.rs | Provider-specific option-list regression coverage (not production modules) | test module declarations | Move applicable safety assertions to shared driver tests; preserve integration coverage for stale identity, drafts, sync frames and input enqueue. Remove obsolete parser-only expectations with the parser. |

## Caller inventory

| Caller | Current responsibility | Migration |
| --- | --- | --- |
| server/terminals/agents.rs agent_info | Parse dialog ID; special collapsed Codex ID changes list metadata | Obtain dialog metadata from driver Grid projection; no provider branches. |
| server/api/agents/dialog.rs | Observe/open pending; choose/answer identity guards; parser-to-schema mapping | Use driver observation and press for every input, with exact identity and observation guard; preserve compatibility responses. |
| terminal/runtime/dialog.rs | Second input/write route | Delete; move only generic coherence/guard operations to driver. |
| server/api/agents/prompt/composer.rs | Refuse pasting into blockers or Codex pending queue | Use shared driver blocker projection, retaining provider composer ownership logic outside dialog parsing. |
| server/api/agents/prompt/kimi.rs | Refuse ready composer on parsed dialog | Use shared blocker reading. |
| server/api/agents/prompt/grok.rs | Avoid queued-status shortcut while dialog visible | Use shared blocker reading. |
| messaging/coordinator/control/dialogs.rs | Fingerprints; opens pending Codex question once; chooses/answers; polls result | Keep signed API fingerprints and caller identity checks, remove special queue opening/ID, delegate input to migrated native dialog API. |
| messaging/coordinator/dialogs.rs | Stable episode notice; special pending ID bypass; fresh native verification | One generic observation/episode route; preserve once-only notices and Blocked lifecycle. |
| messaging/coordinator/auto_dialogs.rs | Closed exact-hash allowlist for harmless latency notice | Keep policy only if its identity can be derived from generic reading, update shape with evidence; it must not become another parser or bypass driver guards. |
| terminal/runtime/detection_task.rs and agents/manifest | Provider lifecycle/working/idle/error detection; generic active-request blocked fallback | Feed generic driver blocker evidence into published state; provider activity and login semantics are outside option parsing, keep only with fixture evidence and document retained boundary. |
| devtools/tui/{screen,input}.rs | Scratch UI Grid and protocol input | Already shares driver; retain and verify #285 production route proof. |
| devtools/tui/fake_agents/{grok,question}.rs | Test assertions call old parser on modeled screens | Migrate assertions to real driver observation or fixture semantics. Fake layouts remain test fixtures, not production parsers. |
| agents/manifest/tests/grok_login_test.rs | Login-state tests also assert old parser-derived synthetic text/actions | Keep login-state regressions; move inspectability/input assertions to generic driver using actual captured text. |
| agents/tests/{dialog,option_list}_test.rs; server/api/agents/tests/dialog_test.rs; runtime tests | Legacy parser and API regression coverage | Port behavior/safety to driver tests and compatibility API; do not score compilation or static inspection as acceptance. |

## Planned retained pieces and evidence requirement

No provider-specific dialog parser is planned to survive. Provider prompt-composer ownership and activity/authentication state detectors are not prompt answer parsers: they prevent sending a Bus message into another draft, identify running work, and report login errors. Existing captured fixtures under tests/fixtures plus manifest tests are their evidence; the final PR must list their exact retained scope, any actual generic limitation discovered, and fresh test evidence. Generic control ambiguity must return readable screen evidence and require explicit keys rather than invent actions. The final inventory will record implementation outcomes and test artifact paths.

## Verification contract

Preserve exact terminal/pane/session identity and signed fingerprint checks, coherent redraw refusal, draft protection, and no unobserved automatic Enter. TDD for behavior moved to driver. #285 acceptance must use a copied binary whose embedded SHA equals the final HEAD, all 15 fake dialogs, real zsh, a real Kimi and complete per-prompt driver-route proof. Final `./run test --full`; evidence copied to main checkout temp/pr-N before report. Builds use only an authorized slot and its matching shared target, with mandatory package clean and free-space check before each Cargo invocation.

## Implementation outcome

The later orchestration scope update retires the legacy compatibility API rather
than projecting the old numbered-dialog schema. All modules in `agents/dialog/`,
`agents/dialog.rs`, `terminal/runtime/dialog.rs`, the native dialog handlers and
coordinator dialog input implementation are removed. The CLI commands `agent
dialog`, `agent choose` and `agent answer` and their socket schema variants are
removed with their callers. No provider-specific dialog parser is retained.

`terminal/driver/blocker.rs` shares generic visible control evidence between
styled driver observations, blocked-state publication and existing composer
inspectors. It handles menu hints, visible shortcut rows, shell yes/no controls
and pending-question controls without provider vocabulary or synthetic actions.
Observation is read-only; orchestrators open panels with explicit visible keys.
The native press guard holds the PTY content lock through revision verification
and enqueue, refusing incomplete or changed frames without input. Selection still
advances one arrow per observed focus change. Existing prompt ownership readers
continue to protect drafts before Bus submits a message.

Provider lifecycle manifests and native composer ownership remain: their callers
identify activity/authentication and prove exact message ownership, rather than
parse answer options. Captured manifest/composer fixtures remain their regression
proof. The old parser-hash automatic latency choice is retired; orchestrators use
the same observe/press path for it as any other blocker. Historical persisted
metadata is a separate storage compatibility boundary and never supplies input.

Author evidence and acceptance are recorded under `temp/universal-dialog/` and
will be archived with the final head under the main checkout's PR evidence folder.
