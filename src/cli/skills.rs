//! Local skill commands never discover or contact a running Bus.
use crate::messaging::skills;
use std::io::{self, BufRead, IsTerminal, Write};

const USAGE: &str = "Usage: bus skills status\n       bus skills install [--yes]\nCopies default skills to ~/.claude/skills and ~/.agents/skills.\n--yes records consent without a prompt. Existing user skills are preserved.";

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
    let report = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => skills::status(&home)?,
        ["install", "--yes"] => skills::install(&home)?,
        ["install"] => {
            if !io::stdin().is_terminal() {
                return Err(io::Error::other(
                    "bus skills install needs a terminal; pass --yes for scripted consent",
                ));
            }
            offer(&home, &mut io::stdin().lock(), &mut io::stderr().lock())?;
            skills::status(&home)?
        }
        _ => return Err(io::Error::other(USAGE)),
    };
    for warning in &report.warnings {
        eprintln!("Warning: {warning}");
    }
    super::help::write_stdout_line(format_args!("{}", serde_json::to_string(&report)?));
    Ok(())
}

pub(super) fn on_launch() {
    if std::env::var("BUS_SKIP_SKILLS_OFFER").is_ok_and(|value| value == "1")
        || !io::stdin().is_terminal()
        || !io::stderr().is_terminal()
    {
        return;
    }
    let result = (|| {
        let home = skills::home()?;
        if skills::needs_offer(&home)? {
            let _lease = skills::offer_lease(&home)?;
            if !skills::needs_offer(&home)? {
                return Ok(());
            }
            offer(&home, &mut io::stdin().lock(), &mut io::stderr().lock())?;
        }
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = result {
        eprintln!("Warning: Bus skills: {error}. Run bus skills install to try again.");
    }
}

fn offer(
    home: &std::path::Path,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<()> {
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
            return Ok(());
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => {
                let report = skills::install(home)?;
                for warning in report.warnings {
                    writeln!(output, "Warning: {warning}")?;
                }
                writeln!(
                    output,
                    "Bus skills install finished. Run bus skills status to inspect copied or skipped skills."
                )?;
                return Ok(());
            }
            "" | "n" | "no" => {
                skills::record_answer(home, false)?;
                return Ok(());
            }
            _ => writeln!(output, "Enter yes or no.")?,
        }
    }
}

#[cfg(test)]
#[path = "tests/skills_test.rs"]
mod tests;
