**模式**：pr

**闸 (d)**：free-form 输入不可判

Target: PR #295, head 45fffccda08b626b111afbe61aa77b192c0c9f1d, base 306b177332aaefd3addfbbb6c6f1cafb1b3b8a7c.
Self-verified both canonical claims; no delegated adjudication. The two reviewer interrupt patches are test-only inputs and will be adopted as normal regressions if green after repair.

## APPLY

1. valid-issue / APPLY-BEHAVIOR — clauderev5 finding 1, usage.rs completed_final_text.
   SUPPORTED: the recorded same-launch control (source rollout lines 2,205,206,209; history-before-37106 request 36975) has empty AgentMessage Text and a citation-only rendered response. The durable Stop reply is empty. review-probes-red.log shows rollout_0164_matches_the_recorded_stop_reply_without_citation_metadata returning the citation XML instead of empty text. This violates the locked goal's exact final-answer recovery.
   Root cause: rendered metadata is preferred over the native completed AgentMessage body. Owner: Codex rollout reader. Repair direction: prefer completed AgentMessage text, retaining response-only support for plain final text and normalizing pure citation metadata to an explicit empty final. Preserve exact body text, turn identity, completion boundary, missing-text rejection, legacy completion text and provider-error precedence. Keep the byte-identical input records and correct their independently verified expected body to empty. Add native-hook parity and response-only metadata checks.

2. valid-issue / APPLY-BEHAVIOR — clauderev5 finding 2, corroborated by codexrevf saved probes; status.rs direct consumers in control/agents.rs and model/requests.rs.
   SUPPORTED: review-probes-red.log has explicit interrupt outcome failed, room interrupt agents=[], and a busy queued request expiring, all with native Working/Blocked or bound Idle while the missing-directory projection is Unavailable. This is a direct regression of the locked goal's availability projection while preserving native activity for in-flight work.
   Root cause: the new availability projection is also used to decide whether execution exists. Owners: RoomAgent status semantics and its control/recovery consumers. Repair direction: retain a common activity status (native status plus existing dialog/binary rules), derive shown availability from it, and use activity status in interrupt targeting/dispatch and busy queue recovery. The direct CLI waiter also consumes the shown state; activity-control-red-2.log proves both the missing activity field in real control state and the CLI false-unavailable outcome. Expose additive activity_status in control state and make the waiter prefer it, with the legacy status fallback for older servers. Keep TUI/control inspection Unavailable and existing client-log-only diagnostics. Preserve single stop-key dispatch, bound-Idle interruption, real process unavailability and queue retention. Adopt the three saved probe cases.

## REJECT

无。

## FLAG

无。

## HOUSEKEEPING

The interrupt and busy-queue assertions share one projection root cause; repair once. No source probes existed before this author verification. The first gate artifact is FAIL due to missing pytest-xdist, with integration/coverage lacking the consequent profile state; private venv requirements were installed, with no source/tooling policy change. Repository-wide gates are not part of this review-response validation; the user's separately requested final-head full gate and live acceptance will follow the scoped repair checks.

lane 独有：clauderev5: 1  codexrevf: 0  总计: 2

## Landed repair and validation

Head ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266. Parser repair a4b19b40 and activity repair ad9dc509 are pushed; no probe files are left uncommitted. All three codexrevf probe cases are adopted in the activity commit. Scoped command: cargo nextest run --locked --no-fail-fast -E 'test(agents::providers::codex::usage) | test(turn_settlement_tests) | test(interrupt) | test(working_directory) | test(missing_launch_directory) | test(send_recovery) | test(directory_error_alias) | test(master_directory_warning)' — 130/130 PASS. Logs: /Users/dylanliu/work/bus/temp/pr-295/review-repairs-green.log, review-probes-red.log, response-metadata-red.log, activity-control-red-2.log. The first activity-control attempt was a test compilation error (private method); it was corrected to exercise the public control response, then both cases failed on behavior before production repair.

The later codexrevf round-01 review corroborates the same activity root and introduces no additional canonical claim. Combined sanitized input is retained alongside this ledger. Two APPLY roots; REJECT and FLAG none. Both repair groups serve the locked goal and preserve its non-goals. The full final-head gate and real-provider acceptance are separate authorized work, not claimed by this scoped review-response validation.

Combined structured sanitation rejected abbreviated/full commit spellings in branch headers as unequal. Both immutable reviews were re-sanitized through the documented labelled free-form path; git rev-parse proves the exact reviewed head. The explicit originating locked goal is unchanged. No historical input was rewritten.
