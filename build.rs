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
