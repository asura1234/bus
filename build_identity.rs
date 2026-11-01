use std::env;
use std::path::PathBuf;
use std::process::Command;

// Cargo must recompile this build script when another checkout uses the target.
const _: Option<&str> = option_env!("BUS_CHECKOUT_ROOT");

const VENDORED_INPUTS: &[&str] = &[
    "build.zig",
    "build.zig.zon",
    "include",
    "pkg",
    "src",
    "VERSION",
];

fn record_build_identity() {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    // Cargo otherwise treats identical relative paths in different checkouts as
    // the same package. Its config resolves this value for the current checkout.
    println!("cargo:rerun-if-env-changed=BUS_CHECKOUT_ROOT");
    if let Some(root) = &manifest {
        println!("cargo:rustc-env=BUS_BUILD_CHECKOUT={}", root.display());
    }
    if let Some(root) = manifest.as_ref().filter(|root| root.join(".git").exists()) {
        let marker = env::var_os("BUS_CHECKOUT_ROOT")
            .map(PathBuf::from)
            .expect("BUS_CHECKOUT_ROOT is missing; run Cargo from the checkout so .cargo/config.toml is loaded");
        assert_eq!(
            root.canonicalize().expect("canonical checkout path"),
            marker.canonicalize().expect("canonical BUS_CHECKOUT_ROOT"),
            "BUS_CHECKOUT_ROOT must identify the current checkout"
        );
    }
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
    record_build_source_identity(manifest.as_deref(), &git);
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

fn record_build_source_identity(
    manifest: Option<&std::path::Path>,
    git: &impl Fn(&[&str]) -> Option<String>,
) {
    // Match dirty detection to the inputs compiled into Bus, not docs/plans/tools.
    // Ghostty's generated zig-out/.zig-cache directories are deliberately excluded.
    let inputs: Vec<String> = [
        "src",
        "build.rs",
        "build_identity.rs",
        "Cargo.toml",
        "Cargo.lock",
        "assets",
        "orchestration",
        "workflows",
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
