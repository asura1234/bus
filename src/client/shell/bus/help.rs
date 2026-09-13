/// Client-only help. These commands never become agent prompts.
pub(super) const TEXT: &str = "Composer
Enter          Send to checked agents
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
Ctrl+C         Clear the draft; quits only when it is empty
Mouse drag     Select notes, history or draft text and copy it
@              Choose agents (Shift+2 on US keyboards)
+ / Ctrl+F     Add files (Shift+= on US keyboards)
Ctrl+Shift+E   Toggle full-height / compact composer
Page Up/Down   Scroll the draft without moving the caret
Mouse wheel    Scroll sidebar, history, recipients or draft under the pointer
F3             Switch notes / composer
/help + Enter  Open this guide locally

Recipient menu
Up / Down      Move through agents
Space / Enter  Check or uncheck an agent
Esc / Tab      Close the picker

Rooms and agents
Ctrl+Shift+R   Add a room
Ctrl+N         Add an agent
Ctrl+T         Review pending hook setup from the room
F2             Rename selected room or agent
Double-click   Rename a room or agent name
× beside name  Delete room / agent after confirmation
Esc / Enter    Cancel / OK in the delete warning
F6             Return from terminal to room

Forms
Tab / Shift+Tab  Move fields
Up / Down      Select path suggestions or agent type
Tab            Complete a selected path
Enter          Add agent / room; complete or add file
Ctrl+Enter     Confirm form
Esc            Cancel

Quit
Ctrl+C / Ctrl+Q Save and quit (retry if saving failed)
Ctrl+Shift+Q   Force quit only after an unsaved warning";
