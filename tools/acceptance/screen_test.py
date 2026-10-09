"""Acceptance capture decoding and visible reply-card assertions; no live IO."""
import re

ROWS, COLS, SIDEBAR = 40, 120, 28

def terminal_frame(raw):
    """Read Bus's absolute-cell capture; this is not the app's renderer."""
    screen = [[" "] * COLS for _ in range(ROWS)]
    x = y = i = 0
    text = raw.decode("utf-8", errors="replace")
    while i < len(text):
        c = text[i]
        if c == "\x1b":
            if text[i:i + 2] == "\x1b[":
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", text[i:])
                if not match:
                    break
                params, _, final = match.groups()
                values = [int(v) if v.isdigit() else 0 for v in params.split(";")]
                n = values[0] or 1
                if final in "Hf":
                    y = max(0, (values[0] or 1) - 1)
                    x = max(0, ((values[1] if len(values) > 1 else 1) or 1) - 1)
                elif final == "G": x = n - 1
                elif final == "A": y = max(0, y - n)
                elif final == "B": y = min(ROWS - 1, y + n)
                elif final == "C": x = min(COLS - 1, x + n)
                elif final == "D": x = max(0, x - n)
                elif final == "J" and values[0] in (2, 3):
                    screen = [[" "] * COLS for _ in range(ROWS)]
                elif final == "K" and y < ROWS:
                    start = 0 if values[0] in (1, 2) else x
                    end = x + 1 if values[0] == 1 else COLS
                    for column in range(start, min(end, COLS)):
                        screen[y][column] = " "
                i += len(match[0])
                continue
            if text[i:i + 2] in ("\x1b]", "\x1bP", "\x1b_", "\x1b^"):
                end = re.search("\x07|\x1b\\\\", text[i + 2:])
                if not end:
                    break
                i += 2 + end.end()
                continue
            i += 2
            continue
        if c == "\r": x = 0
        elif c == "\n": y = min(ROWS - 1, y + 1)
        elif c == "\b": x = max(0, x - 1)
        elif ord(c) >= 32:
            if y < ROWS and x < COLS:
                screen[y][x] = c
            x += 1
        i += 1
    return "\n".join("".join(row).rstrip() for row in screen) + "\n"


def assert_reply_cards(frame, names, token):
    lines = [line[SIDEBAR:].strip() for line in frame.splitlines()]
    for name in names:
        assert any(
            lines[index].startswith(name + "  ")
            and lines[index + 1] == token
            and lines[index + 2] == "Quote"
            for index in range(len(lines) - 2)
        ), f"Final reply card for {name} is not visible in room"
