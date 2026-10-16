//! Argument parsing for the driver verbs, and `--help` sections cut from
//! `docs/tui-driver.md` so the help and the docs cannot drift.
use super::protocol::{Command, Region, Style, Target};
use std::time::Duration;

/// Flags that take no value.
const SWITCHES: &[&str] = &[
    "--regex",
    "--gone",
    "--double",
    "--plain",
    "--real",
    "--changed",
    "--all",
    "--style",
    "--yolo",
];

#[derive(Debug, Default)]
pub(super) struct Parsed {
    values: Vec<(String, String)>,
    switches: Vec<String>,
    positionals: Vec<String>,
}

impl Parsed {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            // After `--` everything is text, so `type -- --model` types it.
            if arg == "--" {
                parsed.positionals.extend(iter.cloned());
                break;
            }
            if SWITCHES.contains(&arg.as_str()) {
                parsed.switches.push(arg.clone());
            } else if arg.starts_with("--") && arg.len() > 2 {
                let value = iter.next().ok_or_else(|| format!("{arg} needs a value"))?;
                parsed.values.push((arg.clone(), value.clone()));
            } else {
                parsed.positionals.push(arg.clone());
            }
        }
        Ok(parsed)
    }

    pub fn take_value(&mut self, flag: &str) -> Option<String> {
        let index = self.values.iter().position(|(k, _)| k == flag)?;
        Some(self.values.remove(index).1)
    }

    pub fn take_switch(&mut self, flag: &str) -> bool {
        match self.switches.iter().position(|k| k == flag) {
            Some(index) => {
                self.switches.remove(index);
                true
            }
            None => false,
        }
    }

    pub fn positional(&mut self) -> Option<String> {
        (!self.positionals.is_empty()).then(|| self.positionals.remove(0))
    }

    pub fn positionals(&mut self) -> Vec<String> {
        std::mem::take(&mut self.positionals)
    }

    /// Fails on anything left over, so typos never pass silently.
    pub fn finish(&self) -> Result<(), String> {
        let mut extra: Vec<String> = self.values.iter().map(|(k, _)| k.clone()).collect();
        extra.extend(self.switches.iter().cloned());
        extra.extend(self.positionals.iter().map(|p| format!("{p:?}")));
        if extra.is_empty() {
            Ok(())
        } else {
            Err(format!("unexpected argument(s): {}", extra.join(", ")))
        }
    }
}

pub(super) fn parse_size(text: &str) -> Result<(u16, u16), String> {
    let (cols, rows) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("size {text:?} must look like 160x45"))?;
    let cols: u16 = cols
        .parse()
        .map_err(|_| format!("bad columns in {text:?}"))?;
    let rows: u16 = rows.parse().map_err(|_| format!("bad rows in {text:?}"))?;
    if !(40..=500).contains(&cols) || !(10..=200).contains(&rows) {
        return Err(format!("size {text:?} out of range (40-500 x 10-200)"));
    }
    Ok((cols, rows))
}

/// `500ms`, `10s`, `2m`, `7d`, or a bare number of milliseconds.
pub(super) fn parse_duration(text: &str) -> Result<Duration, String> {
    let bad = || format!("duration {text:?} must look like 500ms, 10s, 2m or 7d");
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().map_err(|_| bad())?;
    let millis = match unit {
        "" | "ms" => number,
        "s" => number * 1000,
        "m" => number * 60_000,
        "h" => number * 3_600_000,
        "d" => number * 86_400_000,
        _ => return Err(bad()),
    };
    Ok(Duration::from_millis(millis))
}

/// `3-10` or `7`: an inclusive range of screen rows.
pub(super) fn parse_rows(text: &str) -> Result<(u16, u16), String> {
    let bad = || format!("rows {text:?} must look like 3-10 or 7");
    match text.split_once('-') {
        Some((a, b)) => {
            let a: u16 = a.trim().parse().map_err(|_| bad())?;
            let b: u16 = b.trim().parse().map_err(|_| bad())?;
            if a > b {
                return Err(bad());
            }
            Ok((a, b))
        }
        None => {
            let row: u16 = text.trim().parse().map_err(|_| bad())?;
            Ok((row, row))
        }
    }
}

fn number(text: &str, what: &str) -> Result<u16, String> {
    text.parse()
        .map_err(|_| format!("{what} must be a number, got {text:?}"))
}

