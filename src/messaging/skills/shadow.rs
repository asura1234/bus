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
        ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::Kimi => return Ok(Vec::new()),
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
                        let behavior = shadow_behavior(provider);
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

fn shadow_behavior(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::ClaudeCode => "Claude Code gives the user copy precedence",
        ProviderKind::Codex | ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::Kimi => {
            "Codex lists both copies"
        }
    }
}

/// One digest for this launch's repo. A later launch in the same room posts
/// again only when that repo's shadowed set changes, including across restart,
/// because the previous digest stays in room history.
fn shadow_digest(repo: &Path, provider: ProviderKind, messages: &[String]) -> String {
    let mut lines = messages.to_vec();
    lines.sort();
    let behavior = shadow_behavior(provider);
    let noun = if lines.len() == 1 { "skill" } else { "skills" };
    let mut text = format!(
        "Skill shadow in {}: {} same-name {noun}. {behavior}.",
        repo.display(),
        lines.len(),
    );
    for line in &lines {
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// Record warnings in room history so they remain visible after the terminal opens.
pub(crate) fn post_warnings(
    state: &mut crate::messaging::model::BusState,
    agent: &crate::messaging::model::RoomAgent,
) -> Result<(), String> {
    let Some(home) = std::env::home_dir() else {
        return Ok(());
    };
    post_warnings_at(state, agent, &home)
}

fn post_warnings_at(
    state: &mut crate::messaging::model::BusState,
    agent: &crate::messaging::model::RoomAgent,
    home: &Path,
) -> Result<(), String> {
    let provider = crate::messaging::identity::provider_kind(agent.provider);
    let messages = warnings_for_launch(provider, Path::new(&agent.cwd), home)
        .unwrap_or_else(|error| vec![format!("Could not inspect skill shadowing: {error}")]);
    if messages.is_empty() {
        return Ok(());
    }
    if messages
        .iter()
        .any(|message| message.starts_with("Could not inspect skill shadowing:"))
    {
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
        return Ok(());
    }
    let cwd = Path::new(&agent.cwd)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let repo = launch::project_root(&cwd);
    let digest = shadow_digest(&repo, provider, &messages);
    let marker = format!("Skill shadow in {}:", repo.display());
    let unchanged = state
        .notices(agent.room_id)
        .rev()
        .find(|notice| notice.text.starts_with(&marker))
        .is_some_and(|notice| notice.text == digest);
    if unchanged {
        return Ok(());
    }
    state
        .post_bus_notice(
            agent.room_id,
            None,
            digest,
            crate::messaging::storage::io::now_ms(),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/shadow_test.rs"]
mod shadow_tests;
