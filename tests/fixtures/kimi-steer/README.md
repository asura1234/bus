# Kimi steering captures

Captured on 2026-11-01 with Bus base `a0f2f0b0b2de10d921a4a9804d2f4c61fd59c7f3`, Kimi Code 2.4.0 (K3.1 Pro), macOS arm64. The Bus binary was built in the owner worktree, copied and compared under the room 877 build slot. Both scratch run_start records identify that full SHA. No live Bus was used for these captures.

Scratch root: `/private/tmp/bus-kimi-steer-8xvlrlga`, room 2, agent 4, native session `session_aed9a55f-7414-4c34-9b48-313cf1f39600`. The initial no-SHELL launch was discarded before any provider launched; the capture run uses zsh and the approved real-HOME login mapping. Credentials and OAuth locks are shared by Bus's own private-home setup.

- `command.txt`: `agent read --source visible`, 56 columns by 40 rows, message 6's foreground `sleep 45`; subsequent steer message 9 was definitely rejected in 5 ms at 05:14:26.457Z.
- `thinking.txt`: same viewport, message 13 thinking before its shell call.
- `command-tip.txt` and `thinking-tip.txt`: the original 131-column viewport. The moon carries a tip; base incorrectly reports Idle and leaves follow-ups behind `prior_request_active`.
- `approval.txt`: message 6's actual shell approval at 131 columns. Message 9 stayed queued with `agent_blocked` until an explicit Approve once through the fresh Bus dialog fingerprint.
- `idle.txt`: native final of message 9, with the empty framed composer at 56 columns. The initial native idle submission and committed final both succeeded on base.
- `steered.jsonl`: selected complete native rows from message 13's turn 3: turn.prompt, prompt.steered, the final assistant message, and the three terminal commits. Guidance was entered through the scratch native API with Ctrl+S while the shell was running; no transcript row was invented. Unrelated tool and reasoning rows are omitted. The event binds guidance to activePromptId without starting another turn.

Private raw snapshots, status envelopes and logs are retained under the scratch root's `evidence/` directory and the owner's ignored `temp/kimi-steer/` directory.

`thinking-iterations-2.4.1.jsonl` was captured with exact Bus head `13631d8196d4c86338df7e65baab92f3631ed0d3` and the approved Kimi executable reporting 2.4.1, in `/private/tmp/bus-kimi-owner2-rwpyfoa1` (room 2, agent 3). Its selected native rows are compacted; reasoning and nonessential metadata are removed. Outer prompt 1 contains two assistant finals and internal agent ends 1 and 2 before its outer terminal commits. Native guidance is attested to that prompt, but this Bus head incorrectly returned the first BASE final instead of the final GUIDED response. The regression tests also vary internal-counter drift, silence, failure, late guidance and a think-only fragment within one iteration. These variations are synthetic; the captured two-iteration sequence is real native evidence.