fn rows_flag(parsed: &mut Parsed) -> Result<Option<(u16, u16)>, String> {
    parsed
        .take_value("--rows")
        .map(|rows| parse_rows(&rows))
        .transpose()
}

/// A target from `--hit NAME`, `--text T [--regex] [--nth N] [--rows A-B]`,
/// else `ROW COL`. `--nth` counts matches from 1 in reading order.
fn target(parsed: &mut Parsed, text_flag: &str) -> Result<Target, String> {
    if let Some(name) = parsed.take_value("--hit") {
        return Ok(Target::Hit { name });
    }
    if let Some(text) = parsed.take_value(text_flag) {
        let nth = parsed
            .take_value("--nth")
            .map(|n| match n.parse::<usize>() {
                Ok(n) if n >= 1 => Ok(n),
                _ => Err(format!(
                    "--nth {n:?}: give the match number counting from 1 (1 = first, in reading order)"
                )),
            })
            .transpose()?;
        return Ok(Target::Text {
            text,
            regex: parsed.take_switch("--regex"),
            nth,
            rows: rows_flag(parsed)?,
        });
    }
    cell_target(parsed)
}

/// A `ROW COL` target.
fn cell_target(parsed: &mut Parsed) -> Result<Target, String> {
    let row = parsed
        .positional()
        .ok_or("give ROW COL, --text TEXT or --hit NAME")?;
    let col = parsed
        .positional()
        .ok_or("give ROW COL, --text TEXT or --hit NAME")?;
    Ok(Target::Cell {
        row: number(&row, "ROW")?,
        col: number(&col, "COL")?,
    })
}

/// Builds the host request for the screen and input verbs.
pub(super) fn command(verb: &str, parsed: &mut Parsed) -> Result<Command, String> {
    let command = match verb {
        "status" => Command::Status,
        "snapshot" => Command::Snapshot {
            rows: rows_flag(parsed)?,
        },
        "find" => Command::Find {
            text: parsed
                .positional()
                .ok_or("usage: find TEXT [--regex] [--rows A-B]")?,
            regex: parsed.take_switch("--regex"),
            rows: rows_flag(parsed)?,
        },
        "cells" => {
            let mut numbers = Vec::new();
            for value in parsed.positionals() {
                numbers.push(number(&value, "cells argument")?);
            }
            match numbers.as_slice() {
                [row, col] => Command::Cells {
                    row: *row,
                    col: *col,
                    width: 1,
                    height: 1,
                },
                [row, col, width, height] => Command::Cells {
                    row: *row,
                    col: *col,
                    width: *width,
                    height: *height,
                },
                _ => return Err("usage: cells ROW COL [WIDTH HEIGHT]".into()),
            }
        }
        "type" => Command::Type {
            text: parsed.positional().ok_or("usage: type TEXT")?,
        },
        "paste" => Command::Paste {
            text: parsed.positional().ok_or("usage: paste TEXT")?,
        },
        "press" => {
            let keys = parsed.positionals();
            if keys.is_empty() {
                return Err("usage: press KEY [KEY ...], e.g. press ctrl+n".into());
            }
            Command::Press { keys }
        }
        "click" | "drag" | "scroll" | "hover" => pointer_command(verb, parsed)?,
        "resize" => {
            let (cols, rows) = parse_size(&parsed.positional().ok_or("usage: resize COLSxROWS")?)?;
            Command::Resize { cols, rows }
        }
        "clipboard" => Command::Clipboard,
        "screenshot" => Command::Screenshot {
            real: parsed.take_switch("--real"),
            out: parsed.take_value("--out").map(std::path::PathBuf::from),
        },
        "hits" => Command::Hits,
        "effects" => Command::Effects {
            kind: parsed.take_value("--kind"),
        },
        "focus" => Command::Focus {
            gained: match parsed.positional().as_deref() {
                Some("in") => true,
                Some("out") => false,
                _ => return Err("usage: focus in|out".into()),
            },
        },
        _ => return Err(format!("unknown verb {verb:?}; run `bus --dev tui --help`")),
    };
    parsed.finish()?;
    Ok(command)
}

