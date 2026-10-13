//! Resume keeps frozen source scope and current-executable service privileges.
use super::NativeResumeContext;
use crate::{agents::providers::hook_json::HookContext, messaging::model::RoomAgent};
use std::{collections::HashMap, path::Path};

pub(super) fn validate_scope(agent: &RoomAgent, cwd: &Path) -> Result<(), String> {
    if let Some(frozen) = &agent.launch_cwd {
        if cwd
            .canonicalize()
            .map_err(|_| "Bus resume cwd unavailable")?
            != *frozen
        {
            return Err(
                "Bus resume cwd drifted from frozen launch root; inspect before relaunch".into(),
            );
        }
    }
    if let Some(scratch) = &agent.scratch_dir {
        match std::fs::symlink_metadata(scratch) {
            Ok(metadata) if metadata.is_dir() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = scratch
                    .parent()
                    .ok_or("Bus resume scratch parent unavailable")?;
                if parent
                    .canonicalize()
                    .map_err(|_| "Bus resume scratch parent unavailable")?
                    != parent
                {
                    return Err("Bus resume scratch escaped its frozen path".into());
                }
                let mut builder = std::fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder
                    .create(scratch)
                    .map_err(|_| "Bus resume private scratch restoration failed")?;
            }
            _ => return Err("Bus resume scratch escaped its frozen path".into()),
        }
        if scratch
            .canonicalize()
            .map_err(|_| "Bus resume private scratch missing")?
            != *scratch
        {
            return Err("Bus resume scratch escaped its frozen path".into());
        }
    }
    Ok(())
}

pub(super) fn extras(
    context: &NativeResumeContext,
    agent: &RoomAgent,
    hooks: &HookContext,
    root: &Path,
    args: &mut Vec<String>,
) -> Result<HashMap<String, String>, String> {
    let mut env = hooks.env();
    if agent.permission_handler {
        crate::agents::providers::launch::profile::service_settings(hooks)?;
        crate::agents::providers::launch::profile::service_env(&context.binary, &mut env)?;
        env.insert("BUS_PERMISSION_HANDLER_ID".into(), agent.id.0.to_string());
        let text = super::super::super::permissions::handler_prompt_with_binary(
            agent.room_id,
            agent.id,
            root,
            Some(&context.binary),
        );
        crate::messaging::orchestration::write_prompt(&hooks.spool, &text)?;
    } else if agent.orchestrates.is_none() {
        if let Some(scratch) = &agent.scratch_dir {
            env.insert("TMPDIR".into(), scratch.to_string_lossy().into_owned());
        }
        if agent.provider == crate::messaging::model::Provider::ClaudeCode
            && agent.launch_profile.as_deref()
                == Some(crate::agents::providers::launch::profile::CLAUDE_WORKER)
        {
            let cwd = agent
                .launch_cwd
                .as_deref()
                .ok_or("Scoped Claude resume root missing")?;
            let scratch = agent
                .scratch_dir
                .as_deref()
                .ok_or("Scoped Claude resume scratch missing")?;
            crate::agents::providers::launch::profile::worker_claude_settings(hooks, cwd, scratch)?;
        }
        if agent.provider == crate::messaging::model::Provider::Codex {
            let read_only = crate::agents::providers::launch::profile::codex_read_only(
                hooks,
                agent.launch_cwd.as_deref(),
                agent.launch_profile.as_deref()
                    == Some(crate::agents::providers::launch::profile::CODEX_READ_ONLY),
            );
            if agent.launch_profile.is_some() || agent.launch_cwd.is_none() {
                args.extend(crate::agents::providers::launch::profile::codex_args(
                    agent.scratch_dir.as_deref(),
                    read_only,
                ));
            }
        }
    }
    Ok(env)
}
