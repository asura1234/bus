//! Shared notification title/body parsing.

pub fn split_message(message: &str) -> (&str, Option<&str>) {
    match message.split_once(": ") {
        Some((title, body)) if !title.is_empty() && !body.is_empty() => (title, Some(body)),
        _ => (message, None),
    }
}

#[cfg(test)]
#[path = "notification/tests/split_test.rs"]
mod tests;
