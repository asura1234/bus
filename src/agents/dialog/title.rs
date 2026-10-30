//! The question text above a live option list.
use super::{is_separator, unboxed, MAX_TITLE_LINES};

/// The question above the options, up to a separator or a double blank line.
pub(super) fn title(above: &[&str]) -> String {
    // Approval notices must retain every command line and the Reason above it.
    let limit = if above
        .iter()
        .any(|line| unboxed(line).trim().starts_with("$ "))
    {
        usize::MAX
    } else {
        MAX_TITLE_LINES
    };
    let mut collected = Vec::new();
    let mut blanks = 0;
    for line in above.iter().rev() {
        let text = unboxed(line).trim();
        if is_separator(text) {
            break;
        }
        if text.is_empty() {
            blanks += 1;
            if blanks == 2 && !collected.is_empty() {
                break;
            }
            continue;
        }
        if blanks > 0 && !collected.is_empty() {
            collected.push("");
        }
        blanks = 0;
        collected.push(text);
        if collected.iter().filter(|line| !line.is_empty()).count() == limit {
            break;
        }
    }
    collected.reverse();
    collected.join("\n")
}
