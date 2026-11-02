**模式**：pr
**Target**: PR28503F source, locally restacked unchanged as e1b4e7c0 on030; locked goal/non-goals unchanged. Structured Claude R1 sanitized; main self-verifies, no delegated repair.

## APPLY
B1 / clauderev4 R1-1 — SUPPORTED, APPLY-BEHAVIOR valid-issue. Source finalizer calls asserting control(stop), then retained marker/report path. Saved original teardown probe and adopted normal test both assertion-RED: PRIMARY startup failure is masked by stop failed, and evidence path skipped (evidence-red.log). Owner: acceptance teardown. Independently selected repair: record optional stop result/error, close client and preserve an unconfirmed root, always print the evidence path; never raise cleanup over the active primary exception. A successful body with failed cleanup must fail rather than persist PASS. Preserve private stop/root lifecycle and historical attempt separation.

B2 / clauderev4 R1-2 — SUPPORTED, APPLY-BEHAVIOR valid-issue. Source complete() checks outcomes only. An independent full-method mocked counterexample supplies all15 valid outcomes/markers/reply without executing any driver calls and the old method passes (adopted test_outcomes_without_driver_routes_cannot_pass assertion-RED, evidence-red.log). Owner: acceptance proof. Goal explicitly requires the shared driver to clear every prompt; Master36686 confirms this contract. Independently selected repair: private wrapper records metadata-only agent-press receipts with the pre-input audited prompt prefix; correlate each of15 prompt IDs and the shell receipt by control request ID/terminal to actual bus.terminal.driver.result events. Reject legacy send-keys/choose/answer (including native pane input except the recorded runner setup call). No answer contents in route records, and no driver restriction is added.
The candidate logging basis was checked: old client/server JSONL copies contain performance records, not these tracing events. Existing bus.control.command spans already carry control_request_id; copy actual herdr-client/server.log files/archives into private evidence and validate those. No production logging/API expansion needed for this claim.

## REJECT
None.
## FLAG
None.
## HOUSEKEEPING
Independent cleanup and provenance roots; no duplicate/sync items.

## Probe inventory / group
Adopted saved teardown probe as AcceptanceEvidenceTests, preserving primary failure and evidence assertions plus retained-root proof. Original patch/log stays ignored. Added outcome-with-no-routes counterexample; both genuine assertion-RED before repair. One serial group: tests/dialog_driver_acceptance_test.py, tests/dialog_driver_acceptance_support.py (portable private wrapper/proof/log helpers), tests/fixtures/dialog-driver/README.md. No source/test ownership outside acceptance. Real provider/gates remain pending until final exact head; no speculative timeout changes or relaxed assertions.

Original/adopted teardown and full-method outcome-with-no-driver counterexample both assertion-RED (evidence-red.log). All6 runner/metadata-policy tests now PASS (evidence-green-3.log), including native-result request/terminal matching, missing/repeated/wrong-target evidence rejection, legacy input rejection, metadata-only receipt capture, historical PASS separation and retained failed-stop roots. Scoped Ruff E9,F on the two changed Python files and diff whitespace PASS. Canonical child gate/provider acceptance NOT RUN for this repair tree; Rust scope follows final parent restack. lane 独有：clauderev4: 2 总计: 2
