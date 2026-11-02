use super::*;
use crate::utils::restart_record::BuildStamp;

fn report_server(driver: &mut ClientLoopDriver, sha: Option<&str>) {
    let stamp = sha.map(|sha| BuildStamp {
        sha: sha.to_owned(),
        pid: Some(22),
        binary: Some("/server/bus".into()),
    });
    driver
        .state
        .shell
        .as_mut()
        .unwrap()
        .set_bus_server_build(stamp.as_ref());
}

fn footer(driver: &mut ClientLoopDriver) -> String {
    let ui = driver.state.shell.as_mut().unwrap().bus.as_mut().unwrap();
    ui.compute_view(120, 40);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 120, 40));
    ui.render(&mut buffer);
    let row = (1..120)
        .map(|x| buffer[(x, 38)].symbol())
        .take_while(|symbol| *symbol != "│")
        .collect::<String>();
    let label = row.trim_end().to_owned();
    for y in 0..40 {
        let main = (28..120)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>();
        assert!(!main.contains(&label), "version must stay in the sidebar");
        assert!(!main.contains("TUI"));
    }
    assert!(!label.contains("TUI"), "show only the server version");
    label
}

#[test]
fn footer_version_renders_only_the_reported_server_sha() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let client = env!("BUS_BUILD_SHA");
    let server = if client.starts_with("ab") { "cd" } else { "ab" }.repeat(20);
    // Restart and rollback create a new UI; neither reattaches the old driver.
    for sha in [client, server.as_str(), client] {
        let (mut driver, _) = driver(120);
        report_server(&mut driver, Some(sha));
        assert_eq!(footer(&mut driver), &sha[..8]);
    }
}

#[test]
fn footer_version_unknown_server_is_not_replaced_by_the_client_sha() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for sha in [None, Some("invalid")] {
        let (mut driver, _) = driver(120);
        report_server(&mut driver, sha);
        assert_eq!(footer(&mut driver), "unknown");
    }
}

#[test]
fn footer_version_server_sha_fits_the_minimum_sidebar_width() {
    let _lock = crate::utils::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (mut driver, _) = driver(120);
    receive_width(&mut driver, 20);
    let client = env!("BUS_BUILD_SHA");
    let server = if client.starts_with("ab") { "cd" } else { "ab" }.repeat(20);
    report_server(&mut driver, Some(&server));
    assert_eq!(footer(&mut driver), &server[..8]);
}
