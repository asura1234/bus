use super::{
    bundle::{self, Skill},
    data_dir,
};
use crate::messaging::storage::io as bus_io;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io, path::Path};

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Manifest {
    pub(super) schema: u32,
    pub(super) bundle_hash: String,
    pub(super) installed: BTreeMap<String, Owned>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Owned {
    hash: String,
    version: String,
}

pub(super) fn load_manifest(home: &Path) -> io::Result<Manifest> {
    let path = data_dir(home).join("skills-manifest.json");
    match fs::read(&path) {
        Ok(bytes) => {
            let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            if manifest.schema != 1 {
                return Err(io::Error::other("Unsupported Bus skills manifest schema"));
            }
            Ok(manifest)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Manifest {
            schema: 1,
            ..Manifest::default()
        }),
        Err(error) => Err(error),
    }
}

pub(super) fn save_manifest(home: &Path, manifest: &Manifest) -> io::Result<()> {
    bus_io::atomic_write(
        &data_dir(home).join("skills-manifest.json"),
        &serde_json::to_vec_pretty(manifest)?,
    )
}

/// Refuse symlink ancestors as well as symlink skills. A provider registry can
/// itself be a user's link into a repo; Bus must never write through that link.
fn safe_ancestors(path: &Path) -> io::Result<bool> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if ancestor
            .file_name()
            .is_some_and(|name| name == ".claude" || name == ".agents")
        {
            break;
        }
    }
    Ok(true)
}

fn tree_files(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name == "__pycache__" || path.extension().is_some_and(|ext| ext == "pyc") {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::other("Skill contains a symlink"));
        }
        if kind.is_dir() {
            tree_files(root, &path, files)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, fs::read(path)?);
        } else {
            return Err(io::Error::other("Skill contains a special file"));
        }
    }
    Ok(())
}

pub(super) fn tree_hash(root: &Path) -> io::Result<String> {
    let mut files = BTreeMap::new();
    tree_files(root, root, &mut files)?;
    Ok(bundle::digest(
        files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    ))
}

pub(super) fn state(path: &Path, owned: Option<&Owned>, desired: &str) -> io::Result<&'static str> {
    if !safe_ancestors(path)? {
        return Ok("conflict");
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("missing"),
        Err(error) => return Err(error),
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Ok("conflict");
    }
    let Some(owned) = owned else {
        return Ok("conflict");
    };
    match tree_hash(path) {
        Ok(hash) if hash == owned.hash => Ok(if hash == desired {
            "installed"
        } else {
            "outdated"
        }),
        Ok(_) => Ok("conflict"),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Err(error),
        Err(_) => Ok("conflict"),
    }
}

fn copy_files(skill: &Skill, stage: &Path) -> io::Result<()> {
    fs::create_dir(stage)?;
    for file in &skill.files {
        let path = stage.join(&file.path);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("Missing skill file parent"))?,
        )?;
        fs::write(&path, file.bytes)?;
        #[cfg(not(unix))]
        let _ = file.executable;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                &path,
                fs::Permissions::from_mode(if file.executable { 0o755 } else { 0o644 }),
            )?;
        }
    }
    Ok(())
}

pub(super) fn install_one(
    home: &Path,
    root: &Path,
    provider: &str,
    skill: &Skill,
    manifest: &mut Manifest,
) -> io::Result<()> {
    let path = root.join(&skill.name);
    let key = format!("{provider}/{}", skill.name);
    let state = state(&path, manifest.installed.get(&key), &skill.hash)?;
    if matches!(state, "conflict" | "installed") {
        return Ok(());
    }
    bus_io::private_dir(root)?;
    let token = format!("{}-{}", std::process::id(), bus_io::now_ns());
    let stage = root.join(format!(".bus-stage-{token}"));
    let backup = root.join(format!(".bus-backup-{token}"));
    let result = (|| {
        copy_files(skill, &stage)?;
        if self::state(&path, manifest.installed.get(&key), &skill.hash)? != state {
            return Ok(());
        }
        if state == "outdated" {
            fs::rename(&path, &backup)?;
        }
        if let Err(error) = fs::rename(&stage, &path) {
            if backup.exists() {
                fs::rename(&backup, &path)?;
            }
            return Err(error);
        }
        manifest.installed.insert(
            key,
            Owned {
                hash: skill.hash.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        );
        save_manifest(home, manifest)?;
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        Ok(())
    })();
    if stage.exists() {
        fs::remove_dir_all(stage)?;
    }
    result
}
