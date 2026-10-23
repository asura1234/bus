//! Private startup hooks for panes explicitly created for an agent launch.
use portable_pty::CommandBuilder;
use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) struct ShellIntegration {
    directory: PathBuf,
    started_at: Instant,
    observed_output: Cell<(u64, Instant)>,
}

impl Drop for ShellIntegration {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

// Preserve the measured seven-second builtin rc; leave one second of headroom.
const PROMPT_FALLBACK_WINDOW: Duration = Duration::from_secs(8);
// Five native retry intervals, and ten 50-ms progress ticks in the noisy-rc probe.
const OUTPUT_QUIET_PERIOD: Duration = Duration::from_millis(500);

impl ShellIntegration {
    pub(super) fn install_for_agent_launch(
        cmd: &mut CommandBuilder,
        shell: &str,
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
        let started_at = Instant::now();
        let integration = Self {
            directory,
            started_at,
            observed_output: Cell::new((0, started_at)),
        };
        if name == Some("zsh") {
            integration.zsh(cmd)?;
        } else {
            integration.bash(cmd, shell)?;
        }
        Ok(Some(integration))
    }

    pub(super) fn start_wait(&mut self) {
        self.started_at = Instant::now();
        self.observed_output.set((0, self.started_at));
    }

    pub(super) fn waiting_for_quiet_fallback(&self, content_seq: u64) -> bool {
        let now = Instant::now();
        let (previous_seq, mut quiet_since) = self.observed_output.get();
        // Observe the existing PTY sequence rather than adding work to its reader.
        // Starting the clock when a change is observed is conservatively late.
        if content_seq != previous_seq || !content_seq.is_multiple_of(2) {
            quiet_since = now;
            self.observed_output.set((content_seq, now));
        }
        now.duration_since(self.started_at) < PROMPT_FALLBACK_WINDOW
            || now.duration_since(quiet_since) < OUTPUT_QUIET_PERIOD
    }

    fn zsh(&self, cmd: &mut CommandBuilder) -> io::Result<()> {
        let original = cmd.get_env("ZDOTDIR").map(std::ffi::OsStr::to_os_string);
        cmd.env(
            "__BUS_ZDOTDIR_SET",
            if original.is_some() { "1" } else { "0" },
        );
        cmd.env("__BUS_ZDOTDIR", original.unwrap_or_default());
        cmd.env("ZDOTDIR", &self.directory);
        std::fs::write(self.directory.join(".zshenv"), ZSH_ENV)
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

const ZSH_ENV: &str = r#"if [[ $__BUS_ZDOTDIR_SET == 1 ]]; then
    ZDOTDIR=$__BUS_ZDOTDIR
else
    unset ZDOTDIR
fi
unset __BUS_ZDOTDIR_SET __BUS_ZDOTDIR
if [[ -r ${ZDOTDIR-$HOME}/.zshenv ]]; then source "${ZDOTDIR-$HOME}/.zshenv"; fi
function __bus_prompt_ready() {
    local __bus_status=$?
    builtin printf '\033]133;A\007'
    return $__bus_status
}
function __bus_first_prompt() {
    precmd_functions=(${precmd_functions:#__bus_first_prompt} __bus_prompt_ready)
    unfunction __bus_first_prompt
    __bus_prompt_ready
}
typeset -ga precmd_functions
precmd_functions+=(__bus_first_prompt)
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
    builtin printf '\033]133;A\007'
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
