use super::prompt_args;
use std::path::Path;

#[test]
fn claude_adapter_fresh_prompt_uses_the_file_path_without_reading_or_shell_quoting_it() {
    let path = Path::new("/not-created/a path/it's prompt.md");
    assert_eq!(
        prompt_args(path, false),
        vec![
            "--append-system-prompt-file".to_owned(),
            path.to_string_lossy().into_owned()
        ]
    );
}

#[test]
fn claude_adapter_adopted_prompt_disables_snapshot_before_appending_the_file() {
    assert_eq!(
        prompt_args(Path::new("/tmp/prompt.md"), true),
        vec![
            "--system-prompt-snapshot".to_owned(),
            "off".to_owned(),
            "--append-system-prompt-file".to_owned(),
            "/tmp/prompt.md".to_owned(),
        ]
    );
}
