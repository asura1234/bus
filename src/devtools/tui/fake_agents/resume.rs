//! Preserve provider identity across scratch Bus restarts.
use super::{FakeAgent, Value};

impl FakeAgent {
    pub(super) fn from_args(provider: &str, hooks: Value, args: &[String]) -> Self {
        let mut agent = Self::new(provider, hooks);
        agent.resume(args);
        agent
    }

    pub(super) fn resume(&mut self, args: &[String]) {
        let resumed = args
            .iter()
            .enumerate()
            .take_while(|(_, arg)| *arg != "--")
            .find_map(|(index, arg)| {
                if arg == "--resume" || self.codex() && arg == "resume" {
                    args.get(index + 1).map(String::as_str)
                } else {
                    arg.strip_prefix("--resume=")
                }
            })
            .filter(|session| !session.is_empty() && !session.starts_with('-'));
        if let Some(session) = resumed {
            self.session = session.to_owned();
            // A new process must not reuse a previously settled turn.
            self.turns = crate::devtools::tui::effects::now_ms();
        }
    }
}
