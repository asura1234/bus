//! Private startup shims, sourced only by shells Bus launches. User files stay
//! untouched and run before the prompt hook is installed.
use portable_pty::CommandBuilder;
use std::io;
use std::path::{Path, PathBuf};

pub(super) struct ShellIntegration {
    directory: PathBuf,
}

impl Drop for ShellIntegration {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl ShellIntegration {
    pub(super) fn install(cmd: &mut CommandBuilder, shell: &str) -> io::Result<Option<Self>> {
        if !cfg!(unix) {
            return Ok(None);
        }
        let name = Path::new(shell).file_name().and_then(|name| name.to_str());
        if !matches!(name, Some("zsh" | "bash")) {
            return Ok(None);
        }
        let directory = std::env::temp_dir().join(format!(
            "bus-shell-{}-{}",
            std::process::id(),
            crate::utils::time::now_ns()
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let integration = Self { directory };
        if name == Some("zsh") {
            integration.zsh(cmd)?;
        } else {
            integration.bash(cmd, shell)?;
        }
        Ok(Some(integration))
    }

    fn zsh(&self, cmd: &mut CommandBuilder) -> io::Result<()> {
        let original = cmd.get_env("ZDOTDIR").map(std::ffi::OsStr::to_os_string);
        cmd.env(
            "__BUS_ZDOTDIR_SET",
            if original.is_some() { "1" } else { "0" },
        );
        cmd.env("__BUS_ZDOTDIR", original.unwrap_or_default());
        cmd.env("__BUS_SHELL_DIR", &self.directory);
        cmd.env("ZDOTDIR", &self.directory);
        for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            let mut script = String::from(ZSH_RESTORE);
            if file == ".zshenv" {
                // These parameters are private to the initial shell, including
                // while the user's startup code spawns children.
                script.push_str("typeset +x __BUS_ZDOTDIR_SET __BUS_ZDOTDIR __BUS_SHELL_DIR\n");
            }
            script.push_str(&format!("if [[ -r ${{ZDOTDIR-$HOME}}/{file} ]]; then source \"${{ZDOTDIR-$HOME}}/{file}\"; fi\n"));
            match file {
                ".zshrc" => {
                    script.push_str("if [[ ! -o login ]]; then\n");
                    script.push_str(ZSH_PROMPT);
                    script.push_str("else\n");
                    script.push_str(ZSH_SAVE);
                    script.push_str("fi\n");
                }
                ".zlogin" => script.push_str(ZSH_PROMPT),
                _ => script.push_str(ZSH_SAVE),
            }
            std::fs::write(self.directory.join(file), script)?;
        }
        Ok(())
    }

    fn bash(&self, cmd: &mut CommandBuilder, shell: &str) -> io::Result<()> {
        // Interactive login bash ignores --rcfile. Run its normal login files
        // from the rcfile instead, with login_shell set before they execute.
        let login = cmd.is_default_prog();
        let rcfile = self.directory.join("bashrc");
        std::fs::write(
            &rcfile,
            format!(
                "{}{}",
                if login { BASH_LOGIN } else { BASH_RC },
                BASH_PROMPT
            ),
        )?;
        let argv = cmd.get_argv_mut();
        argv.clear();
        argv.extend([
            shell.into(),
            "--rcfile".into(),
            rcfile.into_os_string(),
            "-i".into(),
        ]);
        Ok(())
    }
}

const ZSH_RESTORE: &str = r#"if [[ $__BUS_ZDOTDIR_SET == 1 ]]; then
    ZDOTDIR=$__BUS_ZDOTDIR
else
    unset ZDOTDIR
fi
"#;
const ZSH_SAVE: &str = r#"__BUS_ZDOTDIR_SET=${+ZDOTDIR}
__BUS_ZDOTDIR=${ZDOTDIR-}
ZDOTDIR=$__BUS_SHELL_DIR
"#;
const ZSH_PROMPT: &str = r#"function __bus_prompt_ready() {
    local __bus_status=$?
    builtin printf '\033]133;A\007'
    return $__bus_status
}
typeset -ga precmd_functions
precmd_functions=(${precmd_functions:#__bus_prompt_ready} __bus_prompt_ready)
"#;
const BASH_RC: &str = r#"if [[ -r $HOME/.bashrc ]]; then source "$HOME/.bashrc"; fi
"#;
const BASH_LOGIN: &str = r#"shopt -s login_shell
if [[ -r /etc/profile ]]; then source /etc/profile; fi
for __bus_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
    if [[ -r $__bus_profile ]]; then source "$__bus_profile"; break; fi
done
unset __bus_profile
"#;
const BASH_PROMPT: &str = r#"__bus_prompt_ready() {
    local __bus_status=$?
    builtin printf '\033]133;A\007'
    return "$__bus_status"
}
if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) &&
    [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
    PROMPT_COMMAND+=(__bus_prompt_ready)
else
    PROMPT_COMMAND="${PROMPT_COMMAND-}"$'\n''__bus_prompt_ready'
fi
"#;
