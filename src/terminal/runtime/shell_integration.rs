//! Private startup hooks for panes explicitly created for an agent launch.
use portable_pty::CommandBuilder;
use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static NEXT_INTEGRATION: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
#[path = "tests/shell_integration_test.rs"]
mod tests;

#[cfg_attr(test, derive(Debug))]
pub(super) struct ShellIntegration {
    directory: PathBuf,
    token: String,
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
        // Only explicit per-launch identity opts in, never an inherited server identity.
        let agent_launch = launch_env
            .extra
            .iter()
            .any(|(key, value)| key == "BUS_LAUNCH_ID" && !value.is_empty());
        if !cfg!(unix) || !agent_launch {
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
        let name = Path::new(&shell).file_name().and_then(|name| name.to_str());
        if !matches!(name, Some("zsh" | "bash")) {
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
            started_at,
            process: None,
            initial_image,
            observed_image: RefCell::new(None),
            prompt_floor: Cell::new(0),
        };
        if name == Some("zsh") {
            integration.zsh(cmd)?;
        } else {
            integration.bash(cmd, &shell)?;
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

    fn zsh(&self, cmd: &mut CommandBuilder) -> io::Result<()> {
        let original = cmd.get_env("ZDOTDIR").map(std::ffi::OsStr::to_os_string);
        cmd.env(
            "__BUS_ZDOTDIR_SET",
            if original.is_some() { "1" } else { "0" },
        );
        cmd.env("__BUS_ZDOTDIR", original.unwrap_or_default());
        cmd.env("ZDOTDIR", &self.directory);
        std::fs::write(
            self.directory.join(".zshenv"),
            ZSH_ENV.replace("__BUS_PROMPT_TOKEN__", &self.token),
        )
    }

    fn bash(&self, cmd: &mut CommandBuilder, shell: &str) -> io::Result<()> {
        // --rcfile starts a non-login interactive bash. In login mode we source
        // its profile files manually; this does not set login_shell or run
        // .bash_logout. Only agent-launch panes use this limited emulation.
        let login = cmd.is_default_prog();
        let rcfile = self.directory.join("bashrc");
        std::fs::write(
            &rcfile,
            format!(
                "{}{}",
                if login { BASH_LOGIN } else { BASH_RC },
                BASH_PROMPT.replace("__BUS_PROMPT_TOKEN__", &self.token)
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
const BASH_PROMPT: &str = r#"__bus_prompt_ready() {
    local __bus_status=$?
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
