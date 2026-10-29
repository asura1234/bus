# Recipient menu area and scrolling

## Run contract

Use a fresh scratch Bus through this worktree's dev-tools `bus --dev tui`,
120x30, graphics none. Build only through `/tmp/bus-877-slot.sh`, using this
worktree's single target. Never attach to the live Bus, use desktop automation,
or launch real providers. CLI fixtures may create rooms and fake Claude agents
and post messages to you. All menu interactions and room switches use TUI input.
No skips or failed-step retries. Save snapshots and inspect cell screenshots for
every numbered step. Names with a numeric suffix use two digits.

### ATMENU-01 — MASTER long list owns its area

Setup: create 24 work rooms `menu-work-00` through `menu-work-23`, each with its
own fake Claude orchestrator `menu-orch-00` through `menu-orch-23` in MASTER.
Use each agent's own directory beneath the driver's scratch work directory.
Create one work room `menu-short` with two fake Claude workers `short-00` and
`short-01`. Post 45 numbered lines beginning `History` to you in MASTER from
menu-orch-00. Select MASTER and wait for History to appear before opening @.

1. Click the composer's @ hit `recipients`. Assert a complete bordered menu,
   an All row and several distinct agent rows above the composer. The menu
   erases history inside its rectangle and stays inside the terminal.
2. Wheel down over a menu row until the last orchestrator is visible. Assert
   the highlight and menu window advance while history stays pinned. Toggle
   menu-orch-23 by mouse. Assert its checked row and recipient chip. Press Up
   then Space; assert menu-orch-22 also becomes checked and the frame stays whole.
3. Wheel history outside the menu's horizontal bounds, then post `History NEW`
   as menu-orch-00 to you while the menu stays open. Assert its frame, rows and
   checked selections remain intact. Close with Esc and assert readable history
   and both recipient chips; reopen with Ctrl+P.
4. Resize to 42x14, then 40x10. At each size press Down to the last row and toggle
   it off then on. Assert the highlighted row remains visible, the frame stays
   within the terminal, and text does not bleed through. Resize back to 120x30.
5. Close the menu, scroll the sidebar until menu-short is visible, and switch
   to menu-short. Open Ctrl+P. Assert All and exactly
   the two workers appear inside a complete short menu despite the prior long
   list's highlight. Select short-00 and close; assert only its recipient chip.

### ATMENU-02 — Long work-room list and expanded composer

Setup: create work room `menu-long` with 24 fake Claude workers `long-00` through
`long-23`, using distinct directories beneath the driver's scratch work directory.
Post 45 History lines to you from long-00 and select menu-long. Wait for History.

1. Open Ctrl+P. Wheel down over the menu to long-23 and toggle it using Space.
   Assert a full bordered viewport, last row visible and checked, with no history
   text inside. Press Up and Down; assert the highlight stays visible.
2. Close the menu, paste 40 lines of `Draft preserved` into the composer, and
   reopen Ctrl+P. Assert the picker overlays the expanded composer with a full
   frame and visible choices. Toggle long-22 through its visible row. Close;
   assert the draft and both recipient chips remain intact.
3. Reopen, resize to 42x14 and navigate to the final row. Assert the frame and
   checked rows remain inside the terminal. Close, restore 120x30 and assert
   the draft is unchanged. Stop the scratch session.
