use std::{
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

use super::compile::{compile_manifest, parse_manifest, CompiledRule};
use super::schema::AgentManifest;
use super::{agent_label, parse_agent_label, Agent};

#[derive(Debug, Clone)]
pub(super) struct LoadedManifest {
    pub(super) manifest: AgentManifest,
    pub(super) compiled_rules: Vec<CompiledRule>,
}

#[derive(Debug, Clone)]
struct ManifestCache {
    pub(super) manifests: Vec<(Agent, Option<LoadedManifest>)>,
}

pub(super) const BUNDLED_MANIFESTS: &[(&str, &str)] = &[
    ("amp", include_str!("bundled/amp.toml")),
    ("agy", include_str!("bundled/antigravity.toml")),
    ("claude", include_str!("bundled/claude.toml")),
    ("cline", include_str!("bundled/cline.toml")),
    ("codex", include_str!("bundled/codex.toml")),
    ("cursor", include_str!("bundled/cursor.toml")),
    ("devin", include_str!("bundled/devin.toml")),
    ("droid", include_str!("bundled/droid.toml")),
    ("gemini", include_str!("bundled/gemini.toml")),
    ("grok", include_str!("bundled/grok.toml")),
    ("hermes", include_str!("bundled/hermes.toml")),
    ("kilo", include_str!("bundled/kilo.toml")),
    ("kimi", include_str!("bundled/kimi.toml")),
    ("kiro", include_str!("bundled/kiro.toml")),
    ("maki", include_str!("bundled/maki.toml")),
    ("muse", include_str!("bundled/muse.toml")),
    ("opencode", include_str!("bundled/opencode.toml")),
    ("pi", include_str!("bundled/pi.toml")),
    ("qodercli", include_str!("bundled/qodercli.toml")),
    ("qwen", include_str!("bundled/qwen.toml")),
    ("copilot", include_str!("bundled/github-copilot.toml")),
];

static MANIFEST_CACHE: OnceLock<RwLock<ManifestCache>> = OnceLock::new();

/// Rebuilds the cache so tests observe override files written after first use.
#[cfg(test)]
pub(crate) fn reload_manifests() {
    let cache = build_manifest_cache();
    let lock = MANIFEST_CACHE.get_or_init(|| RwLock::new(cache.clone()));
    match lock.write() {
        Ok(mut guard) => *guard = cache,
        Err(poisoned) => *poisoned.into_inner() = cache,
    }
}

fn manifest_cache() -> &'static RwLock<ManifestCache> {
    MANIFEST_CACHE.get_or_init(|| RwLock::new(build_manifest_cache()))
}

fn build_manifest_cache() -> ManifestCache {
    ManifestCache {
        manifests: Agent::SCREEN_MANIFEST_AGENTS
            .into_iter()
            .map(|agent| (agent, load_manifest_uncached(agent)))
            .collect(),
    }
}

pub(super) fn load_manifest(agent: Agent) -> Option<LoadedManifest> {
    let lock = manifest_cache();
    let guard = match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .manifests
        .iter()
        .find(|(cached_agent, _)| *cached_agent == agent)
        .and_then(|(_, loaded)| loaded.clone())
}

fn load_manifest_uncached(agent: Agent) -> Option<LoadedManifest> {
    let bundled = bundled_manifest(agent)?;
    let Some(path) = override_path(agent).filter(|path| path.exists()) else {
        return Some(bundled_loaded_manifest(agent, bundled));
    };

    let warning = match read_override_manifest(&path) {
        Ok(manifest) if manifest_matches_agent(&manifest, agent) => {
            match loaded_manifest(manifest) {
                Ok(loaded) => return Some(loaded),
                Err(err) => format!(
                    "ignored override {} because it could not be compiled: {err}",
                    path.display()
                ),
            }
        }
        Ok(manifest) => format!(
            "ignored override {} because manifest id {} does not match {}",
            path.display(),
            manifest.id,
            agent_label(agent)
        ),
        Err(err) => format!(
            "ignored override {} because it could not be loaded: {err}",
            path.display()
        ),
    };
    tracing::warn!(target: "bus::agents::manifest", "{warning}");
    Some(bundled_loaded_manifest(agent, bundled))
}

pub(super) fn loaded_manifest(manifest: AgentManifest) -> Result<LoadedManifest, String> {
    let compiled_rules = compile_manifest(&manifest)?;
    Ok(LoadedManifest {
        manifest,
        compiled_rules,
    })
}

pub(super) fn bundled_loaded_manifest(agent: Agent, manifest: AgentManifest) -> LoadedManifest {
    loaded_manifest(manifest).unwrap_or_else(|err| {
        panic!(
            "bundled {} manifest could not be compiled: {err}",
            agent_label(agent)
        )
    })
}

pub(super) fn bundled_manifest(agent: Agent) -> Option<AgentManifest> {
    let id = agent_label(agent);
    BUNDLED_MANIFESTS
        .iter()
        .find(|(manifest_id, _)| *manifest_id == id)
        .map(|(_, content)| {
            parse_manifest(content)
                .unwrap_or_else(|err| panic!("bundled {id} manifest is invalid: {err}"))
        })
}

fn read_override_manifest(path: &Path) -> Result<AgentManifest, String> {
    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    parse_manifest(&content)
}

pub(super) fn override_path(agent: Agent) -> Option<PathBuf> {
    Some(
        crate::config::config_dir()
            .join("agent-detection")
            .join(format!("{}.toml", agent_label(agent))),
    )
}

fn manifest_matches_agent(manifest: &AgentManifest, agent: Agent) -> bool {
    let id = agent_label(agent);
    manifest.id == id
        || manifest.aliases.iter().any(|alias| alias == id)
        || parse_agent_label(&manifest.id) == Some(agent)
        || manifest
            .aliases
            .iter()
            .any(|alias| parse_agent_label(alias) == Some(agent))
}
