fn spawn_waiting_child() -> std::process::Child {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        command
    };
    command.spawn().expect("spawn a waiting child")
}

#[test]
fn ancestry_names_the_child_then_this_process_by_incarnation() {
    let me = process_instance(std::process::id()).expect("this process is readable");
    let mut child = spawn_waiting_child();
    let ancestry = process_ancestry(child.id());
    let child_instance = process_instance(child.id());
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(ancestry.first().copied(), child_instance);
    assert_eq!(ancestry.get(1).copied(), Some(me));
    assert!(ancestry
        .iter()
        .skip(1)
        .all(|process| process.pid != child_instance.unwrap().pid));
}

#[test]
fn an_exited_process_has_no_instance() {
    let mut child = spawn_waiting_child();
    let pid = child.id();
    assert!(process_instance(pid).is_some());
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(process_instance(pid), None);
}

#[cfg(target_os = "macos")]
#[test]
fn caller_boundary_orders_live_incarnations_and_refuses_stale_or_exited_processes() {
    let me = process_instance(std::process::id()).unwrap();
    let mut child = spawn_waiting_child();
    let server = process_instance(child.id()).unwrap();
    let before = caller_precedes_server(me, server);
    let after = caller_precedes_server(server, me);
    let same = caller_precedes_server(me, me);
    let stale = caller_precedes_server(
        ProcessInstance {
            birth: me.birth.wrapping_add(1),
            ..me
        },
        server,
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let exited = caller_precedes_server(me, server);
    assert_eq!(before, Some(true));
    assert_eq!(after, Some(false));
    assert_eq!(same, Some(false));
    assert_eq!(stale, None);
    assert_eq!(exited, None);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn caller_boundary_is_explicitly_unknown_without_macos_metadata() {
    let unknown = ProcessInstance {
        pid: std::process::id(),
        birth: 0,
    };
    assert_eq!(caller_precedes_server(unknown, unknown), None);
}
