use crate::agents::providers::{launch, ProviderKind};
use std::{
    io,
    path::{Path, PathBuf},
};

/// User-visible notices are posted by messaging at launch, including restore.
pub(crate) fn warnings_for_launch(
    provider: ProviderKind,
    cwd: &Path,
    home: &Path,
) -> io::Result<Vec<String>> {
    let (repo_scope, user_scope) = match provider {
        ProviderKind::ClaudeCode => (".claude/skills", ".claude/skills"),
        ProviderKind::Codex => (".agents/skills", ".agents/skills"),
        ProviderKind::Cursor | ProviderKind::Grok => return Ok(Vec::new()),
    };
    let user = home.join(user_scope);
    let canonical_user = user.canonicalize().ok();
    let cwd = cwd.canonicalize()?;
    let root = launch::project_root(&cwd);
    let mut warnings = Vec::new();
    for directory in cwd.ancestors() {
        let local = directory.join(repo_scope);
        if canonical_user.is_some() && local.canonicalize().ok() == canonical_user {
            if directory == root {
                break;
            }
            continue;
        }
        match std::fs::read_dir(&local) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name();
                    let skill = entry.path().join("SKILL.md");
                    let user_skill: PathBuf = user.join(&name).join("SKILL.md");
                    if skill.is_file()
                        && user_skill.is_file()
                        && skill.canonicalize()? != user_skill.canonicalize()?
                    {
                        let behavior = match provider {
                            ProviderKind::ClaudeCode => {
                                "Claude Code gives the user copy precedence"
                            }
                            _ => "Codex lists both copies",
                        };
                        warnings.push(format!(
                            "Same-name repo skill {} and user skill {}: {behavior}.",
                            skill.display(),
                            user_skill.display()
                        ));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if directory == root {
            break;
        }
    }
    Ok(warnings)
}

/// Record warnings in room history so they remain visible after the terminal opens.
pub(crate) fn post_warnings(
    state: &mut crate::messaging::model::BusState,
    agent: &crate::messaging::model::RoomAgent,
) -> Result<(), String> {
    let Some(home) = std::env::home_dir() else {
        return Ok(());
    };
    let messages = warnings_for_launch(
        crate::messaging::identity::provider_kind(agent.provider),
        Path::new(&agent.cwd),
        &home,
    )
    .unwrap_or_else(|error| vec![format!("Could not inspect skill shadowing: {error}")]);
    for message in messages {
        state
            .post_bus_notice(
                agent.room_id,
                None,
                message,
                crate::messaging::storage::io::now_ms(),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
