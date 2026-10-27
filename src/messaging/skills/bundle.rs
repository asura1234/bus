//! Immutable copy payload, independent of the source checkout at runtime.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Debug)]
pub(super) struct File {
    pub(super) path: String,
    pub(super) bytes: &'static [u8],
    pub(super) executable: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Skill {
    pub(super) name: String,
    pub(super) files: Vec<File>,
    pub(super) hash: String,
}

pub(super) fn digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut hash = Sha256::new();
    for (path, bytes) in files {
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

pub(super) fn shipped() -> &'static [Skill] {
    static SKILLS: OnceLock<Vec<Skill>> = OnceLock::new();
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    SKILLS.get_or_init(|| {
        let data = DATA.get_or_init(|| {
            use std::io::Read;
            let mut data = Vec::new();
            flate2::read::ZlibDecoder::new(
                include_bytes!(concat!(env!("OUT_DIR"), "/skills.bin")).as_slice(),
            )
            .read_to_end(&mut data)
            .expect("decode embedded skills");
            data
        });
        let mut skills: BTreeMap<String, Vec<File>> = BTreeMap::new();
        for line in include_str!(concat!(env!("OUT_DIR"), "/skills.index")).lines() {
            let fields: Vec<_> = line.split('\t').collect();
            let (name, path) = fields[0].split_once('/').expect("generated skill path");
            let start: usize = fields[2].parse().expect("generated offset");
            let end: usize = fields[3].parse().expect("generated offset");
            skills.entry(name.to_owned()).or_default().push(File {
                path: path.to_owned(),
                bytes: &data[start..end],
                executable: fields[1] == "x",
            });
        }
        skills
            .into_iter()
            .map(|(name, files)| {
                let hash = digest(files.iter().map(|file| (file.path.as_str(), file.bytes)));
                Skill { name, files, hash }
            })
            .collect()
    })
}

pub(crate) fn hash() -> String {
    digest(
        shipped()
            .iter()
            .map(|skill| (skill.name.as_str(), skill.hash.as_bytes())),
    )
}
