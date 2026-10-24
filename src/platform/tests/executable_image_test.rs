#[test]
fn launch_prompt_executable_resolution_uses_pane_path_cwd_and_permissions() {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    let dir = crate::utils::test_temp::unique_temp_path("bus-pane-executable-image");
    std::fs::create_dir_all(dir.join("not-executable")).unwrap();
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    for (name, mode) in [("not-executable/bash", 0o600), ("bin/bash", 0o700)] {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let expected = dir.join("bin/bash").canonicalize().unwrap();
    let search_path = std::env::join_paths(["not-executable", "bin"]).unwrap();
    assert_eq!(
        resolve_executable_image(Path::new("bash"), Some(&search_path), &dir),
        Some(expected.clone()),
        "skip the non-executable candidate and use the pane's relative PATH"
    );
    assert_eq!(
        resolve_executable_image(Path::new("./bin/bash"), None, &dir),
        Some(expected),
        "a relative program uses the pane cwd without consulting server PATH"
    );
    assert_eq!(
        resolve_executable_image(Path::new("bash"), None, &dir),
        None
    );
    std::fs::remove_dir_all(dir).unwrap();
}
