pub(crate) mod diagnostic;
pub(crate) mod feedback;
pub(crate) mod prof;
pub(crate) mod signal;
pub(crate) mod widgets;

// Keep the original test identity after exposing the physical feedback owner.
#[cfg(test)]
mod status_popups {
    mod tests {
        include!("feedback/tests/feedback_test.rs");
    }
}
