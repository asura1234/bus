/// Client-only help. These commands never become agent prompts.
pub(super) const TEXT: &str = "Composer
Enter          Send to checked agents
Shift+Enter    New line
Ctrl+J         New line (legacy terminal fallback)
@              Choose agents (Shift+2 on US keyboards)
+ / Ctrl+F     Add files (Shift+= on US keyboards)
Ctrl+E         Toggle full-height / compact composer
Page Up/Down   Scroll the draft without moving the caret
Mouse wheel    Scroll sidebar, history, recipients or draft under the pointer
F3             Switch notes / composer
/help + Enter  Open this guide locally

Recipient menu
Up / Down      Move through agents
Space / Enter  Check or uncheck an agent
Esc / Tab      Close the picker

Rooms and agents
Ctrl+R         Add a room
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
