use super::*;

fn spans<'a>(text: &'a str, cwd: Option<&str>) -> Vec<(&'a str, Target)> {
    candidates(text, cwd.map(Path::new), Some(Path::new("/home/me")))
        .into_iter()
        .map(|candidate| (&text[candidate.range], candidate.target))
        .collect()
}

fn path(path: &str) -> Target {
    Target::Path(PathBuf::from(path))
}

#[test]
fn only_the_path_inside_quotes_and_brackets_is_a_link() {
    let text = r#"files: ["/Users/dylanliu/Desktop/lovart_产品设计文档.md"]"#;
    assert_eq!(
        spans(text, None),
        [(
            "/Users/dylanliu/Desktop/lovart_产品设计文档.md",
            path("/Users/dylanliu/Desktop/lovart_产品设计文档.md")
        )]
    );
}

#[test]
fn urls_drop_surrounding_brackets_and_trailing_punctuation() {
    assert_eq!(
        spans("see (https://example.com/a?b=1), then http://x.test.", None),
        [
            (
                "https://example.com/a?b=1",
                Target::Url("https://example.com/a?b=1".into())
            ),
            ("http://x.test", Target::Url("http://x.test".into())),
        ]
    );
    assert!(spans("ftp://example.com/file.txt", Some("/p")).is_empty());
}

#[test]
fn line_and_column_suffixes_are_underlined_but_not_opened() {
    assert_eq!(
        spans("at `src/main.rs:12:5`, and /etc/hosts:3.", Some("/repo")),
        [
            ("src/main.rs:12:5", path("/repo/src/main.rs")),
            ("/etc/hosts:3", path("/etc/hosts")),
        ]
    );
}

#[test]
fn home_and_relative_paths_resolve_and_plain_words_are_not_paths() {
    assert_eq!(
        spans("open ~/notes/a.md and Cargo.toml or temp/x", Some("/repo")),
        [
            ("~/notes/a.md", path("/home/me/notes/a.md")),
            ("Cargo.toml", path("/repo/Cargo.toml")),
            ("temp/x", path("/repo/temp/x")),
        ]
    );
    // Without a working directory only absolute and home paths count.
    assert_eq!(
        spans("Cargo.toml ~/a.md", None),
        [("~/a.md", path("/home/me/a.md"))]
    );
    assert!(
        spans("hello world, e.g. 1.5 and / alone; --flag.x", Some("/repo"))
            .iter()
            .all(|(text, _)| *text == "e.g")
    );
}

#[test]
fn only_existing_paths_link_and_the_check_never_blocks() {
    let dir = std::env::temp_dir().join(format!("bus-links-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("real.md");
    std::fs::write(&file, "x").unwrap();
    let text = format!(
        "    {} {}/missing.md https://a.test",
        file.display(),
        dir.display()
    );
    let mut probe = PathProbe::default();
    let links = |probe: &mut PathProbe| -> Vec<String> {
        row_links(probe, &text, 4, None)
            .into_iter()
            .map(|link| text[link.range].to_owned())
            .collect()
    };
    // The first frame only queues the checks; URLs need none.
    assert_eq!(links(&mut probe), ["https://a.test"]);
    probe.settle();
    assert_eq!(
        links(&mut probe),
        [file.display().to_string(), "https://a.test".into()]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn quoted_existing_paths_with_spaces_link_as_one_target() {
    let dir = Path::new("/tmp").join(format!("bus-links-spaces-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("My Document.md");
    std::fs::write(&file, "x").unwrap();
    let text = format!("files: [\"{}\"]", file.display());
    let mut probe = PathProbe::default();
    row_links(&mut probe, &text, 0, None);
    probe.settle();
    let links: Vec<_> = row_links(&mut probe, &text, 0, None)
        .into_iter()
        .map(|link| (text[link.range].to_owned(), link.target))
        .collect();
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(links, [(file.display().to_string(), Target::Path(file))]);
}

#[test]
fn urls_keep_balanced_parentheses_in_destination() {
    let url = "https://en.wikipedia.org/wiki/Function_(mathematics)";
    assert_eq!(
        spans(&format!("see {url}."), None),
        [(url, Target::Url(url.into()))]
    );
}

#[test]
fn network_share_paths_are_never_link_candidates() {
    // Probing `//host/share/...` makes Windows open an SMB session to `host`
    // while the row merely renders, before any click.
    assert!(spans("see //evil.example/share/report.txt", None).is_empty());
}

#[test]
fn the_probe_never_checks_a_network_share() {
    let mut probe = PathProbe::default();
    for share in ["//evil.example/share/a.md", r"\\evil.example\share\a.md"] {
        assert_eq!(probe.exists(Path::new(share)), Some(false), "{share}");
    }
    assert!(probe.worker.is_none(), "no check was queued");
    assert!(spans(r"see \\evil.example\share\report.txt", Some("/repo")).is_empty());
}

#[cfg(windows)]
#[test]
fn windows_share_forms_are_never_link_candidates() {
    for text in [
        r"\\host\share\a.txt",
        r"\\?\UNC\host\share\a.txt",
        r"\\.\pipe\bus",
        "//host/share/a.txt",
    ] {
        assert!(spans(text, Some(r"C:\repo")).is_empty(), "{text}");
    }
    // A relative path under a working directory on a share is on that share.
    assert!(spans("src/main.rs", Some(r"\\host\share\repo")).is_empty());
    assert_eq!(
        spans(r"C:\repo\a.txt", None),
        [(r"C:\repo\a.txt", path(r"C:\repo\a.txt"))]
    );
}

#[cfg(unix)]
#[test]
fn a_quoted_sentence_still_links_the_path_inside_it() {
    let dir = Path::new("/tmp").join(format!("bus-links-prose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("plan.md");
    std::fs::write(&file, "x").unwrap();
    let text = format!("he said \"see {} now\"", file.display());
    let mut probe = PathProbe::default();
    row_links(&mut probe, &text, 0, Some(&dir));
    probe.settle();
    let links: Vec<_> = row_links(&mut probe, &text, 0, Some(&dir))
        .into_iter()
        .map(|link| text[link.range].to_owned())
        .collect();
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(links, [file.display().to_string()]);
}
