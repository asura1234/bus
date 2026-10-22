//! Selection highlights read from a screen's ANSI styling.

/// How one screen character is drawn, as far as selection highlights go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Style {
    bold: bool,
    inverse: bool,
    fg: Option<u32>,
    bg: Option<u32>,
}

impl Style {
    /// Drawn on a background of its own, as a selection or active tab is.
    pub(super) fn is_filled(&self) -> bool {
        self.inverse || self.bg.is_some()
    }
}

/// The option drawn unlike the others: the only inverse or filled one, or the
/// only one styled differently from options that all agree. With two options
/// the other one must be plain, or the highlight is ambiguous.
pub(super) fn highlighted(styles: &[Style]) -> Option<usize> {
    let filled: Vec<usize> = (0..styles.len())
        .filter(|&index| styles[index].is_filled())
        .collect();
    if !filled.is_empty() {
        return (filled.len() == 1).then(|| filled[0]);
    }
    let mut candidates = (0..styles.len()).filter(|&index| {
        let mut others = styles
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, style)| style);
        let Some(first) = others.next() else {
            return false;
        };
        others.all(|style| style == first)
            && *first != styles[index]
            && (styles.len() >= 3 || *first == Style::default())
    });
    let found = candidates.next();
    candidates.next().is_none().then_some(found).flatten()
}

/// Splits a screen into plain lines and the style of each of their characters,
/// following SGR sequences and skipping every other escape sequence.
pub(super) fn styled_lines(screen: &str) -> (Vec<String>, Vec<Vec<Style>>) {
    let mut texts = Vec::new();
    let mut styles = Vec::new();
    // Keep the current line directly, so even an empty screen has a line and
    // character appends never depend on a fallible last-element lookup.
    let mut text = String::new();
    let mut line_styles = Vec::new();
    let mut style = Style::default();
    let mut chars = screen.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            if c == 'm' {
                                apply_sgr(&mut style, &params);
                            }
                            break;
                        }
                        params.push(c);
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' => {
                texts.push(std::mem::take(&mut text));
                styles.push(std::mem::take(&mut line_styles));
            }
            '\r' => {}
            c => {
                text.push(c);
                line_styles.push(style);
            }
        }
    }
    texts.push(text);
    styles.push(line_styles);
    (texts, styles)
}

fn apply_sgr(style: &mut Style, params: &str) {
    let mut codes = params.split(';').peekable();
    if params.is_empty() {
        *style = Style::default();
    }
    while let Some(code) = codes.next() {
        // Colon form: one parameter such as `38:2::255:0:0` or `48:5:236`.
        if code.contains(':') {
            let parts: Vec<u32> = code
                .split(':')
                .filter_map(|part| part.parse().ok())
                .collect();
            let color = match parts.as_slice() {
                [_, 5, index] => Some(*index),
                [_, 2, .., r, g, b] => Some(0x100_0000 | r << 16 | g << 8 | b),
                _ => None,
            };
            match parts.first() {
                Some(38) => style.fg = color,
                Some(48) => style.bg = color,
                _ => {}
            }
            continue;
        }
        let extended = |codes: &mut std::iter::Peekable<std::str::Split<'_, char>>| {
            let mut next = || codes.next().and_then(|part| part.parse::<u32>().ok());
            match next() {
                Some(5) => next(),
                Some(2) => {
                    let (r, g, b) = (next()?, next()?, next()?);
                    Some(0x100_0000 | r << 16 | g << 8 | b)
                }
                _ => None,
            }
        };
        match code.parse::<u32>().unwrap_or(0) {
            0 => *style = Style::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            7 => style.inverse = true,
            27 => style.inverse = false,
            code @ (30..=37 | 90..=97) => style.fg = Some(code),
            39 => style.fg = None,
            code @ (40..=47 | 100..=107) => style.bg = Some(code),
            49 => style.bg = None,
            38 => style.fg = extended(&mut codes),
            48 => style.bg = extended(&mut codes),
            _ => {}
        }
    }
}
