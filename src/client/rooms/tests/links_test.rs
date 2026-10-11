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
