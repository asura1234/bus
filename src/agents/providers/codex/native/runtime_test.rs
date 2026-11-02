use super::*;

fn stop(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}

#[test]
fn codex_native_server_exits_when_its_bus_lifetime_pipe_closes() {
    let dir = crate::utils::test_temp::unique_temp_path("codex-lifetime");
    fs::create_dir_all(&dir).unwrap();
    let lock = acquire_launch_lock(&dir.join("owner")).unwrap();
    let mut child = owned_command(&["/bin/sleep".into(), "60".into()], &lock)
        .process_group(0)
        .spawn()
        .unwrap();
    let pipe = child.stdin.take();
    let has_pipe = pipe.is_some();
    drop(pipe);
    let deadline = Instant::now() + Duration::from_secs(3);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let ended = matches!(child.try_wait(), Ok(Some(_)));
    if !ended {
        stop(&mut child);
    }
    assert!(
        has_pipe && ended,
        "a dead Bus must not leave a writer runtime running"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_native_server_holds_the_writer_lock_after_its_bus_handle_drops() {
    if std::env::var_os("BUS_CODEX_LOCK_PROBE").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "agents::providers::codex::native::runtime::tests::codex_native_server_holds_the_writer_lock_after_its_bus_handle_drops", "--test-threads=1"])
            .env("BUS_CODEX_LOCK_PROBE", "1")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = crate::utils::test_temp::unique_temp_path("codex-lock");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("owner");
    let lock = acquire_launch_lock(&path).unwrap();
    // A single child owns the inherited descriptor. Reaping a supervisor is
    // not a barrier for its descendants' descriptor teardown.
    let mut command = Command::new("/bin/sleep");
    command.arg("60").process_group(0);
    inherit_writer_lock(&mut command, &lock);
    let mut child = command.spawn().unwrap();
    drop(lock);
    let second = acquire_launch_lock(&path);
    let exclusive = matches!(second,Err(ref e) if e.kind()==io::ErrorKind::WouldBlock);
    drop(second);
    drop(child.stdin.take());
    stop(&mut child);
    let released = acquire_launch_lock(&path).is_ok();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        exclusive,
        "a second runtime must not acquire the same launch while its writer survives"
    );
    assert!(
        released,
        "a stopped runtime must permit the validated cold resume"
    );
}
