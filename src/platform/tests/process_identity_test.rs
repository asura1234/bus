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
    assert!(ancestry.iter().skip(1).all(|process| process.pid != child_instance.unwrap().pid));
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
