use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const CONTENT_INTERFACE: &str = "ROOM_AGENT_CONTENT_INTERFACE_V1";
const CONTENT_FILE_MAX: usize = 64 * 1024;
const CONTENT_TOTAL_MAX: usize = 256 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentManifest {
    interface: String,
    content_version: u32,
    compatibility: u32,
    system: ContentEntry,
    agent: ContentEntry,
    skills: Vec<NamedContentEntry>,
    references: Vec<NamedContentEntry>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentEntry {
    path: String,
    sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NamedContentEntry {
    name: String,
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct PackedContentEntry {
    name: String,
    sha256: String,
    body: String,
}

#[derive(Serialize)]
struct PackedContentBundle {
    interface: String,
    content_version: u32,
    compatibility: u32,
    digest: String,
    system: String,
    agent: String,
    skills: Vec<PackedContentEntry>,
    references: Vec<PackedContentEntry>,
    index: String,
}

#[derive(Clone, Copy, PartialEq)]
enum SelectorPresence {
    Required,
    WhenManifestExists,
}

fn pack_content_selector(
    manifest_dir: &std::path::Path,
    selector: &str,
    presence: SelectorPresence,
) {
    let content_root = manifest_dir.join("src/bus/orchestrator/content");
    let root = content_root.join(selector);
    let manifest_path = root.join("manifest.json");
    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"))
        .join(format!("bus-{selector}-content.json"));
    if presence == SelectorPresence::WhenManifestExists && !manifest_path.exists() {
        // Watching the existing parent notices a later bundle without rerunning every build.
        println!("cargo:rerun-if-changed={}", content_root.display());
        fs::write(output, "null")
            .unwrap_or_else(|_| panic!("{selector} absent content marker write failed"));
        return;
    }
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest_bytes =
        fs::read(&manifest_path).unwrap_or_else(|_| panic!("{selector} manifest missing"));
    let manifest: ContentManifest = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|_| panic!("{selector} manifest is invalid"));
    assert_eq!(
        manifest.interface, CONTENT_INTERFACE,
        "content interface mismatch"
    );
    assert!(
        manifest.content_version > 0,
        "content version must be positive"
    );
    assert_eq!(
        manifest.compatibility, 1,
        "unsupported content compatibility"
    );
    let mut names = std::collections::BTreeSet::new();
    let (system_size, system) = validate_content_entry(&root, "system", &manifest.system);
    let (agent_size, agent) = validate_content_entry(&root, "agent", &manifest.agent);
    let mut total = system_size + agent_size;
    let mut skills = Vec::new();
    let mut references = Vec::new();
    for (kind, entries, packed) in [
        ("skill", &manifest.skills, &mut skills),
        ("reference", &manifest.references, &mut references),
    ] {
        for entry in entries {
            assert!(
                !entry.name.is_empty()
                    && entry.name.len() <= 64
                    && entry.name.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    }),
                "invalid content entry name"
            );
            assert!(
                names.insert(entry.name.as_str()),
                "duplicate content entry name"
            );
            let (size, body) = validate_content_entry(
                &root,
                &entry.name,
                &ContentEntry {
                    path: entry.path.clone(),
                    sha256: entry.sha256.clone(),
                },
            );
            total += size;
            packed.push(PackedContentEntry {
                name: entry.name.clone(),
                sha256: entry.sha256.clone(),
                body,
            });
            println!("cargo:warning=packed {kind} content {}", entry.name);
        }
    }
    assert!(
        total <= CONTENT_TOTAL_MAX,
        "content bundle exceeds total cap"
    );
    let canonical_manifest = serde_json::to_vec(&manifest)
        .unwrap_or_else(|_| panic!("{selector} manifest canonicalization failed"));
    let digest = format!("{:x}", Sha256::digest(&canonical_manifest));
    let mut index = String::from("ROOM_AGENT_CONTENT_INTERFACE_V1\n");
    for entry in &skills {
        index.push_str(&format!("skill\t{}\t{}\n", entry.name, entry.sha256));
    }
    for entry in &references {
        index.push_str(&format!("reference\t{}\t{}\n", entry.name, entry.sha256));
    }
    let packed = PackedContentBundle {
        interface: manifest.interface,
        content_version: manifest.content_version,
        compatibility: manifest.compatibility,
        digest,
        system,
        agent,
        skills,
        references,
        index,
    };
    fs::write(
        output,
        serde_json::to_vec(&packed).unwrap_or_else(|_| panic!("{selector} content packing failed")),
    )
    .unwrap_or_else(|_| panic!("{selector} packed content write failed"));
}

