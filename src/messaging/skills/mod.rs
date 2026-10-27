//! User-scope skill copies and ownership. All paths derive from the caller's HOME.
mod bundle;
mod files;
mod install;
mod shadow;

use crate::messaging::{prefs::settings, storage::io as bus_io};
pub(crate) use bundle::hash as bundle_hash;
use serde::{Deserialize, Serialize};
pub(crate) use shadow::post_warnings;
#[cfg(test)]
use shadow::warnings_for_launch;
use std::{
    io,
    path::{Path, PathBuf},
};

const PROVIDERS: [(&str, &str); 2] = [("claude", ".claude"), ("codex", ".agents")];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Answer {
    pub(crate) bundle_hash: String,
    pub(crate) accepted: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct Status {
    pub(crate) bundle_hash: String,
    version: &'static str,
    answer: Option<Answer>,
    skills: Vec<SkillStatus>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SkillStatus {
    provider: &'static str,
    name: String,
    path: PathBuf,
    state: &'static str,
}

pub(crate) fn home() -> io::Result<PathBuf> {
    std::env::home_dir().ok_or_else(|| io::Error::other("Home directory unavailable"))
}

fn data_dir(home: &Path) -> PathBuf {
    home.join(".local/share/bus")
}
fn settings_path(home: &Path) -> PathBuf {
    data_dir(home).join("settings.json")
}

pub(crate) fn answer(home: &Path) -> io::Result<Option<Answer>> {
    settings::load(&settings_path(home))
        .map(|settings| settings.skills_answer)
        .map_err(io::Error::other)
}

/// Serialize first-launch offers across simultaneous Bus launches.
pub(crate) fn offer_lease(home: &Path) -> io::Result<std::fs::File> {
    bus_io::private_dir(&data_dir(home))?;
    bus_io::lock(&data_dir(home).join("skills-offer.lock"))
}

pub(crate) fn needs_offer(home: &Path) -> io::Result<bool> {
    Ok(answer(home)?.is_none_or(|answer| answer.bundle_hash != bundle_hash()))
}

pub(crate) fn record_answer(home: &Path, accepted: bool) -> io::Result<()> {
    let answer = Answer {
        bundle_hash: bundle_hash(),
        accepted,
    };
    settings::update(&settings_path(home), |settings| {
        settings.skills_answer = Some(answer)
    })
    .map_err(io::Error::other)?;
    Ok(())
}

pub(crate) fn status(home: &Path) -> io::Result<Status> {
    let manifest = install::load_manifest(home)?;
    let mut report = Status {
        bundle_hash: bundle_hash(),
        version: env!("CARGO_PKG_VERSION"),
        answer: answer(home)?,
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
            });
        }
    }
    Ok(report)
}

pub(crate) fn install(home: &Path) -> io::Result<Status> {
    bus_io::private_dir(&data_dir(home))?;
    let _lease = bus_io::append_lock(&data_dir(home).join("skills-install.lock"))?;
    // Keep explicit consent when installation fails.
    record_answer(home, true)?;
    let mut manifest = install::load_manifest(home)?;
    for (provider, directory) in PROVIDERS {
        let root = home.join(directory).join("skills");
        for skill in bundle::shipped() {
            install::install_one(home, &root, provider, skill, &mut manifest)?;
        }
    }
    manifest.bundle_hash = bundle_hash();
    install::save_manifest(home, &manifest)?;
    status(home)
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
