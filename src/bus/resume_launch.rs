//! Rehydrate only Bus-owned callback capture when native sessions cold-resume.
use std::{
    io::Read,
    path::{Path, PathBuf},
};

use super::{callbacks::Manifest, model::Provider, store::JsonStore};
use crate::{agent_resume::AgentResumePlan, terminal::TerminalState};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LaunchExtras {
    pub(crate) env: Vec<(String, String)>,
    pub(crate) args: Vec<String>,
}

pub(crate) fn for_native_resume(
    terminal: &TerminalState,
    plan: &AgentResumePlan,
    cwd: &Path,
) -> Result<LaunchExtras, String> {
    if !is_bus_owned(terminal) {
        return Ok(LaunchExtras::default());
    }
    let root = super::entry::data_dir();
    let session_name = crate::session::active_name();
    let config_root = crate::config::config_dir();
    // A reserved-looking name or inherited data-root variable does not opt a
    // different native server into Bus lifecycle ownership.
    if !root
        .as_ref()
        .is_some_and(|root| root.is_absolute() && config_root == root.join("herdr-config"))
        || session_name.as_deref() != Some(super::runtime::DEFAULT_SESSION)
    {
        return Ok(LaunchExtras::default());
    }
    let binary = std::env::current_exe().map_err(|_| "Bus resume executable unavailable")?;
    // This lookup runs only at a native resume attempt, never in pane rendering.
    let project = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .unwrap_or_else(|| cwd.to_path_buf());
    load(
        root.as_deref(),
        session_name.as_deref(),
        &config_root,
        terminal,
        plan,
        &project,
        &binary,
    )
}

fn is_bus_owned(terminal: &TerminalState) -> bool {
    terminal
        .agent_name
        .as_deref()
        .is_some_and(|name| name.starts_with("bus-r"))
}

fn load(
    root: Option<&Path>,
    session_name: Option<&str>,
    config_root: &Path,
    terminal: &TerminalState,
    plan: &AgentResumePlan,
    project: &Path,
    binary: &Path,
) -> Result<LaunchExtras, String> {
    if !is_bus_owned(terminal) {
        return Ok(LaunchExtras::default());
    }
    let root = root
        .filter(|root| root.is_absolute())
        .ok_or("Bus resume data root unavailable")?;
    if session_name != Some(super::runtime::DEFAULT_SESSION)
        || config_root != root.join("herdr-config")
    {
        return Err("Bus resume context belongs to another server session".into());
    }
    let session = terminal
        .persisted_agent_session
        .as_ref()
        .ok_or("Bus resume has no saved provider session")?;
    if session.session_ref.kind != crate::agent_resume::AgentSessionRefKind::Id
        || terminal
            .managed_agent_kind()
            .map(crate::detect::agent_label)
            != Some(session.agent.as_str())
        || crate::agent_resume::plan(&session.source, &session.agent, &session.session_ref).as_ref()
            != Some(plan)
    {
        return Err("Bus resume plan does not match its official saved provider session".into());
    }
    let state = JsonStore::new(root.join("state.json"))
        .load()
        .map_err(|_| "Bus resume state is unreadable")?
        .ok_or("Bus resume state is missing")?;
    let mut candidates = state.agents().filter(|agent| {
        super::launch::provider_kind(agent.provider) == session.agent
            && agent.runtime_identity.session_id.as_deref()
                == Some(session.session_ref.value.as_str())
    });
    let agent = candidates
        .next()
        .ok_or("Bus resume has no matching saved owner")?;
    if candidates.next().is_some()
        || terminal.agent_name.as_deref()
            != Some(format!("bus-r{}-a{}", agent.room_id.0, agent.id.0).as_str())
    {
        return Err("Bus resume ownership is ambiguous or mismatched".into());
    }
    if agent.deletion_pending
        || state
            .room(agent.room_id)
            .is_none_or(|room| room.deletion_pending)
        || agent.session_binding_invalidated
    {
        return Err("Bus resume is suspended by deletion or invalidated identity".into());
    }
    let launch = agent
        .runtime_identity
        .launch_id
        .as_deref()
        .filter(|launch| {
            !launch.is_empty()
                && launch
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
        .ok_or("Bus resume launch identity is missing or invalid")?;
    let spool = root.join("callbacks").join(launch);
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "Bus resume root unavailable")?;
    if spool
        .canonicalize()
        .map_err(|_| "Bus resume callback directory missing")?
        != canonical_root.join("callbacks").join(launch)
    {
        return Err("Bus resume callback directory escaped its saved root".into());
    }
    let manifest: Manifest = serde_json::from_value(read_json(&spool.join("manifest.json"))?)
        .map_err(|_| "Bus resume callback manifest is invalid")?;
    if manifest.agent_id != agent.id
        || manifest.provider != agent.provider
        || manifest.launch_id != launch
    {
        return Err("Bus resume callback manifest does not match its saved owner".into());
    }
    let hook_path = match agent.provider {
        Provider::ClaudeCode => spool.join("claude-settings.json"),
        Provider::Codex => project.join(".codex/hooks.json"),
        Provider::Cursor => project.join(".cursor/hooks.json"),
    };
    validate_hooks(&hook_path, agent.provider, binary)?;
    Ok(LaunchExtras {
        env: vec![
            ("BUS_LAUNCH_ID".into(), launch.into()),
            (
                "BUS_CALLBACK_DIR".into(),
                spool.to_string_lossy().into_owned(),
            ),
        ],
        args: if agent.provider == Provider::ClaudeCode {
            vec![
                "--settings".into(),
                hook_path.to_string_lossy().into_owned(),
            ]
        } else {
            vec![]
        },
    })
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    if !path.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("Bus resume capture configuration is missing".into());
    }
    let file =
        std::fs::File::open(path).map_err(|_| "Bus resume capture configuration is unreadable")?;
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Bus resume capture configuration is unreadable")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Bus resume capture configuration is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Bus resume capture configuration is invalid".into())
}

