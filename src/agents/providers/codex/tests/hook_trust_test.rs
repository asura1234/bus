use crate::agents::providers::hook_json::{
    codex_callback_binary, install_hooks, resume_hooks, HookContract,
};
use crate::agents::providers::ProviderKind;
use std::os::unix::fs::PermissionsExt;

fn scratch(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bus-codex-hook-trust-{name}-{}-{}",
        std::process::id(),
        crate::utils::time::now_ns()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn codex_hook_command_stays_stable_when_the_bus_executable_moves() {
    let root = scratch("move");
    let data = root.join("data");
    let project = root.join("project");
    let hooks = project.join(".codex/hooks.json");
    let old_exe = root.join("bin-old").join("bus");
    let new_exe = root.join("bin-new").join("bus");
    std::fs::create_dir_all(old_exe.parent().unwrap()).unwrap();
    std::fs::create_dir_all(new_exe.parent().unwrap()).unwrap();
    std::fs::write(&old_exe, b"old").unwrap();
    std::fs::write(&new_exe, b"new").unwrap();

    let bridge = codex_callback_binary(&data, &old_exe).unwrap();
    let contract = HookContract::for_provider(ProviderKind::Codex);
    install_hooks(&hooks, contract, &bridge).unwrap();
    let installed = std::fs::read(&hooks).unwrap();
    let command = contract.command(&bridge);
    assert!(
        std::str::from_utf8(&installed).unwrap().contains(&command),
        "installed hooks must name the stable bridge"
    );

    let moved = codex_callback_binary(&data, &new_exe).unwrap();
    assert_eq!(
        moved, bridge,
        "the hashed callback path must not follow the binary"
    );
    assert!(
        !resume_hooks(&hooks, contract, &moved).unwrap(),
        "a new Bus executable must not rewrite a trusted bridge command"
    );
    assert_eq!(std::fs::read(&hooks).unwrap(), installed);

    let script = String::from_utf8(std::fs::read(&bridge).unwrap()).unwrap();
    assert!(
        script.contains(&new_exe.display().to_string()),
        "bridge must exec the replacement binary: {script}"
    );
    assert!(
        !script.contains(&old_exe.display().to_string()),
        "bridge must drop the previous binary: {script}"
    );
    let mode = std::fs::metadata(&bridge).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o700,
        "callback bridge is private and executable"
    );
    let _ = std::fs::remove_dir_all(&root);
}

struct BridgeScratch(std::path::PathBuf);

impl Drop for BridgeScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn codex_hook_bridge_executes_quoted_paths_and_rebinds_only_once() {
    let cleanup = BridgeScratch(scratch("quoted-paths"));
    let root = &cleanup.0;
    let data = root.join("data 'quoted' & spaces");
    let project_hooks = root.join("project/.codex/hooks.json");
    let old = root.join("old bin/bus 'one' $name");
    let new = root.join("new bin/bus 'two' $name");
    for (path, marker) in [(&old, "old"), (&new, "new")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("#!/bin/sh\nprintf '{marker}\\000'\nprintf '%s\\000' \"$@\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let contract = HookContract::for_provider(ProviderKind::Codex);
    install_hooks(&project_hooks, contract, &old).unwrap();
    let bridge = codex_callback_binary(&data, &old).unwrap();
    assert!(resume_hooks(&project_hooks, contract, &bridge).unwrap());
    let trusted = std::fs::read(&project_hooks).unwrap();
    assert!(!resume_hooks(&project_hooks, contract, &bridge).unwrap());
    let command = format!(
        "{} 'space value' \"quote'value\"",
        contract.command(&bridge)
    );
    let execute = || {
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    assert_eq!(
        execute(),
        b"old\0--bus-callback\0codex-hook\0space value\0quote'value\0"
    );
    assert_eq!(codex_callback_binary(&data, &new).unwrap(), bridge);
    assert!(!resume_hooks(&project_hooks, contract, &bridge).unwrap());
    assert_eq!(std::fs::read(&project_hooks).unwrap(), trusted);
    let expected = b"new\0--bus-callback\0codex-hook\0space value\0quote'value\0";
    std::thread::scope(|scope| {
        let callbacks = scope.spawn(|| {
            for _ in 0..32 {
                assert_eq!(execute(), expected);
            }
        });
        for _ in 0..32 {
            assert_eq!(codex_callback_binary(&data, &new).unwrap(), bridge);
        }
        callbacks.join().unwrap();
    });
    assert_eq!(
        std::fs::metadata(&bridge).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn codex_hook_bridge_does_not_chmod_a_same_content_symlink_target() {
    let cleanup = BridgeScratch(scratch("same-body-symlink"));
    let root = &cleanup.0;
    let data = root.join("data");
    let executable = root.join("bus");
    let bridge = codex_callback_binary(&data, &executable).unwrap();
    let outside = root.join("outside-callbacks-script");
    std::fs::rename(&bridge, &outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&outside, &bridge).unwrap();
    let result = codex_callback_binary(&data, &executable);
    let mode = std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o644,
        "bridge creation changed the symlink target permissions; result={result:?}"
    );
    assert!(
        result.is_err(),
        "an existing bridge symlink must be rejected"
    );
}

#[test]
fn codex_hook_bridge_repairs_permissions_without_chmodding_an_existing_inode() {
    use std::os::unix::fs::MetadataExt;

    let cleanup = BridgeScratch(scratch("hardlink-mode"));
    let root = &cleanup.0;
    let data = root.join("data");
    let executable = root.join("bus");
    std::fs::write(&executable, b"#!/bin/sh\nprintf repaired\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bridge = codex_callback_binary(&data, &executable).unwrap();
    let outside = root.join("outside-script");
    std::fs::hard_link(&bridge, &outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(codex_callback_binary(&data, &executable).unwrap(), bridge);
    let outside_meta = std::fs::metadata(&outside).unwrap();
    assert_eq!(outside_meta.permissions().mode() & 0o777, 0o644);
    let installed_meta = std::fs::metadata(&bridge).unwrap();
    assert_eq!(installed_meta.permissions().mode() & 0o777, 0o700);
    assert_ne!(installed_meta.ino(), outside_meta.ino());
    let output = std::process::Command::new(&bridge).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"repaired");

    // Already-correct files stay published while other callbacks execute.
    assert_eq!(codex_callback_binary(&data, &executable).unwrap(), bridge);
    assert_eq!(
        std::fs::metadata(&bridge).unwrap().ino(),
        installed_meta.ino()
    );
}

#[test]
fn codex_hook_bridge_refuses_a_non_regular_leaf() {
    let cleanup = BridgeScratch(scratch("directory-leaf"));
    let data = cleanup.0.join("data");
    let executable = cleanup.0.join("bus");
    let bridge = codex_callback_binary(&data, &executable).unwrap();
    std::fs::remove_file(&bridge).unwrap();
    std::fs::create_dir(&bridge).unwrap();

    assert!(codex_callback_binary(&data, &executable).is_err());
    assert!(bridge.is_dir());
    assert_eq!(std::fs::read_dir(&bridge).unwrap().count(), 0);
}
