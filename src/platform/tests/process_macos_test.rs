#[test]
fn nofile_target_raises_low_soft_limit_to_cap_when_hard_is_unlimited() {
    assert_eq!(
        target_nofile_soft_limit(256, libc::RLIM_INFINITY, 8192),
        Some(8192)
    );
}

#[test]
fn nofile_target_respects_finite_hard_limit() {
    assert_eq!(target_nofile_soft_limit(256, 4096, 8192), Some(4096));
}

#[test]
fn nofile_target_does_not_lower_existing_soft_limit() {
    assert_eq!(
        target_nofile_soft_limit(16_384, libc::RLIM_INFINITY, 8192),
        None
    );
}

fn build_procargs2(exec_path: &str, argv: &[&str], env: &[&str]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(argv.len() as i32).to_ne_bytes());
    buf.extend_from_slice(exec_path.as_bytes());
    buf.push(0);
    buf.push(0);
    for arg in argv {
        buf.extend_from_slice(arg.as_bytes());
        buf.push(0);
    }
    for entry in env {
        buf.extend_from_slice(entry.as_bytes());
        buf.push(0);
    }
    buf
}

#[test]
fn procargs2_argv_excludes_environment_entries() {
    let buf = build_procargs2(
        "/usr/bin/node",
        &["node", "/Users/can/.local/bin/pi"],
        &[
            "PATH=/usr/bin:/var/run/com.apple.security.cryptexd/codex.system/bootstrap/usr/bin",
            "TERM=tmux-256color",
        ],
    );

    let argv = procargs2_argv(&buf).expect("expected argv");
    assert_eq!(argv, vec!["node", "/Users/can/.local/bin/pi"]);
    assert_eq!(argv.join(" "), "node /Users/can/.local/bin/pi");
    assert!(!argv.join(" ").contains("codex.system"));
}

#[test]
fn procargs2_env_reads_agent_hint_after_argv() {
    let buf = build_procargs2(
        "/opt/homebrew/bin/nono",
        &["nono", "run", "HERDR_AGENT=codex", "--", "claude"],
        &["PATH=/usr/bin", "HERDR_AGENT=claude", "TERM=xterm-256color"],
    );

    let env = procargs2_env(&buf).expect("expected env block");
    assert_eq!(
        env,
        b"PATH=/usr/bin\0HERDR_AGENT=claude\0TERM=xterm-256color\0"
    );
}

#[test]
fn procargs2_env_does_not_treat_argv_as_environment() {
    let buf = build_procargs2(
        "/opt/homebrew/bin/nono",
        &["nono", "run", "HERDR_AGENT=claude"],
        &["PATH=/usr/bin"],
    );

    let env = procargs2_env(&buf).expect("expected env block");
    assert_eq!(env, b"PATH=/usr/bin\0");
}

#[test]
fn procargs2_argv_preserves_empty_arguments() {
    let buf = build_procargs2(
        "/usr/bin/node",
        &["node", "/opt/bin/codex", "", "--verbose"],
        &["PATH=/usr/bin"],
    );

    assert_eq!(
        procargs2_argv(&buf),
        Some(vec![
            "node".to_string(),
            "/opt/bin/codex".to_string(),
            String::new(),
            "--verbose".to_string(),
        ]),
        "an empty positional argument must not discard the entire process argv"
    );
    assert_eq!(procargs2_env(&buf), Some(&b"PATH=/usr/bin\0"[..]));
}

#[test]
fn process_argv_reads_a_live_process_with_empty_arguments() {
    struct ChildGuard(std::process::Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let ordinary_child = ChildGuard(
        Command::new("/bin/sh")
            .args(["-c", "read value", "ordinary"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the ordinary argv control"),
    );
    let ordinary_argv = process_argv(ordinary_child.0.id());
    drop(ordinary_child);
    assert_eq!(
        ordinary_argv,
        Some(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "read value".to_string(),
            "ordinary".to_string(),
        ]),
        "the native argv reader must work for the ordinary control process"
    );

    let child = ChildGuard(
        Command::new("/bin/sh")
            .args(["-c", "read value", ""])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a shell waiting on its piped stdin"),
    );
    let argv = process_argv(child.0.id());
    drop(child);

    assert_eq!(
        argv,
        Some(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "read value".to_string(),
            String::new(),
        ]),
        "valid empty arguments returned by the kernel must be preserved"
    );
}