fn validate_hooks(path: &Path, provider: Provider, binary: &Path) -> Result<(), String> {
    let document = read_json(path)?;
    let (adapter, events): (&str, &[&str]) = match provider {
        Provider::ClaudeCode => (
            "claude-hook",
            &["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"],
        ),
        Provider::Codex => ("codex-hook", &["SessionStart", "UserPromptSubmit", "Stop"]),
        Provider::Cursor => (
            "cursor-hook",
            &[
                "sessionStart",
                "beforeSubmitPrompt",
                "afterAgentResponse",
                "stop",
            ],
        ),
    };
    let command = format!(
        "{} --bus-callback {adapter}",
        crate::platform::remote_reattach_program(&binary.to_string_lossy())
    );
    let complete = document
        .get("disableAllHooks")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        && events.iter().all(|event| {
            document
                .get("hooks")
                .and_then(|hooks| hooks.get(event))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        if provider == Provider::Cursor {
                            entry.get("command").and_then(serde_json::Value::as_str)
                                == Some(command.as_str())
                        } else {
                            entry
                                .get("hooks")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|hooks| {
                                    hooks.iter().any(|hook| {
                                        hook.get("type").and_then(serde_json::Value::as_str)
                                            == Some("command")
                                            && hook
                                                .get("command")
                                                .and_then(serde_json::Value::as_str)
                                                == Some(command.as_str())
                                    })
                                })
                        }
                    })
                })
        });
    if !complete {
        return Err(
            "Bus resume hooks are missing or changed; existing configuration was kept".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{ffi::OsString, path::Path, sync::MutexGuard};

    pub(crate) struct ProcessEnvironment {
        _guard: MutexGuard<'static, ()>,
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl ProcessEnvironment {
        pub(crate) fn enter(root: Option<&Path>, session: Option<&str>) -> Self {
            let guard = crate::config::test_config_env_lock().lock().unwrap();
            let previous = ["BUS_DATA_DIR", "HERDR_SESSION"]
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect();
            for (key, value) in [
                ("BUS_DATA_DIR", root.map(|root| root.as_os_str())),
                ("HERDR_SESSION", session.map(std::ffi::OsStr::new)),
            ] {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
            Self {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for ProcessEnvironment {
        fn drop(&mut self) {
            for (key, value) in &self.previous {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{callbacks, model::*, store::JsonStore};
    use serde_json::json;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        project: PathBuf,
        binary: PathBuf,
        terminal: TerminalState,
        plan: AgentResumePlan,
        state: BusState,
        provider: Provider,
        agent: AgentId,
    }

    impl Fixture {
        fn new(provider: Provider) -> Self {
            let root =
                std::env::temp_dir().join(format!("bus-resume-{}", crate::bus::io::now_ns()));
            let project = root.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let binary = std::env::current_exe().unwrap();
            let mut state = BusState::new();
            let room = state.create_room("original room").unwrap();
            let agent = state
                .create_agent(room, "original agent", provider, project.clone(), None)
                .unwrap();
            state
                .set_agent_runtime_identity(
                    agent,
                    AgentRuntimeIdentity {
                        launch_id: Some("owned-launch".into()),
                        terminal_id: Some("old-terminal".into()),
                        pane_id: Some("w1:p2".into()),
                        session_id: Some("original-session".into()),
                    },
                )
                .unwrap();
            state.confirm_hook_setup(agent).unwrap();
            let label = crate::bus::launch::provider_kind(provider);
            let kind = crate::detect::parse_agent_label(label).unwrap();
            let session = crate::agent_resume::PersistedAgentSession {
                source: format!("herdr:{label}"),
                agent: label.into(),
                session_ref: crate::agent_resume::AgentSessionRef::id("original-session").unwrap(),
            };
            let plan =
                crate::agent_resume::plan(&session.source, &session.agent, &session.session_ref)
                    .unwrap();
            let mut terminal =
                TerminalState::new(crate::terminal::TerminalId::alloc(), project.clone());
            terminal.set_persisted_agent_session(session);
            terminal.restore_managed_agent("bus-r1-a2".into(), kind);
            callbacks::initialize(
                &root.join("callbacks/owned-launch"),
                &callbacks::Manifest {
                    agent_id: agent,
                    provider,
                    launch_id: "owned-launch".into(),
                },
            )
            .unwrap();
            let fixture = Self {
                root,
                project,
                binary,
                terminal,
                plan,
                state,
                provider,
                agent,
            };
            fixture.save();
            fixture.write_hooks();
            fixture
        }

        fn save(&self) {
            JsonStore::new(self.root.join("state.json"))
                .save(&self.state)
                .unwrap();
        }

        fn hook_path(&self) -> PathBuf {
            match self.provider {
                Provider::ClaudeCode => self
                    .root
                    .join("callbacks/owned-launch/claude-settings.json"),
                Provider::Codex => self.project.join(".codex/hooks.json"),
                Provider::Cursor => self.project.join(".cursor/hooks.json"),
            }
        }

        fn write_hooks(&self) {
            let (adapter, events): (&str, &[&str]) = match self.provider {
                Provider::ClaudeCode => (
                    "claude-hook",
                    &["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"],
                ),
                Provider::Codex => ("codex-hook", &["SessionStart", "UserPromptSubmit", "Stop"]),
                Provider::Cursor => (
                    "cursor-hook",
                    &[
                        "sessionStart",
                        "beforeSubmitPrompt",
                        "afterAgentResponse",
                        "stop",
                    ],
                ),
            };
            let command = format!(
                "{} --bus-callback {adapter}",
                crate::platform::remote_reattach_program(&self.binary.to_string_lossy())
            );
            let mut hooks = serde_json::Map::new();
            for event in events {
                hooks.insert(
                    (*event).into(),
                    if self.provider == Provider::Cursor {
                        json!([{"command":command}])
                    } else {
                        json!([{"hooks":[{"type":"command","command":command,"timeout":5}]}])
                    },
                );
            }
            let path = self.hook_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                serde_json::to_vec(&json!({"version":1,"hooks":hooks})).unwrap(),
            )
            .unwrap();
        }

        fn load(&self) -> Result<LaunchExtras, String> {
            load(
                Some(&self.root),
                Some("bus"),
                &self.root.join("herdr-config"),
                &self.terminal,
                &self.plan,
                &self.project,
                &self.binary,
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn bus_resume_restores_existing_capture_for_each_provider_without_rewriting_plan() {
        for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
            let fixture = Fixture::new(provider);
            let original = fixture.plan.clone();
            let extras = fixture.load().unwrap();
            assert_eq!(
                extras.env,
                vec![
                    ("BUS_LAUNCH_ID".into(), "owned-launch".into()),
                    (
                        "BUS_CALLBACK_DIR".into(),
                        fixture
                            .root
                            .join("callbacks/owned-launch")
                            .to_string_lossy()
                            .into_owned()
                    ),
                ]
            );
            assert_eq!(
                extras.args,
                if provider == Provider::ClaudeCode {
                    vec![
                        "--settings".into(),
                        fixture.hook_path().to_string_lossy().into_owned(),
                    ]
                } else {
                    vec![]
                }
            );
            assert_eq!(fixture.plan, original);
        }
    }

    #[test]
    fn bus_resume_rejects_unattested_or_suspended_ownership() {
        for field in [
            "session",
            "name",
            "provider",
            "plan",
            "deletion",
            "invalidated",
            "launch",
            "manifest",
            "hooks",
            "duplicate",
        ] {
            let mut fixture = Fixture::new(Provider::Codex);
            let mut value = serde_json::to_value(&fixture.state).unwrap();
            let key = fixture.agent.0.to_string();
            match field {
                "session" => {
                    value["agents"][&key]["runtime_identity"]["session_id"] = json!("other")
                }
                "name" => fixture.terminal.set_agent_name("bus-r999-a2".into()),
                "provider" => value["agents"][&key]["provider"] = json!("cursor"),
                "plan" => fixture
                    .plan
                    .argv
                    .push("--dangerously-bypass-approvals-and-sandbox".into()),
                "deletion" => value["agents"][&key]["deletion_pending"] = json!(true),
                "invalidated" => value["agents"][&key]["session_binding_invalidated"] = json!(true),
                "launch" => {
                    value["agents"][&key]["runtime_identity"]["launch_id"] =
                        json!("../owned-launch")
                }
                "manifest" => std::fs::write(
                    fixture.root.join("callbacks/owned-launch/manifest.json"),
                    b"{}",
                )
                .unwrap(),
                "hooks" => std::fs::remove_file(fixture.hook_path()).unwrap(),
                "duplicate" => {
                    let mut other = value["agents"][&key].clone();
                    other["id"] = json!(99);
                    value["agents"]["99"] = other;
                }
                _ => unreachable!(),
            }
            fixture.state = serde_json::from_value(value).unwrap();
            fixture.save();
            assert!(fixture.load().is_err(), "{field}");
        }
    }

    #[test]
    fn bus_resume_preserves_unconfirmed_delivery_gate_without_blocking_native_setup() {
        let mut fixture = Fixture::new(Provider::Cursor);
        let mut value = serde_json::to_value(&fixture.state).unwrap();
        value["agents"][fixture.agent.0.to_string()]["hook_setup_confirmed"] = json!(false);
        fixture.state = serde_json::from_value(value).unwrap();
        fixture.save();
        assert_eq!(fixture.load().unwrap().env.len(), 2);
        let saved = JsonStore::new(fixture.root.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert!(!saved.agent(fixture.agent).unwrap().hook_setup_confirmed);
    }

    #[test]
    fn bus_resume_is_scoped_to_bus_root_and_leaves_other_native_resumes_unchanged() {
        let mut fixture = Fixture::new(Provider::Codex);
        for (root, session, config) in [
            (None, Some("bus"), fixture.root.join("herdr-config")),
            (
                Some(fixture.root.as_path()),
                Some("other"),
                fixture.root.join("herdr-config"),
            ),
            (
                Some(fixture.root.as_path()),
                Some("bus"),
                fixture.root.join("other-config"),
            ),
        ] {
            assert!(load(
                root,
                session,
                &config,
                &fixture.terminal,
                &fixture.plan,
                &fixture.project,
                &fixture.binary
            )
            .is_err());
        }
        fixture.terminal.set_agent_name("personal-agent".into());
        assert_eq!(
            load(
                None,
                None,
                Path::new("/unavailable"),
                &fixture.terminal,
                &fixture.plan,
                &fixture.project,
                &fixture.binary
            )
            .unwrap(),
            LaunchExtras::default()
        );
    }

    #[test]
    fn bus_resume_entry_leaves_non_bus_servers_alone_even_with_reserved_names() {
        let fixture = Fixture::new(Provider::Codex);
        for (root, session) in [(None, None), (Some(fixture.root.as_path()), Some("nested"))] {
            let _env = test_support::ProcessEnvironment::enter(root, session);
            assert_eq!(
                for_native_resume(&fixture.terminal, &fixture.plan, &fixture.project).unwrap(),
                LaunchExtras::default()
            );
        }
    }
}
