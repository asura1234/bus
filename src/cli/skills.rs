//! Local skill commands never discover or contact a running Bus.
use crate::messaging::skills;
use std::io::{self, BufRead, IsTerminal, Write};

const USAGE: &str = "Usage: bus skills status\n       bus skills install [--yes]\nCopies default skills to ~/.claude/skills and ~/.agents/skills.\n--yes installs without a prompt. Existing user skills are preserved.\nEach copy's action is installed, updated, unchanged, preserved-conflict, or blocked-link.";

pub(super) fn run(args: &[String]) -> io::Result<()> {
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        super::help::write_stdout_line(format_args!("{USAGE}"));
        return Ok(());
    }
    let home = skills::home()?;
    let (report, warnings_printed) = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => (skills::status(&home)?, false),
        ["install", "--yes"] => (skills::install(&home)?, false),
        ["install"] => {
            if !io::stdin().is_terminal() {
                return Err(io::Error::other(
                    "bus skills install needs a terminal; pass --yes to install without a prompt",
                ));
            }
            match offer(&home, &mut io::stdin().lock(), &mut io::stderr().lock())? {
                // The prompt already wrote this report's warnings to the terminal.
                Some(report) => (report, true),
                None => (skills::status(&home)?, false),
            }
        }
        _ => return Err(io::Error::other(USAGE)),
    };
    if !warnings_printed {
        for warning in &report.warnings {
            eprintln!("Warning: {warning}");
        }
    }
    super::help::write_stdout_line(format_args!("{}", serde_json::to_string(&report)?));
    Ok(())
}

fn offer(
    home: &std::path::Path,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<skills::Status>> {
    writeln!(output, "Install the following set of default skills?")?;
    for name in skills::skill_names() {
        writeln!(output, "  - {name}")?;
    }
    writeln!(
        output,
        "Copies go to ~/.claude/skills and ~/.agents/skills; user skills are preserved."
    )?;
    loop {
        write!(output, "Install? [y/N] ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => {
                let report = skills::install(home)?;
                for warning in &report.warnings {
                    writeln!(output, "Warning: {warning}")?;
                }
                writeln!(
                    output,
                    "Bus skills install finished. Run bus skills status to inspect copied or skipped skills."
                )?;
                return Ok(Some(report));
            }
            "" | "n" | "no" => return Ok(None),
            _ => writeln!(output, "Enter yes or no.")?,
        }
    }
}

#[cfg(test)]
#[path = "tests/skills_test.rs"]
mod tests;
