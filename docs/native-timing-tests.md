# Reply-delivery fixture timing under load

Two blocking failures were the first `typed(orchestrator).last()` lookup in
`worker_message_during_orchestrator_background_hold_keeps_the_held_master_reply`
and `worker_message_to_orchestrator_busy_with_a_master_turn_gets_its_own_reply`.
The first submission happens before either test exercises its background-hold or
busy-turn behavior. Both use the same scripted provider in `Delivery`.

The fixture runs the real coordinator, socket transport, App API, terminal
submission writer, callback spool and persistence. Only the provider is fake.
Its screen redraws happen when the fixture's pump gets scheduled. Previously,
the native confirmation/withdrawal clocks kept advancing while that pump was
not running. The native writer could correctly expire its 2.3-second
confirmation window (3.3 seconds including withdrawal), so the coordinator never
saw an accepted submission and `typed()` stayed empty. Extending only the test's
outer assertion wait cannot change an already-expired submission.

The regression test deliberately delays the fake pump for three seconds and
records the actual App API response. With a real clock it requires the native
timeout/refusal and no Enter; with the fake clock it requires `agent_prompted`
and exactly one submission through the same pipeline. This distinguishes a
fixture scheduling stall from dropped coordinator traffic. It does not establish
real-provider performance or relax the product's time bounds.

`SubmissionClock` uses `Instant` and real sleeps in production. Pausing exists
only under `cfg(test)`. The scripted delivery fixture enables it explicitly;
clock boundary tests control it directly.
Its API response waiter shares the selected terminal's clock. Redraws wake the
writer; a generation captured before observing/writing prevents a redraw between
observation and waiting from being lost. This also prevents repeated editing keys
from filling the mock channel while its provider pump is descheduled. Missing
fixture progress still fails after a sixty-second real ceiling.

The fixture's submission RPC wait allows that same scheduling ceiling. Its
non-submission RPC deadlines stay real, preserving the kernel receive-timeout
ownership/recovery regression. Existing expired-dispatch, withdrawal, and native
transport budget/classification tests also retain real time. Other composer,
geometry and notification timing work is a separate follow-up.

The full failure catalog and raw task evidence are preserved under ignored
`temp/native-timing/`, including an unchanged `494ed7ee` binary that reproduced a
native-input wait failure after a controlled three-second scratch-process
suspension. Four earlier focused probes passed; the controlled stall is not
presented as a naturally recurring failure in those runs. No CPU stress was
added on top of the normal two build slots.

Validation must retain default parallelism and no retries. A complete local gate
and exact-head independent review are release evidence; this repository has no
hosted CI. Reduced-thread retries are not the fix.
