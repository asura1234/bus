use crate::platform::windows::process::peb::command_line_to_argv;
use std::path::PathBuf;

pub(crate) fn remote_reattach_program(program: &str) -> String {
    let path = std::env::current_exe()
        .ok()
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(program));
    format!(
        "& {}",
        remote_reattach_argument(&path.display().to_string())
    )
}

pub(in crate::platform::windows) fn remote_reattach_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(crate) fn interactive_shell_command(argv: &[String], shell_name: &str) -> Option<String> {
    let shell_name = shell_name.to_ascii_lowercase();
    let powershell = shell_name.contains("powershell") || shell_name.contains("pwsh");
    let script = powershell_agent_script(argv)?;
    if powershell {
        Some(script)
    } else {
        Some(cmd_encoded_powershell_command(&script))
    }
}

pub(in crate::platform::windows) fn powershell_agent_script(argv: &[String]) -> Option<String> {
    let (program, args) = argv.split_first()?;
    if args.is_empty() {
        return Some(format!(
            "& {}",
            crate::platform::quote_powershell_arg(program)
        ));
    }

    let powershell_args = args
        .iter()
        .map(|arg| crate::platform::quote_powershell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let command_line = args
        .iter()
        .map(|arg| quote_windows_command_line_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!(
        "if((Get-Command {} -ErrorAction SilentlyContinue).CommandType -eq 'ExternalScript'){{& {} {}}}else{{Start-Process -FilePath {} -ArgumentList {} -NoNewWindow -Wait}}",
        crate::platform::quote_powershell_arg(program),
        crate::platform::quote_powershell_arg(program),
        powershell_args,
        crate::platform::quote_powershell_arg(program),
        crate::platform::quote_powershell_arg(&command_line),
    ))
}

pub(in crate::platform::windows) fn quote_windows_command_line_arg(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|ch| matches!(ch, ' ' | '\t' | '\n' | '\x0b' | '"'))
    {
        return value.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
        }
        backslashes = 0;
        quoted.push(ch);
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

pub(in crate::platform::windows) fn cmd_encoded_powershell_command(script: &str) -> String {
    use base64::Engine as _;

    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!("powershell.exe -NoLogo -NoProfile -EncodedCommand {encoded}")
}

pub(crate) fn scrollback_editor_argv(path: &std::path::Path) -> std::io::Result<Vec<String>> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    scrollback_editor_argv_with_env(path, editor.as_deref())
}

pub(in crate::platform::windows) fn scrollback_editor_argv_with_env(
    path: &std::path::Path,
    editor: Option<&str>,
) -> std::io::Result<Vec<String>> {
    let mut argv = match editor.filter(|value| !value.trim().is_empty()) {
        Some(editor) => command_line_to_argv(editor).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("failed to parse editor command {editor:?}"),
            )
        })?,
        None => vec!["notepad.exe".to_string()],
    };
    if argv.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "editor command must not be empty",
        ));
    }
    argv.push(path.display().to_string());
    Ok(argv)
}