/// Verbs that act at a screen position.
fn pointer_command(verb: &str, parsed: &mut Parsed) -> Result<Command, String> {
    Ok(match verb {
        "click" => Command::Click {
            double: parsed.take_switch("--double"),
            button: parsed
                .take_value("--button")
                .unwrap_or_else(|| "left".into()),
            mods: parsed.take_value("--mods").unwrap_or_default(),
            target: target(parsed, "--text")?,
        },
        "drag" => {
            let button = parsed
                .take_value("--button")
                .unwrap_or_else(|| "left".into());
            let mods = parsed.take_value("--mods").unwrap_or_default();
            let (from, to) = match (
                parsed.take_value("--from-text"),
                parsed.take_value("--to-text"),
            ) {
                (Some(from), Some(to)) => {
                    let regex = parsed.take_switch("--regex");
                    let text = |text: String| Target::Text {
                        text,
                        regex,
                        nth: None,
                        rows: None,
                    };
                    (text(from), text(to))
                }
                (None, None) => (cell_target(parsed)?, cell_target(parsed)?),
                _ => return Err("give both --from-text and --to-text".into()),
            };
            Command::Drag {
                from,
                to,
                button,
                mods,
            }
        }
        "hover" => Command::Hover {
            target: target(parsed, "--text")?,
        },
        "scroll" => {
            let (up, notches) = match (parsed.take_value("--up"), parsed.take_value("--down")) {
                (Some(n), None) => (true, number(&n, "--up")?),
                (None, Some(n)) => (false, number(&n, "--down")?),
                _ => return Err("give exactly one of --up N or --down N".into()),
            };
            Command::Scroll {
                mods: parsed.take_value("--mods").unwrap_or_default(),
                target: target(parsed, "--text")?,
                up,
                notches,
            }
        }
        _ => return Err(format!("{verb} is not a pointer verb")),
    })
}

fn region(parsed: &mut Parsed) -> Result<Region, String> {
    Ok(Region {
        rows: rows_flag(parsed)?,
        cols: parsed
            .take_value("--cols")
            .map(|cols| parse_rows(&cols))
            .transpose()?,
    })
}

fn names(list: Option<String>) -> Vec<String> {
    list.map(|list| {
        list.split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// Host waits: text, stable or changed regions, cell style, or an effect.
pub(super) fn wait_command(parsed: &mut Parsed, timeout: Duration) -> Result<Command, String> {
    let timeout_ms = timeout.as_millis() as u64;
    let command = if let Some(kind) = parsed.take_value("--effect") {
        Command::WaitEffect {
            kind,
            target: parsed.take_value("--target"),
            all: parsed.take_switch("--all"),
            timeout_ms,
        }
    } else if parsed.take_switch("--style") {
        let style = Style {
            fg: parsed.take_value("--fg"),
            bg: parsed.take_value("--bg"),
            attrs: names(parsed.take_value("--attrs")),
            no_attrs: names(parsed.take_value("--no-attrs")),
        };
        if style.fg.is_none()
            && style.bg.is_none()
            && style.attrs.is_empty()
            && style.no_attrs.is_empty()
        {
            return Err("--style needs --fg, --bg, --attrs or --no-attrs".into());
        }
        Command::WaitStyle {
            target: target(parsed, "--text")?,
            style,
            timeout_ms,
        }
    } else if let Some(text) = parsed.take_value("--text") {
        Command::WaitText {
            text,
            regex: parsed.take_switch("--regex"),
            gone: parsed.take_switch("--gone"),
            rows: rows_flag(parsed)?,
            timeout_ms,
        }
    } else if let Some(stable) = parsed.take_value("--stable") {
        Command::WaitStable {
            stable_ms: parse_duration(&stable)?.as_millis() as u64,
            region: region(parsed)?,
            timeout_ms,
        }
    } else if parsed.take_switch("--changed") {
        Command::WaitChanged {
            region: region(parsed)?,
            timeout_ms,
        }
    } else {
        return Err(
            "usage: wait --text T | --stable 300ms | --changed | --style TARGET --fg C | --effect KIND | --state 'PRED' | --message ID"
                .into(),
        );
    };
    parsed.finish()?;
    Ok(command)
}

/// The doc up to the first `### VERB` section.
pub(super) fn help_overview(doc: &str) -> String {
    doc.lines()
        .take_while(|line| !line.starts_with("### "))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The `### VERB` section of the doc, up to the next heading.
pub(super) fn help_for(doc: &str, verb: &str) -> Option<String> {
    let heading = format!("### {verb}");
    let mut lines = doc.lines().skip_while(|line| {
        line.split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
            != heading
    });
    let first = lines.next()?;
    let mut out = format!("{first}\n");
    for line in lines.take_while(|line| !line.starts_with("#")) {
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
#[path = "tests/args_test.rs"]
mod tests;
