//! First-turn text for a provider that cannot take the prompt at launch.
use super::Provider;
use crate::agents::providers::cursor;
use crate::messaging::orchestration as orchestrator;

pub(super) fn first_message_text(
    provider: Provider,
    is_orchestrator: bool,
    instructions: &str,
) -> String {
    match provider {
        Provider::Grok if is_orchestrator => {
            crate::agents::providers::grok::system_prompt::prompt_message(instructions)
        }
        Provider::Cursor | Provider::Grok => cursor::system_prompt::prompt_message(instructions),
        _ => orchestrator::prompt_message(instructions),
    }
}
