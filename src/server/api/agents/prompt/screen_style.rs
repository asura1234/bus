//! ANSI SGR cell styling and multi-row composer text matching helpers.

pub(super) struct StyledChar {
    pub(super) character: char,
    pub(super) faint: bool,
    pub(super) background: bool,
}

pub(super) fn matches_rows(rows: &[String], expected: &str) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    let Some(rest) = expected.strip_prefix(first) else {
        return false;
    };
    let mut positions = vec![expected.len() - rest.len()];
    for row in &rows[1..] {
        let mut next = Vec::new();
        for position in positions {
            let rest = &expected[position..];
            for separator in ["\n", " ", ""] {
                if let Some(after) = rest
                    .strip_prefix(separator)
                    .and_then(|tail| tail.strip_prefix(row))
                {
                    next.push(expected.len() - after.len());
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        if next.is_empty() {
            return false;
        }
        positions = next;
    }
    positions.contains(&expected.len())
}

/// Retain the styles that distinguish suggestions and composer chrome.
pub(super) fn styled_chars(line: &str) -> Vec<StyledChar> {
    let mut out = Vec::new();
    let (mut dim, mut inverse, mut background) = (false, false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(StyledChar {
                character: c,
                faint: dim || inverse,
                background,
            });
            continue;
        }
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                let mut last = None;
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        last = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if last == Some('m') {
                    apply_sgr(&params, &mut dim, &mut inverse, &mut background);
                }
            }
            // OSC (hyperlinks): skip to BEL or ST.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

pub(super) fn plain_screen(screen: &str) -> String {
    screen
        .lines()
        .map(|line| {
            styled_chars(line)
                .iter()
                .map(|c| c.character)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_sgr(params: &str, dim: &mut bool, inverse: &mut bool, background: &mut bool) {
    let params: Vec<&str> = params.split(';').collect();
    let mut index = 0;
    while index < params.len() {
        match params[index] {
            "" | "0" => (*dim, *inverse, *background) = (false, false, false),
            "2" => *dim = true,
            "22" => *dim = false,
            "7" => *inverse = true,
            "27" => *inverse = false,
            "49" => *background = false,
            // Extended colors carry their own numbers (38;2;R;G;B or 38;5;N),
            // which must not read as dim or inverse.
            "38" | "48" | "58" => {
                if params[index] == "48" {
                    *background = true;
                }
                index += match params.get(index + 1) {
                    Some(&"5") => 2,
                    Some(&"2") => 4,
                    _ => 0,
                }
            }
            value
                if value.parse::<u8>().ok().is_some_and(|value| {
                    (40..=47).contains(&value) || (100..=107).contains(&value)
                }) =>
            {
                *background = true;
            }
            _ => {}
        }
        index += 1;
    }
}
