//! A new provider tab can execute argv after shell initialization, without ZLE/readline.
use crate::agents::AgentKind;
use crate::protocol::api::schema::TabAgentLaunch;
use crate::terminal::runtime::spawn::PaneShellConfig;

pub(super) struct PreparedAgent {
    pub(super) name: String,
    pub(super) kind: AgentKind,
    pub(super) provider_argv: Vec<String>,
    pub(super) shell_argv: Vec<String>,
}

pub(super) fn prepare(
    launch: Option<TabAgentLaunch>,
    shell: PaneShellConfig<'_>,
) -> Result<Option<PreparedAgent>, String> {
    let Some(launch) = launch else {
        return Ok(None);
    };
    if !crate::server::terminals::agents::valid_agent_name(&launch.name) {
        return Err("Invalid provider launch name".into());
    }
    let kind =
        crate::agents::parse_agent_label(&launch.kind).ok_or("Unsupported provider launch kind")?;
    if launch
        .args
        .iter()
        .any(|arg| arg.chars().any(char::is_control))
    {
        return Err("Provider launch arguments cannot contain control characters".into());
    }
    let provider_argv = crate::server::terminals::agents::provider_launch_argv(kind, launch.args);
    let Some(shell_argv) = crate::terminal::runtime::spawn::agent_exec_argv(shell, &provider_argv)
        .map_err(|error| error.to_string())?
    else {
        // Preserve the existing native start path for other configured shells.
        return Ok(None);
    };
    Ok(Some(PreparedAgent {
        name: launch.name,
        kind,
        provider_argv,
        shell_argv,
    }))
}

impl PreparedAgent {
    pub(super) fn apply_shell_env(&self, env: &mut Vec<(String, String)>) {
        // Match the normal login-shell builder; explicit caller overrides win.
        if self.shell_argv.get(1).is_some_and(|arg| arg == "-l")
            && !env.iter().any(|(key, _)| key == "SHELL")
        {
            env.push(("SHELL".into(), self.shell_argv[0].clone()));
        }
    }

    pub(super) fn bind(
        self,
        terminal: &mut crate::terminal::TerminalState,
        runtime: &crate::terminal::TerminalRuntime,
    ) {
        terminal.launch_argv = Some(self.provider_argv.clone());
        terminal.bind_managed_launch_pending(runtime.managed_launch_pending_signal());
        terminal.begin_managed_agent(
            self.name,
            self.kind,
            std::time::Instant::now(),
            crate::server::terminals::agents::AGENT_START_SETTLE_DELAY,
            crate::protocol::api::AGENT_START_TIMEOUT,
        );
        if let Some(session) = crate::agents::resume::catalog::persisted_session_from_launch_args(
            self.kind,
            &self.provider_argv[1..],
        ) {
            terminal.set_managed_agent_launch_session(session);
        }
        runtime.mark_agent_launch();
    }
}
