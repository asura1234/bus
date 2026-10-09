// Included in the same legacy launch test namespace.

#[test]
fn suggestions_report_directory_kind_without_ui_filesystem_reads() {
    let dir = std::env::temp_dir().join(format!("bus-suggestions-{}", bus_io::now_ns()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::create_dir(dir.join("folder")).unwrap();
    std::fs::write(dir.join("file.md"), b"fixture").unwrap();
    let input = format!("{}/", dir.display());
    let entries = suggestions(&input, false).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        !entries
            .iter()
            .find(|entry| entry.path == dir.join("file.md"))
            .unwrap()
            .is_directory
    );
    assert!(
        entries
            .iter()
            .find(|entry| entry.path == dir.join("folder"))
            .unwrap()
            .is_directory
    );
    let directories = suggestions(&input, true).unwrap();
    assert_eq!(directories.len(), 1);
    assert_eq!(directories[0].path, dir.join("folder"));
    assert!(directories[0].is_directory);
    std::fs::remove_dir_all(dir).unwrap();
}
