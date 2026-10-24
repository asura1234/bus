use super::*;

#[test]
fn unknown_process_cannot_authorize_even_a_new_prompt_mark() {
    let mut command = CommandBuilder::new("/bin/bash");
    let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
        "BUS_LAUNCH_ID".into(),
        "unknown-process".into(),
    )]);
    let Some(mut integration) =
        ShellIntegration::install_for_agent_launch(&mut command, &launch).unwrap()
    else {
        return;
    };
    integration.start_wait(None);
    assert!(integration.waiting_for_prompt(None, 1));
    assert!(integration.waiting_for_prompt(Some(u32::MAX), 2));
}

#[test]
fn fork_image_cannot_authorize_even_a_launch_specific_prompt_mark() {
    let mut command = CommandBuilder::new("/bin/bash");
    let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
        "BUS_LAUNCH_ID".into(),
        "fork-image".into(),
    )]);
    let Some(mut integration) =
        ShellIntegration::install_for_agent_launch(&mut command, &launch).unwrap()
    else {
        return;
    };
    integration.start_wait(Some(std::process::id()));
    assert!(integration.waiting_for_prompt(Some(std::process::id()), 1));
}

#[test]
fn concurrent_agent_panes_own_distinct_shell_integration_directories() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut command = CommandBuilder::new("/bin/bash");
                let launch = super::super::spawn::PaneLaunchEnv::from_extra(vec![(
                    "BUS_LAUNCH_ID".into(),
                    "parallel-panes".into(),
                )]);
                barrier.wait();
                ShellIntegration::install_for_agent_launch(&mut command, &launch).unwrap()
            })
        })
        .collect();
    let mut integrations: Vec<_> = threads
        .into_iter()
        .filter_map(|thread| thread.join().unwrap())
        .collect();
    if integrations.is_empty() {
        return;
    }
    let directories: std::collections::BTreeSet<_> =
        integrations.iter().map(|i| i.directory.clone()).collect();
    assert_eq!(directories.len(), 8);
    drop(integrations.pop());
    assert!(integrations
        .iter()
        .all(|i| i.directory.join("bashrc").is_file()));
    drop(integrations);
    assert!(directories.iter().all(|path| !path.exists()));
}
