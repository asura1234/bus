/// Client-only help. These commands never become agent prompts.
pub(super) const TEXT: &str = "Composer
Enter          Send; a working agent takes it into its current turn
Option+Enter   Send to wait for each agent's next turn of its own
Shift+Enter    New line
Ctrl+J         New line (legacy terminal fallback)
\\ then Enter   New line in any terminal
Ctrl+A / Ctrl+E Start / end of the current line
Option+B / F   Back / forward one word (also Option+←/→)
Up / Down      Move between rows, then browse sent prompts
Ctrl+U         Delete to start of line; again clears lines above
Cmd+Backspace  Same as Ctrl+U
Ctrl+K         Delete to end of line
Ctrl+W         Delete back to the previous space (whole path)
Option+Delete  Delete the previous word
Option+D       Delete to the end of the word
Ctrl+_         Undo the last edit
Ctrl+Y         Paste back the last Ctrl+K / Ctrl+U / Ctrl+W deletion
Option+Y       After Ctrl+Y, cycle earlier deletions
Esc Esc        Clear the draft and save it so Up brings it back
Ctrl+R         Search prompt history
Ctrl+S         Stash the current prompt; empty composer restores it
Ctrl+G         Open the prompt in $EDITOR
Ctrl+V         Attach a clipboard image
Ctrl+C         Clear the draft (never quits Bus)
Mouse drag     Select notes, history or draft text and copy it
Ctrl+P         Choose agents
Ctrl+F         Add files
Ctrl+Shift+E   Toggle full-height / compact composer
Page Up/Down   Scroll the draft, or the notes while editing (F3)
Mouse wheel    Scroll sidebar, notes, history, recipients or draft under the pointer
F3             Switch notes / composer
/help + Enter  Open this guide locally

Recipient menu
Up / Down      Move through agents
Space / Enter  Check or uncheck an agent
Esc / Tab      Close the picker

Rooms and agents
Ctrl+Shift+R   Add a room
Ctrl+N         Add an agent
F2             Rename selected room or agent
Double-click   Rename a room or agent name
× beside name  Delete open room / any agent after confirmation
Click #room    On a MASTER agent, reassign or unassign the room it
               orchestrates; click its name to open its terminal
Esc / Enter    Cancel / OK in the delete warning
F6             Return from terminal to room
Ctrl+C         In an agent terminal, interrupt the agent

Forms
Tab / Shift+Tab  Move fields
Up / Down      Select path suggestions or agent type
Tab            Complete a selected path
Enter          Add agent / room; complete or add file
               (in a MASTER agent's system prompt, add a line)
Ctrl+Enter     Confirm form
Esc            Cancel

Quit
Ctrl+Q         Save, stop the agents and the server, quit
Ctrl+Shift+Q   Force quit only after an unsaved warning";
