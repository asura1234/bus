#!/usr/bin/env python3
"""Render the spike result from archived comparison data, retaining failures."""
import json, math
from pathlib import Path
R=Path('/Users/dylanliu/work/bus/temp/spikes/codexspikeb')
c=json.loads((R/'comparison.json').read_text())
def ms(x):return 'N/A' if x is None else f'{x:,.1f}'
def pct(b,a):return 'N/A' if b is None or a is None else f'{100*(b-a)/b:.1f}%'
def triple(d):return ' / '.join(ms(d[k]) for k in ['p50','p90','max'])
head=['| Scenario | BEFORE p50 ms | AFTER p50 ms | IMPROVEMENT | Correctness before → after |','|---|---:|---:|---:|---|']
for key in ['S1','S2','S3','S4','S5']:
 b=c['before']['scenarios'][key];a=c['after']['scenarios'][key];bv=b['accepted_to_enter']['p50'];av=a['accepted_to_enter']['p50']
 bt='>15,000; no Enter' if key=='S2' else ms(bv)
 at=('>15,000; no Enter' if key=='S2' else '>60,000; no Enter') if av is None and key in ['S2','S3'] else ms(av)
 head.append(f"| {key} | {bt} | {at} | {pct(bv,av)} | {'PASS' if b['correct'] else 'FAIL'} → {'PASS' if a['correct'] else 'FAIL'} |")
text='\n'.join(head)+'\n\n'
text+='**Not Ready.** Event-driven confirmation improves the successful Enter path, but this spike does not satisfy the no-stuck-draft requirement or the chat latency bar. Censored failures have no p50 endpoint and no improvement percentage. S4 includes intentional permission-dialog waiting. All times below are milliseconds.\n\n'
text+='## What ran\n\n'
text+='Baseline source: master `18624b2dbe30919b9fc38f5691b81586b4bf7cea`. Instrumented binary: `67b76f7e6c53aee9a191a38e1cafbbd9447b46cc`, copied and SHA-256 recorded in every `setup.json`. The same binary runs the unchanged master submission path with `BUS_SPIKE_FAST_CONFIRM` absent, and the prototype with it set. This is a switched master-path baseline, not a separately compiled pristine master binary. The per-PTY change counter and content-free timestamp fields also exist in the off run.\n\n'
text+='Only fake Claude and Codex processes ran, found through a closed scratch PATH. Every run used a fresh mode-0700 private `BUS_DATA_DIR`, scratch HOME/TMPDIR/ZDOTDIR and no copied provider credentials. Source worktree: `/Users/dylanliu/work/bus-spike-codexspikeb`, branch `spike/codexspikeb`. No real model calls, PR, review or full gate.\n\n'
text+='S1: 10 idle/empty fake-Claude deliveries. S2: a 64-byte multiline developer draft with tabs, blank lines, Unicode and trailing whitespace; one delivery plus a next-delivery probe. S3: the unmodified 823-byte payload steered into fake Codex during `fake:work 3`; before n=5, after n=1 (the first failure is retained). S4: five deliveries while a four-second fake permission dialog was visible. S5: 30 serial deliveries to one agent plus ten waves of five simultaneous deliveries (50 parallel deliveries).\n\n'
text+='The after run stopped at S3 to retain its uncertainty and draft. S4 and serial S5 then ran in a fresh private Bus. A parallel setup encountered resource/readiness failures before deliveries began; the final parallel pair uses identical control-socket `message.send` envelopes for both paths, avoiding five CLI process launches per wave. The accepted boundary and server delivery code are unchanged. All archived attempts remain available; setup failures are not silently converted to delivery passes.\n\n'
text+='## Correctness\n\n'
text+='| Scenario | Evidence and result |\n|---|---|\n'
text+='| S1 | Exact full payload observed at one consumed Enter for every sample; fake submit state is empty afterward. |\n'
text+='| S2 | FAIL both paths: original developer draft retained byte-exact, zero Enter, request still queued after 15 s, and the next delivery also remained queued. Agent status is Blocked while dialog=false; submission never starts. No restoration success is claimed because no delivery ran. |\n'
text+='| S3 | Before: 5/5 exact 823-byte payloads consumed once. After: full payload is in the fake draft, zero Enter, no owned-composer observation, native outcome uncertain, and request still submitting/blocked after 60 s with `[Pasted Content 653 chars]` visible. Next-delivery success is unproven after that failure. |\n'
text+='| S4 | No Enter in any audited dialog-open/close interval. Queued deliveries ran only after the dialog closed; exact-once payload consumption checked independently. |\n'
text+='| S5 | Requested serial/parallel counts, exact bytes and one consumed Enter are checked in `comparison.json`; a verdict is PASS only at n=80 with every sample correct. Subsequent deliveries in each sequence and the final empty fake submit state corroborate no residual draft. |\n\n'
text+='## Latencies\n\n'
text+='Nearest-rank p50 / p90 / max, joined from durable request IDs, pane identity and log timestamps. Started means accepted native Started/AcceptedSteering callback correlation; it includes fake hook and callback scheduling.\n\n'
text+='| Scenario | n before / after | Enter before p50 / p90 / max | Enter after p50 / p90 / max | Started before p50 / p90 / max | Started after p50 / p90 / max |\n|---|---:|---:|---:|---:|---:|\n'
for key in ['S1','S2','S3','S4','S5']:
 b=c['before']['scenarios'][key];a=c['after']['scenarios'][key]
 text+=f"| {key} | {b['n']} / {a['n']} | {triple(b['accepted_to_enter'])} | {triple(a['accepted_to_enter'])} | {triple(b['accepted_to_started'])} | {triple(a['accepted_to_started'])} |\n"
