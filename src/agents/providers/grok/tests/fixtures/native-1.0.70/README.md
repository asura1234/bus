# Authenticated Grok Build 1.0.70 fixtures

These JSONL files are redactions of `updates.jsonl` from authenticated native sessions documented in `temp/grok-provider/native-capture.md`. The raw files and streaming output remain in the owner-only `<PRIVATE_CAPTURE_ROOT>/raw/` directory. Session and prompt UUIDs were replaced with stable fixture IDs, timestamps and unrelated metadata were omitted, and the disposable project path became `<SCRATCH_PROJECT>`. The method, update kind, prompt association, stop reason, message order, Unicode answer bytes, and final text boundaries were preserved. No fixture row asserts an outcome absent from the native source.

`ordinary.jsonl` and `unicode-continuation.jsonl` contain completed answers; the latter has two distinct prompts in one session. `tool-final.jsonl` has an assistant preamble, tool events, and then the final answer. `cancel-early.jsonl` has no completion row. `max-turns-failure.jsonl` has a tool preamble and `turn_completed/cancelled`, so it must never be published as a reply.

`long-unicode-180.jsonl` adds a 180-line completed answer with CJK, emoji, accented Latin, Cyrillic and Arabic text. The final native message is 7,019 characters and 10,799 UTF-8 bytes; the regression checks its full text.

`clipped-answer.jsonl` and `clipped-answer-hooks.json` capture a real answer of 36,839 characters. The Stop hook's `lastAssistantMessage` has only 32,768 answer characters plus `… [+4071 chars]`. The native transcript has the full final message and a later `turn_completed/end_turn`; the regression checks its full SHA-256 and never uses the clipped hook preview as answer text.

`idle-composer.pty` is a real fullscreen 1.0.70 PTY capture from the same disposable project, with its displayed project path replaced by `<SCRATCH_PROJECT>`. It shows an editable `❯` composer, the updated `Grok 4.7 (high)` footer, and `Grok Build 1.0.70`; it contains no model prompt. The private raw PTY is `raw/grok-idle.pty`.

`always-approve.jsonl` is a separate real `--always-approve` turn. It ran a harmless `pwd` tool call without a permission dialog and committed an `end_turn` answer. It verifies Grok's no-approval argument behavior; Bus's run-scoped `--yolo` translation still needs its scratch Bus check.

`gate-continuation.jsonl` and `gate-continuation-hooks.json` come from a real project Stop hook that blocked the first answer. The second Stop had the same prompt ID and `stopHookActive:true`; only after it allowed the turn did `turn_completed/end_turn` appear. `gate-cancel.jsonl` and `gate-cancel-hooks.json` come from a Stop hook interrupted with Ctrl-C. Its first Stop payload carried an answer, but the transcript ended after `hook_execution` with no `turn_completed`; the answer is therefore uncommitted. The private raw hook payloads are `raw/stop-continuation-hook.jsonl` and `raw/stop-gate-hook.jsonl`.

`submit-stop-hooks.json` and `submit-stop.jsonl` are a real matched submit/Stop/transcript set. `user_prompt_submit` carries exact `prompt`, `sessionId` and `promptId`; the Stop carries the same IDs and an owned `transcriptPath`. The transcript's answer chunk and `turn_completed` use that prompt ID. The coordinator regression uses this fixture pair, replacing only the redacted transcript path with its test-local copy.
