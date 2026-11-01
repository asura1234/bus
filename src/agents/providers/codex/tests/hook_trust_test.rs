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
