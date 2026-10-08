use super::system_prompt::{prompt_args, prompt_message};
use std::path::Path;

#[test]
fn cursor_fresh_and_adopted_launches_require_the_first_prompt_message() {
    for adopted in [false, true] {
        assert_eq!(
            prompt_args(Path::new("/tmp/system-prompt.md"), adopted).unwrap(),
            None
        );
    }
}

#[test]
fn cursor_first_prompt_message_preserves_the_authored_text_verbatim() {
    let text = "Room: work\n\nKeep this prompt. {{already-authored}}";
    assert_eq!(prompt_message(text), format!(
        "Bus: this is your system prompt; Bus could not set it at launch for this provider or session. Follow it for the rest of this session, then reply briefly that you are ready.\n\n{text}"
    ));
}
