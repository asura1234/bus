use crate::agents::providers::{launch, ProviderKind};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Same-name repo skills that also exist in the user scope. The names are
/// sorted so one launch writes one stable log line.
pub(crate) fn shadowed_skills(
    provider: ProviderKind,
    cwd: &Path,
    home: &Path,
) -> io::Result<Vec<String>> {
    let Some((repo_scope, user_scope)) = skill_shadow_scopes(provider) else {
        return Ok(Vec::new());
    };
    let user = home.join(user_scope);
    let canonical_user = user.canonicalize().ok();
    let cwd = cwd.canonicalize()?;
    let root = launch::project_root(&cwd);
    let mut names = Vec::new();
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
                        if let Some(name) = name.to_str() {
                            if !names.iter().any(|existing| existing == name) {
                                names.push(name.to_owned());
                            }
                        }
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
    names.sort();
    Ok(names)
}

fn skill_shadow_scopes(provider: ProviderKind) -> Option<(&'static str, &'static str)> {
    match provider {
        ProviderKind::ClaudeCode => Some((".claude/skills", ".claude/skills")),
        ProviderKind::Codex => Some((".agents/skills", ".agents/skills")),
        ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::Kimi => None,
        #[cfg(feature = "dev-tools")]
        ProviderKind::Fake => None,
    }
}

fn shadow_behavior(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::ClaudeCode => "Claude Code gives the user copy precedence",
        ProviderKind::Codex | ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::Kimi => {
            "Codex lists both copies"
        }
        #[cfg(feature = "dev-tools")]
        ProviderKind::Fake => "the fake agent does not scan skills",
    }
}

/// Scan at launch, including restore. Collisions go to the server log only.
pub(crate) fn log_warnings(agent: &crate::messaging::model::RoomAgent) {
    let Some(home) = std::env::home_dir() else {
        return;
    };
    log_warnings_at(agent, &home);
}

fn log_warnings_at(agent: &crate::messaging::model::RoomAgent, home: &Path) {
    let provider = crate::messaging::identity::provider_kind(agent.provider);
    if skill_shadow_scopes(provider).is_none() {
        return;
    }
    let names = match shadowed_skills(provider, Path::new(&agent.cwd), home) {
        Ok(names) => names,
        Err(error) => {
            tracing::warn!("Could not inspect skill shadowing: {error}");
            return;
        }
    };
    if names.is_empty() {
        return;
    }
    let cwd = match Path::new(&agent.cwd).canonicalize() {
        Ok(cwd) => cwd,
        Err(error) => {
            tracing::warn!("Could not inspect skill shadowing: {error}");
            return;
        }
    };
    let repo = launch::project_root(&cwd);
    let noun = if names.len() == 1 { "skill" } else { "skills" };
    tracing::info!(
        "Skill shadow in {} ({}): {} same-name {noun} {}; {}",
        repo.display(),
        provider.label(),
        names.len(),
        names.join(", "),
        shadow_behavior(provider),
    );
}

#[cfg(test)]
#[path = "tests/shadow_test.rs"]
mod shadow_tests;
