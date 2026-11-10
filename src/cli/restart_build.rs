//! Build the checkout that owns the running executable before a source restart.
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `cargo build` the checkout above `current_exe`, then return the fresh binary.
pub(super) fn default_source_build() -> io::Result<PathBuf> {
    let exe = crate::platform::fs::running_executable()
        .map_err(|error| io::Error::other(format!("could not find this bus binary: {error}")))?;
    build_checkout(&exe, cfg!(feature = "dev-tools"))
}

pub(super) fn build_checkout(exe: &Path, dev_tools: bool) -> io::Result<PathBuf> {
    let checkout = source_checkout(exe).ok_or_else(|| {
        io::Error::other(
            "could not find the Cargo.toml that builds bin bus above this executable; pass --binary PATH",
        )
    })?;
    let binary = output_binary(&checkout);
    run_cargo_build(&checkout, dev_tools)?;
    if !binary.is_file() {
        return Err(io::Error::other(format!(
            "cargo build succeeded but did not produce {}",
            binary.display()
        )));
    }
    Ok(binary)
}

/// Walk parents of `exe` to the first manifest that builds the `bus` binary.
/// A virtual workspace or another package is not a match, so the walk continues.
/// An executable outside the source tree (a custom target dir) has no such
/// manifest; the caller passes `--binary` instead.
pub(super) fn source_checkout(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent()?.to_path_buf();
    loop {
        if manifest_builds_bus_binary(&dir.join("Cargo.toml")) {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn manifest_builds_bus_binary(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    explicit_bus_bin(&value) || package_autobin_bus(&value, path)
}

fn explicit_bus_bin(value: &toml::Value) -> bool {
    value
        .get("bin")
        .and_then(toml::Value::as_array)
        .is_some_and(|bins| bins.iter().any(bin_named_bus))
}

fn bin_named_bus(bin: &toml::Value) -> bool {
    bin.get("name").and_then(toml::Value::as_str) == Some("bus")
}

fn package_autobin_bus(value: &toml::Value, manifest: &Path) -> bool {
    let Some(package) = value.get("package") else {
        return false;
    };
    if package.get("name").and_then(toml::Value::as_str) != Some("bus") {
        return false;
    }
    let autobins = package
        .get("autobins")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);
    autobins
        && manifest
            .parent()
            .is_some_and(|dir| dir.join("src/main.rs").is_file())
}

pub(super) fn cargo_build_command(checkout: &Path, dev_tools: bool) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(checkout)
        .arg("build")
        .arg("--locked")
        .arg("--bin")
        .arg("bus");
    if dev_tools {
        command.arg("--features").arg("dev-tools");
    }
    command
}

pub(super) fn output_binary(checkout: &Path) -> PathBuf {
    debug_bus(checkout, std::env::var_os("CARGO_TARGET_DIR").as_deref())
}

pub(super) fn debug_bus(checkout: &Path, cargo_target_dir: Option<&OsStr>) -> PathBuf {
    let target = match cargo_target_dir {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                checkout.join(path)
            }
        }
        _ => checkout.join("target"),
    };
    target
        .join("debug")
        .join(format!("bus{}", std::env::consts::EXE_SUFFIX))
}

