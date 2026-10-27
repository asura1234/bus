//! Provider argv execution through the configured shell's startup files.
use super::spawn::{
    pane_shell, resolve_shell_for_login_mode, shell_mode_uses_login_shell, PaneShellConfig,
    ShellLaunchTarget,
};
use std::{io, path::Path};

/// Run rc files in the configured interactive/login shell, then replace it with
/// the provider. Positional arguments keep shell metacharacters out of the code.
pub(crate) fn agent_exec_argv(
    shell_config: PaneShellConfig<'_>,
    provider: &[String],
) -> io::Result<Option<Vec<String>>> {
    let shell = pane_shell(shell_config.default_shell);
    let name = Path::new(&shell).file_name().and_then(|name| name.to_str());
    if !cfg!(unix) || !matches!(name, Some("zsh" | "bash" | "sh" | "dash" | "ksh")) {
        return Ok(None);
    }
    let login = shell_mode_uses_login_shell(shell_config.mode, ShellLaunchTarget::current());
    let mut argv = vec![if login {
        resolve_shell_for_login_mode(&shell)?
    } else {
        shell
    }];
    if login {
        argv.push("-l".into());
    }
    argv.extend([
        "-i".into(),
        "-c".into(),
        "exec \"$@\"".into(),
        "bus-provider-launch".into(),
    ]);
    argv.extend_from_slice(provider);
    Ok(Some(argv))
}
