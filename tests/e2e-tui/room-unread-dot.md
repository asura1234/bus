# Unseen messages to You in the room sidebar

## Run contract

Fresh scratch Bus through the own-worktree dev-tools TUI driver, 120x40,
graphics none. Use only fake providers and recorded effects; never restart or
connect to the live Bus. CLI fixtures may create rooms/agents and post incoming
agent-authored messages, plus the provider bootstrap prompts in setup and the
developer fixtures in steps 2 and 5. Step 2 may create and delete its disposable
fake worker as fixture setup/cleanup. Room opening, terminal opening, F6 and resizing use
real driver input. Read-only state/history corroborate the screen. Capture and
open cell screenshots for layout, and inspect cells for the green dot's color.
No skips or failed-step retries. `restart` quits and relaunches only this
driver-owned scratch Bus, retaining its data and trace.

### DOT-01 — Developer unread dots persist, clear on opening, and fit narrow names

Preconditions: create work rooms `dot-very-long-room-name` and `dot-other` with
the scratch CLI. Add fake Claude workers `dot-worker` and `dot-peer` to the long-named room,
using the session's scratch work directory and consenting to their hooks. Wait
for both to be idle. Send each one a developer `DOT bootstrap` prompt with the
scratch CLI and wait for both fake replies, so both provider conversations have
completed a turn before restart. Record IDs and select `dot-other` through the sidebar.

1. Fixture: send `DOT peer` as dot-worker to dot-peer in the long-named
   room and wait for its fake reply to settle. Assert its `unread_count` is
   positive but `unseen_to_developer` is false and no sidebar dot appears.
2. Fixture: send `DOT decision` as dot-worker to `you` in the long-named room
   and `DOT MASTER decision` as dot-worker to `human` in MASTER. Assert both
   are posted with no requests, both rooms report `unseen_to_developer: true`,
   and both sidebar names end with one green `●`. Inspect its actual cell
   foreground (`#66ff66`), and capture/open a screenshot. The other room stays
   visible. Resize to 60x40: the long name truncates and its green dot remains
   visible before the status; MASTER also keeps its dot. Capture/open a screenshot.
   Fixture: add a disposable fake Claude worker `dot-working` to the long-named
   room using the scratch workspace and hook consent. Wait until it is idle,
   send `fake:work 30 DOT narrow working` as the developer to dot-working,
   wait until Working, then resize to 40x40. Assert the work-room dot is still
   visible and green beside Working and MASTER also retains its dot. Capture/open
   a screenshot. Delete only the disposable dot-working fixture with the scratch
   CLI and `--confirm`, assert it is absent from state and the unseen marker stays
   true, then return to 60x40. Its request may close abandoned during cleanup;
   no provider continuation in the narrow pane is under test.
3. Click the long-named room's sidebar entry. Assert the decision and its You
   recipient display, the room's dot and marker clear, and MASTER's remain.
   Fixture: send `DOT visible` to you in the visible room; assert it displays
   without setting its marker or dot.
4. Click `agent:dot-worker` to open its terminal. Fixture: send `DOT terminal`
   to you in the long-named room; assert its marker and sidebar dot set because
   the chat is hidden. Press F6: the room opens, the message displays, and its
   marker and dot clear. This covers keyboard opening as well as sidebar clicks.
5. Click `dot-other`. Fixture: send `DOT restart` to you in the long-named
   room. Record state and process ID, then use the scratch driver's `restart`.
   Assert a new Bus process with the same scratch data directory, both work-room
   and MASTER markers/dots retained, and the default first work room (`bus`)
   visible after startup. Wait for both fake agents to be idle, then send a
   developer fixture `DOT after restart` to dot-peer in the long-named room
   and wait for its fake reply. Assert no session invalidation and both unseen
   markers retained. Capture/open
   a screenshot of the persisted dots.
6. Click MASTER and assert its decision displays with a You recipient and its
   marker/dot clear. Click the long-named room and assert its marker/dot clear.
   Click dot-other and restart again. Assert both markers remain false and both
   dots remain absent. Capture state and a screenshot, then stop the session.