fn validate_content_entry(
    root: &std::path::Path,
    slot: &str,
    entry: &ContentEntry,
) -> (usize, String) {
    let path = PathBuf::from(&entry.path);
    assert!(
        !path.is_absolute()
            && path.components().count() == 1
            && path.extension().and_then(|extension| extension.to_str()) == Some("md"),
        "{slot} content path must be one fixed-tree markdown file"
    );
    let full = root.join(path);
    println!("cargo:rerun-if-changed={}", full.display());
    let bytes = fs::read(&full).unwrap_or_else(|_| panic!("{slot} content file missing"));
    assert!(
        bytes.len() <= CONTENT_FILE_MAX,
        "{slot} content exceeds per-file cap"
    );
    let body = std::str::from_utf8(&bytes)
        .unwrap_or_else(|_| panic!("{slot} content is not UTF-8"))
        .to_owned();
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        entry.sha256,
        "{slot} content digest mismatch"
    );
    (bytes.len(), body)
}

fn zig_target(target: &str) -> &str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("unsupported target for libghostty-vt build: {other}"),
    }
}

fn env_bool(name: &str) -> Option<bool> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            other => panic!("invalid boolean value for {name}: {other}"),
        },
        Err(env::VarError::NotPresent) => None,
        Err(err) => panic!("failed to read {name}: {err}"),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    pack_content_selector(&manifest_dir, "test-agent-led", SelectorPresence::Required);
    pack_content_selector(
        &manifest_dir,
        "production",
        SelectorPresence::WhenManifestExists,
    );
    println!("cargo:rerun-if-changed=vendor/libghostty-vt.vendor.json");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/build.zig");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/build.zig.zon");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/include");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/pkg");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/src");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/VERSION");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SIMD");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_ID");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=ZIG");

    let vendored_dir = manifest_dir.join("vendor/libghostty-vt");
    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let simd = env_bool("LIBGHOSTTY_VT_SIMD").unwrap_or(true);
    let target = env::var("TARGET").expect("TARGET");
    let zig_target = zig_target(&target);
    let version_string = fs::read_to_string(vendored_dir.join("VERSION"))
        .expect("failed to read vendored libghostty-vt VERSION")
        .trim()
        .to_string();

    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let mut command = Command::new(&zig);
    command
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dsimd={simd}"))
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={version_string}"))
        .arg("-Demit-xcframework=false");
    if let Ok(system_dir) = env::var("LIBGHOSTTY_VT_ZIG_SYSTEM_DIR") {
        command.arg("--system").arg(system_dir);
    }

    let status = command
        .current_dir(&vendored_dir)
        .status()
        .unwrap_or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                panic!(
                    "zig executable not found (looked for {zig:?}; set the ZIG \
                     environment variable to point at the zig binary). Building \
                     the vendored libghostty-vt requires Zig 0.15.2: on macOS run \
                     `brew install zig@0.15`, elsewhere install it from \
                     https://ziglang.org/download/, then retry the build"
                );
            }
            panic!("failed to execute zig build for vendored libghostty-vt: {err}");
        });
    assert!(
        status.success(),
        "zig build for vendored libghostty-vt failed: {status}"
    );

    let lib_dir = vendored_dir.join("zig-out/lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    if target.contains("apple-darwin") {
        let static_lib = lib_dir.join("libghostty-vt.a");
        println!("cargo:rustc-link-arg={}", static_lib.display());
    } else if target.contains("windows-msvc") {
        println!("cargo:rustc-link-lib=static=ghostty-vt-static");
    } else {
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}
