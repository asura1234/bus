use super::*;

/// The provider must start without entering ZLE/readline or sending a single key.
/// The prompt deliberately consumes one byte before drawing, reproducing odex/laude
/// if a launch is submitted as terminal input while precmd is running.
#[tokio::test]
async fn direct_launch_keeps_login_environment_and_bypasses_slow_prompt_input() {
    use crate::protocol::api::schema::{SuccessResponse, TabCreateParams};
    use std::os::unix::fs::PermissionsExt;
    for shell in ["/bin/zsh", "/bin/bash"] {
        if !std::path::Path::new(shell).exists() {
            continue;
        }
        for mode in [
            crate::utils::config::ShellModeConfig::Login,
            crate::utils::config::ShellModeConfig::NonLogin,
        ] {
            let dir = crate::utils::test_temp::unique_temp_path("bus-direct-launch");
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::write(dir.join(".zshenv"), "export FIXTURE_ENV=env\n").unwrap();
            std::fs::write(dir.join(".zprofile"), "export FIXTURE_PROFILE=profile\n").unwrap();
            std::fs::write(dir.join(".zlogin"), "export FIXTURE_LOGIN=login\n").unwrap();
            std::fs::write(dir.join(".zshrc"), "export PATH=\"$ZDOTDIR/bin:/usr/bin:/bin\"\nexport FIXTURE_RC=rc\nprecmd() { print PROMPT_CONSUMING_INPUT; read -r -k 1 stolen; sleep 2; }\nPS1='ready> '\n").unwrap();
            std::fs::write(dir.join(".bashrc"), "export PATH=\"$HOME/bin:/usr/bin:/bin\"\nexport FIXTURE_ENV=env FIXTURE_RC=rc\nPROMPT_COMMAND='printf PROMPT_CONSUMING_INPUT; read -r -n 1 stolen; sleep 2'\n").unwrap();
            std::fs::write(
                dir.join(".bash_profile"),
                "export FIXTURE_PROFILE=profile FIXTURE_LOGIN=login\nsource \"$HOME/.bashrc\"\n",
            )
            .unwrap();
            for provider in ["codex", "claude"] {
                let file = dir.join("bin").join(provider);
                std::fs::write(&file, "#!/bin/sh\nprintf '%s|%s|%s|%s\\n' \"$FIXTURE_ENV\" \"$FIXTURE_PROFILE\" \"$FIXTURE_RC\" \"$FIXTURE_LOGIN\" > \"$ZDOTDIR/environment\"\nprintf '%s\\n' \"$@\" > \"$ZDOTDIR/arguments\"\nprintf 'DIRECT_PROVIDER_RUNNING\\n'\nread -r unused\n").unwrap();
                std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
                let mut app = app_with_agent();
                app.state.default_shell = shell.into();
                app.state.shell_mode = mode;
                let params: TabCreateParams = serde_json::from_value(serde_json::json!({
                "cwd": dir, "focus": false,
                "env": {"HOME":dir,"ZDOTDIR":dir,"PATH":"/usr/bin:/bin","BUS_LAUNCH_ID":"direct-fixture"},
                "agent_launch": {"name":"direct-fixture","kind":provider,"args":["one two","quote'and;$()","中文"]}
            })).unwrap();
                let response = app.handle_tab_create("direct".into(), params);
                let success: SuccessResponse = serde_json::from_str(&response).unwrap();
                let ResponseResult::TabCreated {
                    root_pane,
                    agent_started,
                    ..
                } = success.result
                else {
                    panic!("{response}")
                };
                assert!(
                    agent_started,
                    "provider must already own the tab; typing another launch would duplicate it"
                );
                let runtime = app.terminal_runtimes.values().next().unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while !runtime.visible_text().contains("DIRECT_PROVIDER_RUNNING") {
                    if std::time::Instant::now() >= deadline {
                        panic!("Provider requires prompt input: {}", runtime.visible_text());
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let expected = if mode == crate::utils::config::ShellModeConfig::Login {
                    "env|profile|rc|login\n"
                } else {
                    "env||rc|\n"
                };
                assert_eq!(
                    std::fs::read_to_string(dir.join("environment")).unwrap(),
                    expected
                );
                assert!(std::fs::read_to_string(dir.join("arguments"))
                    .unwrap()
                    .starts_with("one two\nquote'and;$()\n中文\n"));
                assert!(!runtime.visible_text().contains("PROMPT_CONSUMING_INPUT"));
                assert!(!root_pane.terminal_id.is_empty());
                drop(app);
                std::fs::remove_file(dir.join("environment")).unwrap();
                std::fs::remove_file(dir.join("arguments")).unwrap();
            }
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