fn run_cargo_build(checkout: &Path, dev_tools: bool) -> io::Result<()> {
    let output = cargo_build_command(checkout, dev_tools)
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .map_err(|error| io::Error::other(format!("could not run cargo build: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(build_failure(output.status, &output.stderr))
}

fn build_failure(status: std::process::ExitStatus, stderr: &[u8]) -> io::Error {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(20);
    let tail = lines[start..].join("\n");
    let detail = if tail.trim().is_empty() {
        status.to_string()
    } else {
        tail
    };
    io::Error::other(format!(
        "cargo build failed; the running Bus was not restarted:\n{detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn cargo_build_command_is_locked_and_adds_dev_tools_only_when_asked() {
        let checkout = Path::new("/src/bus");
        let plain = cargo_build_command(checkout, false);
        assert_eq!(plain.get_program(), "cargo");
        assert_eq!(plain.get_current_dir(), Some(checkout));
        assert_eq!(args_of(&plain), ["build", "--locked", "--bin", "bus"]);
        let featured = cargo_build_command(checkout, true);
        assert_eq!(
            args_of(&featured),
            [
                "build",
                "--locked",
                "--bin",
                "bus",
                "--features",
                "dev-tools"
            ]
        );
    }

    #[test]
    fn debug_bus_uses_the_checkout_target_or_cargo_target_dir() {
        let checkout = Path::new("/src/bus");
        let suffix = std::env::consts::EXE_SUFFIX;
        assert_eq!(
            debug_bus(checkout, None),
            PathBuf::from(format!("/src/bus/target/debug/bus{suffix}"))
        );
        assert_eq!(
            debug_bus(checkout, Some(OsStr::new("out"))),
            PathBuf::from(format!("/src/bus/out/debug/bus{suffix}"))
        );
        assert_eq!(
            debug_bus(checkout, Some(OsStr::new("/tmp/shared"))),
            PathBuf::from(format!("/tmp/shared/debug/bus{suffix}"))
        );
    }

    #[test]
    fn source_checkout_walks_past_a_decoy_to_the_manifest_that_builds_bus() {
        let root = crate::utils::test_temp::unique_temp_path("checkout-walk");
        let exe = root.join("nested/target/debug/bus");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();
        std::fs::write(
            root.join("nested/Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"bus\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(source_checkout(&exe).unwrap(), root);

        let outside = crate::utils::test_temp::unique_temp_path("checkout-missing");
        let orphan = outside.join("target/debug/bus");
        std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
        assert!(source_checkout(&orphan).is_none());

        let named = crate::utils::test_temp::unique_temp_path("checkout-bin");
        let named_exe = named.join("target/debug/bus");
        std::fs::create_dir_all(named_exe.parent().unwrap()).unwrap();
        std::fs::write(
            named.join("Cargo.toml"),
            "[package]\nname = \"workspace-member\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"bus\"\npath = \"src/main.rs\"\n",
        )
        .unwrap();
        assert_eq!(source_checkout(&named_exe).unwrap(), named);

        let invalid = crate::utils::test_temp::unique_temp_path("checkout-invalid");
        let invalid_exe = invalid.join("nested/target/debug/bus");
        std::fs::create_dir_all(invalid_exe.parent().unwrap()).unwrap();
        std::fs::write(invalid.join("nested/Cargo.toml"), "not toml = [\n").unwrap();
        std::fs::create_dir_all(invalid.join("src")).unwrap();
        std::fs::write(invalid.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            invalid.join("Cargo.toml"),
            "[package]\nname = \"bus\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(source_checkout(&invalid_exe).unwrap(), invalid);

        let no_auto = crate::utils::test_temp::unique_temp_path("checkout-no-auto");
        std::fs::create_dir_all(no_auto.join("src")).unwrap();
        std::fs::write(no_auto.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            no_auto.join("Cargo.toml"),
            "[package]\nname = \"bus\"\nversion = \"0.1.0\"\nautobins = false\n",
        )
        .unwrap();
        let no_auto_exe = no_auto.join("target/debug/bus");
        std::fs::create_dir_all(no_auto_exe.parent().unwrap()).unwrap();
        assert!(source_checkout(&no_auto_exe).is_none());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
        std::fs::remove_dir_all(named).unwrap();
        std::fs::remove_dir_all(invalid).unwrap();
        std::fs::remove_dir_all(no_auto).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn source_restart_resolves_the_checkout_through_a_bus_cli_alias() {
        let root = crate::utils::test_temp::unique_temp_path("bus-alias-checkout");
        let checkout = root.join("source checkout");
        let image = checkout.join("target/debug/bus");
        let directory = root.join("session/cli-bin");
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(&image, b"image").unwrap();
        std::fs::write(checkout.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            checkout.join("Cargo.toml"),
            "[package]\nname = \"bus\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let alias = directory.join("bus");
        crate::platform::fs::refresh_executable_alias(&alias, &image).unwrap();
        let resolved = crate::platform::fs::resolve_running_executable(&alias).unwrap();
        assert_eq!(
            source_checkout(&resolved).unwrap(),
            checkout.canonicalize().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
