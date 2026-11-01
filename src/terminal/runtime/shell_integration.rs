//! Private prompt/editor hooks installed only when Bus creates a pane.
use portable_pty::CommandBuilder;
use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

mod other_shells;

static NEXT_INTEGRATION: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
#[path = "tests/shell_integration_test.rs"]
mod tests;

#[cfg_attr(test, derive(Debug))]
pub(super) struct ShellIntegration {
    directory: PathBuf,
    token: String,
    shell: String,
    windows: bool,
    started_at: Instant,
    process: Option<crate::platform::ProcessInstance>,
    initial_image: Option<PathBuf>,
    observed_image: RefCell<Option<PathBuf>>,
    prompt_floor: Cell<u64>,
}

impl Drop for ShellIntegration {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl ShellIntegration {
    pub(super) fn install_for_agent_launch(
        cmd: &mut CommandBuilder,
        launch_env: &super::spawn::PaneLaunchEnv,
    ) -> io::Result<Option<Self>> {
        Self::install_for_target(cmd, launch_env, super::spawn::ShellLaunchTarget::current())
    }

    pub(super) fn install_for_target(
        cmd: &mut CommandBuilder,
        launch_env: &super::spawn::PaneLaunchEnv,
        target: super::spawn::ShellLaunchTarget,
    ) -> io::Result<Option<Self>> {
        // Only a pane created with Bus-owned identity or explicit launch identity
        // opts in; never inject into an arbitrary existing shell.
        let agent_launch = launch_env
            .extra
            .iter()
            .any(|(key, value)| key == "BUS_LAUNCH_ID" && !value.is_empty());
        let owned_pane = matches!(
            &launch_env.identity,
            super::spawn::PaneLaunchIdentity::Managed { .. }
        );
        if !owned_pane && !agent_launch {
            return Ok(None);
        }
        let shell = if cmd.is_default_prog() {
            cmd.get_shell()
        } else {
            cmd.get_argv()
                .first()
                .and_then(|arg| arg.to_str())
                .unwrap_or_default()
                .to_owned()
        };
        let name = shell
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let powershell = super::spawn::is_powershell_shell(&shell);
        if !matches!(name.as_str(), "zsh" | "bash" | "fish") && !powershell {
            if agent_launch {
                tracing::warn!(event = "bus.agent.unsupported_shell", shell = %shell,
                    "Agent launch rejected before spawning an unsupported shell");
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("Cannot launch an agent in shell {shell:?}: Bus shell integration requires zsh, bash, fish or PowerShell. Configure terminal.default_shell to a supported shell; no launch input was sent"),
                ));
            }
            return Ok(None);
        }
        let token = format!(
            "{:08x}{:032x}{:016x}",
            std::process::id(),
            crate::utils::time::now_ns(),
            NEXT_INTEGRATION.fetch_add(1, Ordering::Relaxed)
        );
        let directory = std::env::temp_dir().join(format!("bus-shell-{token}"));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "create shell integration at {}: {error}",
                    directory.display()
                ),
            )
        })?;
        let initial_image = cmd
            .get_cwd()
            .map(Path::new)
            .filter(|cwd| cwd.is_dir())
            .or_else(|| cmd.get_env("HOME").map(Path::new))
            .and_then(|cwd| {
                crate::platform::resolve_executable_image(
                    Path::new(&shell),
                    cmd.get_env("PATH"),
                    cwd,
                )
            });
        let started_at = Instant::now();
        let integration = Self {
            directory,
            token,
            shell: name.clone(),
            windows: target == super::spawn::ShellLaunchTarget::Windows,
            started_at,
            process: None,
            initial_image,
            observed_image: RefCell::new(None),
            prompt_floor: Cell::new(0),
        };
        match name.as_str() {
            "zsh" => integration.zsh(cmd)?,
            "bash" => integration.bash(cmd, &shell)?,
            "fish" => integration.fish(cmd, &shell)?,
            _ => integration.powershell(cmd, &shell)?,
        }
        Ok(Some(integration))
    }

    pub(super) fn start_wait(&mut self, pid: Option<u32>) {
        self.started_at = Instant::now();
        self.process = pid.and_then(crate::platform::process_instance);
    }

    fn image(pid: Option<u32>) -> Option<(crate::platform::ProcessInstance, PathBuf)> {
        let pid = pid?;
        let process = crate::platform::process_instance(pid)?;
        let image = crate::platform::process_image(pid)?;
        (crate::platform::process_instance(pid) == Some(process)).then_some((process, image))
    }

    pub(super) fn token(&self) -> &str {
        &self.token
    }

    pub(super) fn invalidate_prompt(&self, generation: u64) {
        self.prompt_floor
            .set(self.prompt_floor.get().max(generation));
    }

    pub(super) fn waiting_for_prompt(&self, pid: Option<u32>, generation: u64) -> bool {
        if self.windows {
            let matches = pid.is_some_and(|pid| {
                self.process.is_some()
                    && crate::platform::process_instance(pid) == self.process
                    && crate::platform::available_pane_shell(pid)
                        .is_some_and(|name| super::spawn::is_powershell_shell(&name))
            });
            if !matches {
                self.invalidate_prompt(generation);
                return true;
            }
            return generation <= self.prompt_floor.get();
        }
        let Some((process, image)) = Self::image(pid) else {
            self.invalidate_prompt(generation);
            return true;
        };
        if Some(process) != self.process {
            self.invalidate_prompt(generation);
            return true;
        }
        let mut previous = self.observed_image.borrow_mut();
        // portable-pty may return while the child still has Bus's fork image.
        // Bind only once the OS confirms the resolved initial shell. Its
        // launch-specific prompt may already have arrived by then.
        if previous.is_none() && self.initial_image.as_ref() != Some(&image) {
            self.invalidate_prompt(generation);
            return true;
        }
        if previous.as_ref().is_some_and(|previous| previous != &image) {
            *previous = Some(image);
            self.invalidate_prompt(generation);
            return true;
        }
        *previous = Some(image);
        generation <= self.prompt_floor.get()
    }

    pub(super) fn wait_expired(&self) -> bool {
        self.started_at.elapsed() >= crate::protocol::api::AGENT_START_TIMEOUT
    }

    pub(super) fn editor_probe(&self, pid: Option<u32>) -> super::shell_launch::EditorProbe {
        super::shell_launch::EditorProbe {
            directory: self.directory.clone(),
            token: self.token.clone(),
            shell: self.shell.clone(),
            floor: self.prompt_floor.get(),
            pid,
            process: self.process,
            image: self.initial_image.clone(),
            windows: self.windows,
        }
    }

    fn script(&self, script: &str) -> String {
        let quote = |name: &str| {
            if super::spawn::is_powershell_shell(&self.shell) {
                return format!(
                    "'{}'",
                    self.directory
                        .join(name)
                        .to_string_lossy()
                        .replace('\'', "''")
                );
            }
            format!(
                "'{}'",
                self.directory
                    .join(name)
                    .to_string_lossy()
                    .replace('\'', "'\"'\"'")
            )
        };
        script
            .replace("__BUS_PROMPT_TOKEN__", &self.token)
            .replace("__BUS_REQUEST__", &quote("request"))
            .replace("__BUS_CAPTURE__", &quote("capture"))
            .replace("__BUS_CAPTURE_TMP__", &quote("capture.tmp"))
    }

    fn fish(&self, cmd: &mut CommandBuilder, shell: &str) -> io::Result<()> {
        let login = cmd.is_default_prog();
        let script = self.directory.join("integration.fish");
        std::fs::write(&script, self.script(other_shells::FISH))?;
        let source = format!(
            "source '{}'",
            script
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
        );
        let argv = cmd.get_argv_mut();
        argv.clear();
        argv.push(shell.into());
        if login {
            argv.push("--login".into());
        }
        argv.extend([
            "--interactive".into(),
            "--init-command".into(),
            source.into(),
        ]);
        Ok(())
    }

    fn powershell(&self, cmd: &mut CommandBuilder, shell: &str) -> io::Result<()> {
        let existing = cmd
            .get_argv()
            .windows(2)
            .find(|args| args[0].to_string_lossy().eq_ignore_ascii_case("-Command"))
            .map(|args| args[1].to_string_lossy().into_owned())
            .unwrap_or_default();
        let script = self.script(other_shells::POWERSHELL);
        let script = if existing.is_empty() {
            script
        } else {
            format!("{existing}; {script}")
        };
        let argv = cmd.get_argv_mut();
        argv.clear();
        argv.extend([
            shell.into(),
            "-NoExit".into(),
            "-Command".into(),
            script.into(),
        ]);
        Ok(())
    }

    fn zsh(&self, cmd: &mut CommandBuilder) -> io::Result<()> {
        let original = cmd.get_env("ZDOTDIR").map(std::ffi::OsStr::to_os_string);
        cmd.env(
            "__BUS_ZDOTDIR_SET",
            if original.is_some() { "1" } else { "0" },
        );
        cmd.env("__BUS_ZDOTDIR", original.unwrap_or_default());
        cmd.env("ZDOTDIR", &self.directory);
        std::fs::write(self.directory.join(".zshenv"), self.script(ZSH_ENV))
    }

    fn bash(&self, cmd: &mut CommandBuilder, shell: &str) -> io::Result<()> {
        // --rcfile starts a non-login interactive bash. In login mode we source
        // its profile files manually; this does not set login_shell or run
        // .bash_logout. Bus-created panes use this existing limited emulation.
        let login = cmd.is_default_prog();
        let rcfile = self.directory.join("bashrc");
        std::fs::write(
            &rcfile,
            format!(
                "{}{}",
                if login { BASH_LOGIN } else { BASH_RC },
                self.script(BASH_PROMPT)
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

const ZSH_ENV: &str = r#"if [[ $__BUS_ZDOTDIR_SET == 1 ]]; then
    ZDOTDIR=$__BUS_ZDOTDIR
else
    unset ZDOTDIR
fi
unset __BUS_ZDOTDIR_SET __BUS_ZDOTDIR
if [[ -r ${ZDOTDIR-$HOME}/.zshenv ]]; then source "${ZDOTDIR-$HOME}/.zshenv"; fi
function __bus_capture_editor() {
    [[ $CONTEXT == start ]] || return
    local __bus_challenge
    IFS= builtin read -r __bus_challenge < __BUS_REQUEST__ || return
    { builtin printf '%s\n' "$__bus_challenge"; builtin printf '%s' "$BUFFER"; } > __BUS_CAPTURE_TMP__
    command /bin/mv -f __BUS_CAPTURE_TMP__ __BUS_CAPTURE__
}
function __bus_clear_editor() {
    [[ $CONTEXT == start ]] || return
    BUFFER=''; CURSOR=0
}
function __bus_bind_editor() {
    zle -N __bus_capture_editor
    zle -N __bus_clear_editor
    local __bus_map
    for __bus_map in emacs viins vicmd; do
        bindkey -M "$__bus_map" $'\e[__BUS_PROMPT_TOKEN__v~' __bus_capture_editor
        bindkey -M "$__bus_map" $'\e[__BUS_PROMPT_TOKEN__c~' __bus_clear_editor
    done
}
function __bus_arm() {
    local __bus_rc=$?
    sched +0 __bus_install_line_init
    return $__bus_rc
}
function __bus_line_init() {
    local __bus_rc=$?
    if [[ $CONTEXT != start ]]; then
        if (( __bus_had_line_init )); then
            () { return $__bus_rc }
            zle __bus_saved_line_init -- "$@"
            return $?
        fi
        return $__bus_rc
    fi
    # Restore before delegating, so nesting cannot recurse through this wrapper.
    if (( __bus_had_line_init )); then
        zle -A __bus_saved_line_init zle-line-init
        () { return $__bus_rc }
        zle __bus_saved_line_init -- "$@"
        __bus_rc=$?
        zle -D __bus_saved_line_init
    else
        zle -D zle-line-init
    fi
    precmd_functions=(${precmd_functions:#__bus_arm} __bus_arm)
    __bus_bind_editor
    builtin printf '\033]133;A;bus=__BUS_PROMPT_TOKEN__\007'
    unset __bus_had_line_init
    return $__bus_rc
}
function __bus_active_editor() {
    # Invoked as a widget: CONTEXT is available even when sched itself lacks it.
    if [[ $CONTEXT != start ]]; then
        sched +1 __bus_install_line_init
        return 0
    fi
    # A timer ran after primary line-init completed: do not call the user's
    # line-init again. Its editor is positively waiting for primary input.
    precmd_functions=(${precmd_functions:#__bus_arm} __bus_arm)
    __bus_bind_editor
    builtin printf '\033]133;A;bus=__BUS_PROMPT_TOKEN__\007'
}
function __bus_install_line_init() {
    local __bus_install_rc=$?
    zmodload zsh/zle 2>/dev/null || return $__bus_install_rc
    if zle; then
        zle -N __bus_check_context __bus_active_editor
        zle __bus_check_context
        zle -D __bus_check_context
        return $__bus_install_rc
    fi
    typeset -g __bus_had_line_init=0
    if zle -l zle-line-init >/dev/null 2>&1; then
        zle -A zle-line-init __bus_saved_line_init
        __bus_had_line_init=1
    fi
    zle -N zle-line-init __bus_line_init
    return $__bus_install_rc
}
if zmodload zsh/sched 2>/dev/null; then
    sched +0 __bus_install_line_init
fi
"#;
const BASH_RC: &str = r#"if [[ -r $HOME/.bashrc ]]; then source "$HOME/.bashrc"; fi
"#;
const BASH_LOGIN: &str = r#"if [[ -r /etc/profile ]]; then source /etc/profile; fi
for __bus_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
    if [[ -r $__bus_profile ]]; then source "$__bus_profile"; break; fi
done
unset __bus_profile
"#;
const BASH_PROMPT: &str = r#"__bus_capture_editor() {
    COMPREPLY=()
    local __bus_challenge __bus_prefix='__bus_probe '
    [[ $COMP_LINE == "$__bus_prefix"* ]] || return
    IFS= builtin read -r __bus_challenge < __BUS_REQUEST__ || return
    { builtin printf '%s\n' "$__bus_challenge"; builtin printf '%s' "${COMP_LINE:${#__bus_prefix}}"; } > __BUS_CAPTURE_TMP__
    command /bin/mv -f __BUS_CAPTURE_TMP__ __BUS_CAPTURE__
}
__bus_bind_editor() {
    # COMP_LINE is available in Bash 3.2 too; READLINE_LINE is not. A private
    # completion macro temporarily prefixes the buffer, captures it, then
    # removes exactly that prefix before reading another PTY key.
    local __bus_prefix='__bus_probe ' __bus_query='\e[__BUS_PROMPT_TOKEN__h~'
    local __bus_i __bus_map
    for ((__bus_i=0; __bus_i<${#__bus_prefix}; __bus_i++)); do
        __bus_query+='\e[__BUS_PROMPT_TOKEN__q~'${__bus_prefix:__bus_i:1}
    done
    __bus_query+='\e[__BUS_PROMPT_TOKEN__e~\e[__BUS_PROMPT_TOKEN__t~\e[__BUS_PROMPT_TOKEN__h~'
    for ((__bus_i=0; __bus_i<${#__bus_prefix}; __bus_i++)); do
        __bus_query+='\e[__BUS_PROMPT_TOKEN__d~'
    done
    __bus_query+='\e[__BUS_PROMPT_TOKEN__e~'
    complete -F __bus_capture_editor '__bus_probe'
    for __bus_map in emacs-standard vi-insertion vi-command; do
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__h~":beginning-of-line'
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__e~":end-of-line'
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__q~":quoted-insert'
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__d~":delete-char'
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__t~":complete'
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__k~":kill-line'
        bind -m "$__bus_map" "\"\\e[__BUS_PROMPT_TOKEN__v~\":\"$__bus_query\""
        bind -m "$__bus_map" '"\e[__BUS_PROMPT_TOKEN__c~":"\e[__BUS_PROMPT_TOKEN__h~\e[__BUS_PROMPT_TOKEN__k~"'
    done
}
__bus_prompt_ready() {
    local __bus_status=$?
    __bus_bind_editor
    local __bus_mark='\[\e]133;A;bus=__BUS_PROMPT_TOKEN__\a\]'
    # Readline renders this only after PS1's user expansions have completed.
    # Keep the private marker out of child shells, including exported user PS1.
    export -n PS1
    [[ $PS1 == *"$__bus_mark" ]] || PS1+="$__bus_mark"
    return "$__bus_status"
}
# The private hook function is not exported; neither may its invocation be.
export -n PROMPT_COMMAND
if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) &&
    [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a '* ]]; then
    PROMPT_COMMAND+=(__bus_prompt_ready)
else
    PROMPT_COMMAND="${PROMPT_COMMAND-}"$'\n''__bus_prompt_ready'
fi
"#;
