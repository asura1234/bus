#[test]
fn double_click_word_bounds_cover_terminal_text() {
    let cases = [
        (
            "see https://example.com/a-b_c?q=x@y.",
            "example.com",
            "https://example.com/a-b_c?q=x@y",
        ),
        (
            "open \"https://example.com/a,b;c?q=x\";",
            "example.com",
            "https://example.com/a,b;c?q=x",
        ),
        (
            "see https://en.wikipedia.org/wiki/Foo_(bar_(baz)),",
            "wikipedia",
            "https://en.wikipedia.org/wiki/Foo_(bar_(baz))",
        ),
        (
            "see https://example.com/a(b[c{d}e]f),",
            "example.com",
            "https://example.com/a(b[c{d}e]f)",
        ),
        (
            "see (https://example.com/a(b(c)d)))",
            "example.com",
            "https://example.com/a(b(c)d)",
        ),
        (
            "open /tmp/foo-bar/baz_qux/",
            "foo-bar",
            "/tmp/foo-bar/baz_qux/",
        ),
        (
            "open ./src/app/actions.rs:795",
            "actions",
            "./src/app/actions.rs:795",
        ),
        (
            "open ../herdr-worktrees/issue-1",
            "herdr",
            "../herdr-worktrees/issue-1",
        ),
        (
            "edit src/app/actions.rs,then",
            "actions",
            "src/app/actions.rs",
        ),
        (
            "cat \"/tmp/build output/log.txt\"",
            "output",
            "/tmp/build output/log.txt",
        ),
        (
            "cat '/Users/me/Library/Application Support/app/config.json'",
            "Support",
            "/Users/me/Library/Application Support/app/config.json",
        ),
        ("echo 你好-world done", "好", "你好-world"),
        ("先跑 cargo test", "cargo", "cargo"),
        (
            "export PATH=$HOME/.cargo/bin:$PATH",
            "$HOME",
            "PATH=$HOME/.cargo/bin:$PATH",
        ),
        (
            "git checkout feature/foo-bar_baz",
            "foo",
            "feature/foo-bar_baz",
        ),
        ("refs #123 and @owner/name", "#123", "#123"),
        ("refs #123 and @owner/name", "owner", "@owner/name"),
        ("cargo test --package=herdr", "--package", "--package=herdr"),
        (
            "cargo test app::actions::tests",
            "app::",
            "app::actions::tests",
        ),
        (
            "image ghcr.io/org/app:latest",
            "ghcr",
            "ghcr.io/org/app:latest",
        ),
        ("ERROR [worker-1] request_id=abc-123", "worker", "worker-1"),
        (
            "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
            "newhoo",
            "newhoo",
        ),
        (
            "render_status_line(app, area)",
            "render",
            "render_status_line",
        ),
        ("render_status_line(app, area)", "app", "app"),
        ("render_status_line(app, area)", "area", "area"),
        ("if !enabled {", "enabled", "enabled"),
        ("println!(\"hi\")", "println", "println"),
        ("( master)$", "master", "master"),
        ("regex foo$", "foo", "foo$"),
    ];

    for (row, click, expected) in cases {
        assert_selects(row, click, expected);
    }

    let row = "echo 你好-world done";
    assert_eq!(
        selected_word(row, col_of(row, "好") + 1).as_deref(),
        Some("你好-world")
    );
}

#[test]
fn double_click_word_bounds_ignore_delimiters() {
    for (row, click) in [
        (
            "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
            "|",
        ),
        ("alpha,beta;gamma", ","),
        ("alpha,beta;gamma", ";"),
        ("render_status_line(app, area)", "("),
        ("render_status_line(app, area)", ")"),
        ("if !enabled {", "!"),
        ("if !enabled {", "{"),
        ("(done).", "("),
        ("(done).", "."),
    ] {
        assert_selects_nothing(row, click);
    }
}

#[test]
fn url_at_column_returns_safe_visible_url_only() {
    assert_eq!(
        selected_url("see https://example.com/a(b)c.", "example"),
        Some("https://example.com/a(b)c")
    );
    assert_eq!(
        selected_url("[docs](https://example.com/docs),", "example"),
        Some("https://example.com/docs")
    );
    assert_eq!(
        selected_url("[docs](https://example.com/docs)", "docs"),
        None
    );
    assert_eq!(selected_url("open file:///tmp/report", "file"), None);
}
