use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

const VENDORED_INPUTS: &[&str] = &[
    "build.zig",
    "build.zig.zon",
    "include",
    "pkg",
    "src",
    "VERSION",
];

fn newest_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    let metadata = fs::metadata(path).ok()?;
    let own = metadata.modified().ok();
    if !metadata.is_dir() {
        return own;
    }
    fs::read_dir(path)
        .ok()?
        .filter_map(|entry| newest_mtime(&entry.ok()?.path()))
        .chain(own)
        .max()
}

/// Zig re-resolves every dependency from the network on each run, so skip it when the
/// previous output was built with the same arguments and no vendored input is newer.
fn vendored_lib_is_fresh(
    vendored_dir: &std::path::Path,
    stamp: &std::path::Path,
    key: &str,
) -> bool {
    let Ok(recorded) = fs::read_to_string(stamp) else {
        return false;
    };
    let Some(built) = fs::metadata(stamp).and_then(|m| m.modified()).ok() else {
        return false;
    };
    recorded == key
        && VENDORED_INPUTS
            .iter()
            .map(|input| vendored_dir.join(input))
            .chain([vendored_dir.with_extension("vendor.json")])
            .all(|input| newest_mtime(&input).is_some_and(|mtime| mtime <= built))
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
    record_build_identity();
    embed_skills();
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
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
    println!("cargo:rerun-if-env-changed=BUS_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=BUS_BUILD_ID");
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
    command.current_dir(&vendored_dir);
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

    let lib_dir = vendored_dir.join("zig-out/lib");
    let stamp = lib_dir.join(".bus-build-stamp");
    let stamp_key = format!("{zig} {:?}", command.get_args().collect::<Vec<_>>());
    if !vendored_lib_is_fresh(&vendored_dir, &stamp, &stamp_key) {
        build_vendored_lib(command, &zig);
        fs::write(&stamp, &stamp_key).expect("failed to write libghostty-vt build stamp");
    }

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

fn build_vendored_lib(mut command: Command, zig: &str) {
    let status = command.status().unwrap_or_else(|err| {
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
}

fn record_build_identity() {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let git = |args: &[&str]| {
        // A source archive inside another project must not claim its HEAD.
        // Both ordinary checkouts and worktrees have their own .git entry.
        let manifest = manifest
            .as_ref()
            .filter(|root| root.join(".git").exists())?;
        Command::new("git")
            .current_dir(manifest)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            // Build metadata collection must not refresh or rewrite the index.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
    };
    let sha = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=BUS_BUILD_SHA={sha}");
    println!(
        "cargo:rustc-env=BUS_BUILD_PROFILE={}",
        env::var("PROFILE").unwrap_or_else(|_| "unknown".into())
    );
    if env::var_os("CARGO_FEATURE_DEV_TOOLS").is_some() {
        record_dev_build_identity(manifest.as_deref(), &git);
    }
    if let Some(path) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &branch]) {
            // Packed refs have no loose file. Watch its existing parent so the
            // next commit creating that file still invalidates the build.
            let mut watched = PathBuf::from(path);
            while !watched.exists() && watched.pop() {}
            if watched.exists() {
                println!("cargo:rerun-if-changed={}", watched.display());
            }
        }
        if let Some(path) = git(&["rev-parse", "--git-path", "packed-refs"]) {
            if PathBuf::from(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

fn record_dev_build_identity(
    manifest: Option<&std::path::Path>,
    git: &impl Fn(&[&str]) -> Option<String>,
) {
    // Match dirty detection to the inputs compiled into Bus, not docs/plans/tools.
    // Ghostty's generated zig-out/.zig-cache directories are deliberately excluded.
    let inputs: Vec<String> = [
        "src",
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "assets",
        "orchestration",
        "workflows",
        "skills",
        "cli_extensions",
        "scripts",
        "docs/guides",
        "docs/templates",
        "docs/dev-tools.md",
        "docs/tui-driver.md",
        "vendor/portable-pty",
        "vendor/libghostty-vt.vendor.json",
    ]
    .into_iter()
    .map(String::from)
    .chain(
        VENDORED_INPUTS
            .iter()
            .map(|path| format!("vendor/libghostty-vt/{path}")),
    )
    .collect();
    let mut status_args = vec!["status", "--porcelain", "--untracked-files=normal", "--"];
    status_args.extend(inputs.iter().map(String::as_str));
    let short = match (git(&["rev-parse", "--short", "HEAD"]), git(&status_args)) {
        (Some(short), Some(status)) if !status.is_empty() => format!("{short}-dirty"),
        (Some(short), Some(_)) => short,
        _ => "unknown".into(),
    };
    println!("cargo:rustc-env=BUS_BUILD_SHORT_SHA={short}");
    for path in inputs {
        // Directory watches detect additions/deletions without watching missing files
        // forever. Archives and minimal build-identity test crates may omit inputs.
        if manifest.is_some_and(|root| root.join(&path).exists()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// Embed all runtime resources so a copied executable needs no source checkout.
fn embed_skills() {
    fn collect(root: &std::path::Path, path: &std::path::Path, files: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("read skill resources")
            .map(|entry| entry.expect("skill entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            let name = entry.file_name().expect("skill filename").to_string_lossy();
            if name.starts_with('.')
                || name == "tests"
                || name == "__pycache__"
                || name.ends_with("_test.py")
                || name.ends_with(".pyc")
            {
                continue;
            }
            if entry.is_dir() {
                collect(root, &entry, files);
            } else if entry.is_file() {
                files.push(
                    entry
                        .strip_prefix(root)
                        .expect("skill resource path")
                        .to_path_buf(),
                );
            }
        }
    }
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest"));
    let skills = root.join("skills");
    println!("cargo:rerun-if-changed=skills");
    println!("cargo:rerun-if-changed=cli_extensions");
    println!("cargo:rerun-if-changed=scripts");
    println!("cargo:rerun-if-changed=workflows");
    println!("cargo:rerun-if-changed=docs/guides");
    println!("cargo:rerun-if-changed=docs/templates");
    let mut files = Vec::new();
    collect(&skills, &skills, &mut files);
    let mut data = Vec::new();
    let mut index = String::new();
    for relative in files {
        // The architecture document is authoring guidance, not an installable skill.
        if relative.components().count() < 2 {
            continue;
        }
        let path = skills.join(&relative);
        let start = data.len();
        data.extend(fs::read(&path).expect("skill bytes"));
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&path)
                .expect("skill metadata")
                .permissions()
                .mode()
                & 0o111
                != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        index.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            relative.to_string_lossy().replace('\\', "/"),
            if executable { "x" } else { "-" },
            start,
            data.len()
        ));
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("skills.bin"), data).expect("write embedded skill bytes");
    fs::write(out.join("skills.index"), index).expect("write embedded skill index");
}
