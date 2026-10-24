//! Preserve provider identity across scratch Bus restarts.
use super::FakeAgent;

impl FakeAgent {
    pub(super) fn resume(&mut self, args: &[String]) {
        let flag = if self.codex() { "resume" } else { "--resume" };
        if let Some(session) = args
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| &pair[1])
            .filter(|session| !session.is_empty() && !session.starts_with('-'))
        {
            self.session.clone_from(session);
            // A new process must not reuse turn 1 of the same session: Bus
            // durably remembers which provider turns already settled.
            self.turns = crate::devtools::tui::effects::now_ms();
        }
    }
}
