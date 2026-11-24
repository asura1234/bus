use super::{drain_input, FakeAgent, Typed};

/// Native composer transitions from the same fake used by the TUI driver.
pub(crate) struct FakeClaudeComposer(FakeAgent);

impl FakeClaudeComposer {
    pub(crate) fn new() -> Self {
        Self(FakeAgent::new("claude", serde_json::Value::Null))
    }

    pub(crate) fn input(&mut self, bytes: &[u8]) -> Option<String> {
        drain_input(&mut bytes.to_vec())
            .into_iter()
            .filter_map(|input| self.0.apply(input))
            .find_map(|result| match result {
                Typed::Submit(text) => Some(text),
                _ => None,
            })
    }

    pub(crate) fn body(&self) -> &str {
        &self.0.draft
    }

    pub(crate) fn shown(&self) -> String {
        self.0.shown_draft()
    }

    pub(crate) fn stashed(&self) -> Option<&str> {
        self.0.stashed.as_ref().map(|(text, _)| text.as_str())
    }
}