text+='\n`accepted` is the coordinator `bus.control.command.dequeued(method=message.send)` timestamp, matching the developer\'s latency baseline definition for CLI sends. It precedes the durable queue save and excludes CLI launch/socket admission time. Enter is the first successful writer-flush acknowledgment, not enqueue. Fake consumed-input audit is an independent exact-byte and exact-once check.\n\n'
text+='The existing `paste_flushed` acknowledgment includes the deliberate 300 ms delay. Actual recipient-box timing below uses the fake\'s complete-draft audit immediately before redraw; that is provider composer state, not a Bus UI paint acknowledgment.\n\n'
text+='| Scenario | Box BEFORE p50 / p90 / max | Box AFTER p50 / p90 / max | AFTER p99 |\n|---|---:|---:|---:|\n'
for key in ['S1','S2','S3','S4','S5']:
 b=c['before']['scenarios'][key]['accepted_to_fake_box'];a=c['after']['scenarios'][key]['accepted_to_fake_box']
 text+=f"| {key} | {triple(b)} | {triple(a)} | {ms(a['p99'])} |\n"
text+='\n## Against the chat bar\n\n'
for mode in ['before','after']:
 rows=[r for r in c[mode]['samples'] if r['scenario'] in ['S1','S2','S3','S4','S5']]
 failed=sum(not r['correct'] for r in rows)
 ready=[r['timing']['fake_box']-r['timing']['accepted'] for r in rows if r['scenario'] in ['S1','S5'] and r['timing'].get('accepted') is not None and 'fake_box' in r['timing']]
 def q(p):return sorted(ready)[math.ceil(p*len(ready))-1] if ready else None
 text+=f"- {mode.upper()}: ready S1+S5 actual-box p50 {ms(q(.5))} ms, p99 {ms(q(.99))} ms (n={len(ready)}); {failed}/{len(rows)} scenario deliveries failed ({100*failed/len(rows):.2f}%).\n"
text+='\nThe required p50 <200 ms, p99 <1,000 ms and failures <0.1% are not established. S2 and after-S3 failures stay in the correctness denominator. S4 permission waiting is excluded from the ready-box cohort. These small, loaded-host samples cannot establish a production failure rate below 0.1%.\n\n'
text+='## Fixed waits, polling and grace in this path\n\n'
text+='| Mechanism | Before | Prototype | Actual cost / meaning |\n|---|---:|---:|---|\n'
text+='| Paste submission delay, protocol/api/submission.rs | 300 ms | 0 ms | Unconditional on normal submissions; measured phase distributions below. Windows-specific Codex delay is 600 + bytes/4 ms and was not exercised on this Mac. |\n'
text+='| Initial composer / await-paste / confirmation observation loop | 20 ms polling | PTY change Condvar wake; up to 250 ms fallback | The generation is sampled before inspection to avoid a lost wake. Content lock, complete-frame check and ownership inspector remain before Enter. |\n'
text+='| Confirmation grace | 2,000 ms plus paste delay | 2,000 ms | Deadline, not a happy-path sleep. Setting delay=0 also reduces the total confirmation budget from 2,300 to 2,000 ms; evidence does not justify that shortening under load. |\n'
text+='| Enter retry spacing | 250 ms | 250 ms | Only after a prior Enter when the same draft is still proven owned. No paste retry. |\n'
text+='| Editor/native stash transfer loop | 20 ms | PTY wake with 20 ms fallback | External editor files can change without PTY output; polling not removed. S2 is blocked before this layer. |\n'
text+='| Preserved-paste recovery loop | 20 ms | 20 ms | Unchanged recovery path, not a normal successful submission wait. |\n'
text+='| Clear-key spacing | 25 ms/key | 25 ms/key | Kept to separate native editing events. |\n'
text+='| Draft quiet window | 3,000 ms | 3,000 ms | Eligibility bound after developer typing, not a sleep inside every paste. |\n'
text+='| Draft transfer reserve | 6,000 ms | 6,000 ms | Budget for capture/restore/verify; no unconditional six-second wait. |\n'
text+='| Withdrawal reserve | >=1,000 ms; 150 ms/logical line | unchanged | Progress/deadline reserve. Failed/no-progress withdrawal can use it. |\n'
text+='| Coordinator status poll | 500 ms; 1,000/2,000 ms after failures | unchanged | Adds 0–500 ms normal phase latency; queue and serial native work can add more. |\n'
text+='| Detection ticks | 300 ms identified / 500 ms unidentified; 50 ms startup | unchanged | Readiness/Working/Idle propagation. |\n'
text+='| Working-to-Idle stability | 100 ms rechecks, three confirmations, 700 ms cap | unchanged | Conditional hold when no positive ready/blocker signal is visible. |\n'
text+='| Stable visible signal refresh / startup grace | 800 / 3,000 ms | unchanged | State signal refresh and startup classification bounds; not unconditional per-send sleeps. |\n'
text+='| Stale-paste maximum age | 60,000 ms | unchanged | Recovery ownership age bound; not a normal send delay. |\n'
text+='| OWN_TURN_GRACE | 10,000 ms | unchanged | No permitted Kimi fake acceptance evidence to justify removal. It is not a universal ten-second sleep. |\n'
text+='| prompt_not_shown retry | 20,000 ms | unchanged | Rejected-prompt recovery path; can dominate a failing delivery. |\n'
text+='| CLI control framing poll | 2 ms | unchanged | Client socket readiness loop; final S5 parallel pair uses direct equivalent socket frames. |\n'
text+='| PTY actor idle poll | 1,000 ms | unchanged | Input readiness wake drains ready events; not a normal one-second send delay. |\n'
text+='| RPC margin / response grace | 1,000 / 100 ms | unchanged | Deadline allowances, not mandatory latency. |\n'
text+='| Fake work tick / permission fixture | 120 / 4,000 ms | unchanged | Fixture behavior, not Bus overhead. |\n\n'
text+='| Scenario | Paste-write phase BEFORE p50 / p90 / max | AFTER p50 / p90 / max |\n|---|---:|---:|\n'
for key in ['S1','S3','S4','S5']:
 text+=f"| {key} | {triple(c['before']['scenarios'][key]['paste_write'])} | {triple(c['after']['scenarios'][key]['paste_write'])} |\n"
