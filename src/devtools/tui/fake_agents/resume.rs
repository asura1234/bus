//! Session identity for the existing fake provider resume commands.
use super::{FakeAgent, Value};

impl FakeAgent {
    pub(super) fn next_turn(&mut self) -> String {
        self.turns += 1;
        super::pseudo_uuid()
    }

    pub(super) fn from_args(provider: &str, hooks: Value, args: &[String]) -> Self {
        let mut agent = Self::new(provider, hooks);
        let resumed = args
            .iter()
            .enumerate()
            .take_while(|(_, arg)| *arg != "--")
            .find_map(|(index, arg)| {
                if arg == "--resume" || provider == "codex" && arg == "resume" {
                    args.get(index + 1).map(String::as_str)
                } else {
                    arg.strip_prefix("--resume=")
                }
            })
            .filter(|session| !session.is_empty() && !session.starts_with('-'));
        if let Some(session) = resumed {
            agent.session = session.to_owned();
        }
        agent
    }
}
