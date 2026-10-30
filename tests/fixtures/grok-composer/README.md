# Grok composer fixtures

`empty.txt`, `draft.txt` and `pasted.txt` are synthetic normalized frames.
The box, `❯ Type a message...` placeholder and `Shift+Tab:mode │ Ctrl+x:shortcuts`
footer are informed by the committed `native-1.0.5-offline.ansi` capture;
these three text files are not direct captures.
The version-matching keyboard guide documents Ctrl+X as the non-Kitty shortcut.

`empty.txt` represents an empty boxed composer. `draft.txt` demonstrates editable
numbered instructions. `pasted.txt` is an explicitly hypothetical collapsed chip;
its exact rendering and count semantics have no authenticated evidence.

Only a complete current composer frame with the normal-mode footer is readable.
The welcome-card layout is the exception: its frame ends on the model-labelled
lower border (`╰` … `Grok <model>`) and has no footer beneath it.
A live card, parked card, modal, incomplete frame or login is unavailable. Plain
unrelated drafts return Unavailable: the shared runtime clears Other drafts,
so that observation would overwrite developer input. Only exact visible text
is an owned pending prompt.
Only CRLF line endings are normalized; indentation and visible line breaks must
match exactly. Ambiguous soft wrapping remains unavailable. A faint placeholder
is empty; the same literal text without placeholder styling is a developer draft.
A collapsed paste chip returns Unavailable: matching the hidden text by count
alone would permit Enter to submit an unrelated draft of the same length. Real
long-paste observation/ownership needs an actual capture and an attestable shape.

## Native offline capture

`native-1.0.5-offline.ansi` is the byte-identical archived Grok Build 1.0.5
fullscreen offline built-ins capture (120 columns, 40 rows). SHA-256:
`9a1264095bd419c3385d2f52387064e0137f2bf154668619213a51fcd4115cdc`.
The model label begins at byte 7171. Tests replay through the synchronized
frame end after the first `:shortcuts` footer, when the composer is fully
ready; subsequent built-in modal interactions and shutdown remain intact. No model was run for this capture.

This proves a normal composer whose bottom border contains the model label.
It does not establish authenticated cards, final completion, long-paste chips,
or native background wake. The same normal frame must be accepted by both the
input observer and the dialog exclusion parser. Ownership guards still apply.