text+='\n## What broke and production requirements\n\n'
text+='The first audit reader consumed an incomplete JSON line; it now reads complete records, and fake processes use separate audit files. A short fake-Claude dialog setup could not establish the target dialog state within the native confirmation budget; the comparison S4 fixture uses fake Codex with an observable four-second dialog. Those discarded setup attempts are archived separately.\n\n'
text+='S2 has a concrete readiness cycle: `coordinator/poll.rs:detection_contexts` marks queued requests as obligations; `runtime/detection_context.rs:ready` accepts only an Empty composer; a foreign draft produces Other; `detection_policy.rs:apply_obligation_detection` therefore publishes Blocked when activity is quiet. `coordinator/delivery.rs` refuses Blocked agents, so the existing preservation routine never runs. This is unchanged by B.\n\n'
text+='The after S3 failure is an observed counterexample, not proof that the new wake mechanism alone caused it. Its audited fake spent 1,175.904 ms between receiving the Paste input and recording the complete draft, versus 0.779–2.965 ms in the five baseline S3 samples. The host reached load1 >600 and OS resource errors. JSON audit formatting itself makes multiple file writes. Before making a production causal claim, use a buffered single-write audit and a deterministic delayed-render / Working-to-Idle fixture. Never remove the failed sample or infer a historical provider cause from it.\n\n'
text+='A production version needs: (1) distinguish a preservable developer draft from a real readiness blocker so S2 reaches the existing preservation machinery; (2) an owned-paste cleanup/recovery trigger when the composer later becomes readable, independent of the next request, so an uncertain active request cannot latch its draft; (3) keep timeout budgets separate from paste delays; (4) cover late/partial/synchronized redraws, dialog replacement, developer edits, input backpressure, resize, cancellation and a lost flush acknowledgment; (5) provider-specific acceptance before removing any Enter-suppression delay or Kimi grace; (6) integrate coordinator/detection event work separately, since B alone cannot remove their dominant waits.\n\n'
text+='## Evidence and reproduction\n\n'
text+='`comparison.json` contains every selected sample and its evidence part, byte check, consumed Enter count, status and timing endpoints. Each part retains `samples.json`, `fake-audit.json`, copied client/server plain logs, perf run-start identities, native terminal output, stop receipt and cleanup receipt. Failed setup attempts are retained under `attempt-*`. Build log: `build.log`. No full gate or real-provider validation was run.\n\n'
text+='Sources committed under `tools/spikes/`: `fast_confirm.py`, `analyze_fast_confirm.py`, `render_fast_confirm_result.py`. Build a dev-tools binary under an authorized slot, copy it to `/private/tmp/codexspikeb-binary/bus`, then run the harness modes `before after`, `after-tail`, and `before-parallel after-parallel`; run the analyzer and renderer. The hardcoded output paths intentionally make this a throwaway local spike.\n'
(R/'result.md').write_text(text)
(R/'headline.md').write_text('\n'.join(head)+'\n')
