#[test]
fn local_attach_command_resumes_the_local_session_or_relaunches_an_explicit_root() {
    assert_eq!(
        attach_command_for(Some("0123456789abcdef")),
        "bus resume 0123456789abcdef"
    );
    assert_eq!(attach_command_for(None), "bus");
}

#[test]
fn restart_after_update_guidance_names_stop_and_attach_commands() {
    assert_eq!(
        restart_after_update_guidance("bus stop", "bus resume 0123456789abcdef"),
        "Stop the old server to use the new version.\nStopping exits pane processes.\nRun `bus stop`, then run `bus resume 0123456789abcdef` again."
    );
}
