//! User-scope skill copies and ownership. All paths derive from the caller's HOME.
mod bundle;
mod files;
mod install;

use crate::messaging::storage::io as bus_io;
pub(crate) use bundle::hash as bundle_hash;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

const PROVIDERS: [(&str, &str); 2] = [("claude", ".claude"), ("codex", ".agents")];

#[derive(Debug, Serialize)]
pub(crate) struct Status {
    pub(crate) bundle_hash: String,
    version: &'static str,
    build_sha: &'static str,
    source_root: String,
    skills: Vec<SkillStatus>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SkillStatus {
    provider: &'static str,
    name: String,
    path: PathBuf,
    state: &'static str,
    /// What this install invocation did. Status omits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'static str>,
}

pub(crate) fn home() -> io::Result<PathBuf> {
    std::env::home_dir().ok_or_else(|| io::Error::other("Home directory unavailable"))
}

fn data_dir(home: &Path) -> PathBuf {
    home.join(".local/share/bus")
}
fn source_root() -> String {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::canonicalize(&manifest)
        .unwrap_or(manifest)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn status(home: &Path) -> io::Result<Status> {
    let manifest = install::load_manifest(home)?;
    let mut report = Status {
        bundle_hash: bundle_hash(),
        version: env!("CARGO_PKG_VERSION"),
        build_sha: env!("BUS_BUILD_SHA"),
        source_root: source_root(),
        skills: Vec::new(),
        warnings: Vec::new(),
    };
    for (provider, directory) in PROVIDERS {
        for skill in bundle::shipped() {
            let path = home.join(directory).join("skills").join(&skill.name);
            let key = format!("{provider}/{}", skill.name);
            let state = install::state(&path, manifest.installed.get(&key), &skill.hash)?;
            if state == "blocked-link" {
                report.warnings.push(format!("Bus skipped {}: provider home or skills registry is symlinked; it will not write through that link", install::linked_ancestor(&path)?.unwrap_or_else(|| path.clone()).display()));
            }
            if state == "conflict" {
                report.warnings.push(format!(
                    "Bus will not overwrite user skill at {} (unmanaged or modified)",
                    path.display()
                ));
            }
            report.skills.push(SkillStatus {
                provider,
                name: skill.name.clone(),
                path,
                state,
                action: None,
            });
        }
    }
    Ok(report)
}

pub(crate) fn install(home: &Path) -> io::Result<Status> {
    bus_io::private_dir(&data_dir(home))?;
    let _lease = bus_io::lock_wait(&data_dir(home).join("skills-install.lock"))?;
    let mut manifest = install::load_manifest(home)?;
    let mut actions = BTreeMap::new();
    for (provider, directory) in PROVIDERS {
        let root = home.join(directory).join("skills");
        for skill in bundle::shipped() {
            let action = install::install_one(home, &root, provider, skill, &mut manifest)?;
            actions.insert(format!("{provider}/{}", skill.name), action);
        }
    }
    manifest.bundle_hash = bundle_hash();
    install::save_manifest(home, &manifest)?;
    let mut report = status(home)?;
    for skill in &mut report.skills {
        let key = format!("{}/{}", skill.provider, skill.name);
        skill.action = actions.get(&key).copied();
    }
    Ok(report)
}

pub(crate) fn skill_names() -> Vec<&'static str> {
    bundle::shipped()
        .iter()
        .filter(|skill| skill.name != "_bus")
        .map(|skill| skill.name.as_str())
        .collect()
}

#[cfg(test)]
#[path = "tests/installer_test.rs"]
mod tests;
