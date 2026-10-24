# Unseen messages to You in the room sidebar

## Run contract

Fresh scratch Bus through the own-worktree dev-tools TUI driver, 120x40,
graphics none. Use only fake providers and recorded effects; never restart or
connect to the live Bus. CLI fixtures may create rooms/agents and post incoming
agent-authored messages. Room opening, terminal opening, F6 and resizing use
real driver input. Read-only state/history corroborate the screen. Capture and
open cell screenshots for layout, and inspect cells for the green dot's color.
No skips or failed-step retries. `restart` quits and relaunches only this
driver-owned scratch Bus, retaining its data and trace.

### DOT-01 — Developer unread dots persist, clear on opening, and fit narrow names

Preconditions: create work rooms `dot-very-long-room-name` and `dot-other` with
the scratch CLI. Add fake Claude worker `dot-worker` to the long-named room,
using the session's scratch work directory and consenting to its hooks. Wait
for it to be idle. Record IDs and select `dot-other` through the sidebar.

1. Fixture: send `DOT peer` as dot-worker to dot-worker in the long-named
   room and wait for its fake reply to settle. Assert its `unread_count` is
   positive but `unseen_to_developer` is false and no sidebar dot appears.
2. Fixture: send `DOT decision` as dot-worker to `you` in the long-named room
   and `DOT MASTER decision` as dot-worker to `human` in MASTER. Assert both
   are posted with no requests, both rooms report `unseen_to_developer: true`,
   and both sidebar names end with one green `●`. Inspect its actual cell
   foreground (`#66ff66`), and capture/open a screenshot. The other room stays
   visible. Resize to 60x40: the long name truncates and its green dot remains
   visible before the status; MASTER also keeps its dot. Capture/open a screenshot.
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
   visible after startup. Capture/open
   a screenshot of the persisted dots.
6. Click MASTER and assert its decision displays with a You recipient and its
   marker/dot clear. Click the long-named room and assert its marker/dot clear.
   Click dot-other and restart again. Assert both markers remain false and both
   dots remain absent. Capture state and a screenshot, then stop the session.
