# Grok login captures

Visible screens from the room 8259 investigation, captured with native Grok
Build 1.0.70 on macOS. `native-1.0.70-error.txt` comes from
`/private/tmp/inv-grok-p0vv01o_/report.json`; `native-1.0.70-connecting.txt`
comes from `/private/tmp/inv-grok-jorzrjv4/report.json`. Only the disposable
directory name and trailing whitespace were normalized. Both had the idle
OSC title `grok`; neither had emitted SessionStart. The error screen also
retains an empty composer. No login keys were sent by the investigator.

`native-1.0.70-error-71col.txt` is the native 71x30 `agent read --source visible`
capture supplied by gl-rev-cl for PR #268. Its authentication URL wraps over
two rows. That rendering probe used a disposable HOME and a dead local proxy;
it does not claim real-account acceptance. The directory name and trailing
whitespace were normalized as above.

`native-1.0.70-error-61col.txt` was captured by the owner with real HOME,
private Bus data and the same dead proxy. The exact-head native probe reported
Idle at `cc6138da66b3016546542c4e4c242dd27109a178` on that wrapped login screen.
Both scratch processes' build identities matched the copied binary. No login
keys were sent, and the scratch server and agent were closed afterward.
