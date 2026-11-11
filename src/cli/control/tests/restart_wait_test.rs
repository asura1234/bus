use super::*;

struct PlannedRestartFixture {
    root: std::path::PathBuf,
    previous: Option<std::path::PathBuf>,
}

impl PlannedRestartFixture {
    fn new() -> Self {
        use crate::utils::restart_record as restart;
        let root = crate::utils::test_temp::unique_temp_path("async-planned-restart");
        let previous = wait::RESTART_TEST_ROOT.with(|slot| slot.replace(Some(root.clone())));
        let fixture = Self { root, previous };
        fixture.generation(2);
        let live = crate::platform::process_instance(std::process::id()).unwrap();
        let pid = restart::LivePid {
            pid: live.pid,
            birth: live.birth,
        };
        let plan = restart::HandoffPlan {
            id: "planned-async-restart".into(),
            root: fixture.root.clone(),
            old_binary: "/old/bus".into(),
            new_binary: "/new/bus".into(),
            old_sha: "a".repeat(40),
            new_sha: "b".repeat(40),
            who: "user:test".into(),
            tui: pid.clone(),
            session_id: None,
            explicit_data_dir: true,
            dev: true,
            yolo: false,
            clear_data: false,
        };
        restart::write_json(&restart::handoff_path(&fixture.root), &plan).unwrap();
        restart::write_json(&restart::restart_lock_path(&fixture.root), &pid).unwrap();
        fixture
    }

    fn generation(&self, generation: u64) {
        crate::utils::restart_record::write_json(
            &self
                .root
                .join("herdr-config/sessions/bus/restarts/generation.json"),
            &generation,
        )
        .unwrap();
    }
}

impl Drop for PlannedRestartFixture {
    fn drop(&mut self) {
        wait::RESTART_TEST_ROOT.with(|slot| slot.replace(self.previous.take()));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn send_async_survives_planned_restart_and_new_generation() {
    let restart = PlannedRestartFixture::new();
    let mut reads = 0;
    let response = follow_message(
        "restart-follow",
        &json!(19),
        &mut |request, _| {
            reads += 1;
            if reads == 1 {
                return Ok(Response::success(
                    &request.id,
                    status(vec![recipient(1, "delivered", false)]),
                ));
            }
            if reads <= 7 {
                return Err("connection refused".into());
            }
            restart.generation(3);
            Ok(Response::success(
                &request.id,
                status(vec![recipient(1, "replied", true)]),
            ))
        },
        &mut |_| {},
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(reads, 8);
    assert_eq!(response.result["complete"], true);
}

#[test]
fn wait_async_survives_planned_restart_and_new_generation() {
    let restart = PlannedRestartFixture::new();
    let mut reads = 0;
    let response = execute_with_pause(
        command(&["wait", "--message", "19", "--async"]).unwrap(),
        |request, _| {
            reads += 1;
            if reads <= 6 {
                return Err("connection refused".into());
            }
            restart.generation(3);
            Ok(Response::success(
                &request.id,
                status(vec![recipient(1, "replied", true)]),
            ))
        },
        &mut |_| {},
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(reads, 7);
}

#[test]
fn async_follower_does_not_extend_a_stale_restart_without_live_helper() {
    let restart = PlannedRestartFixture::new();
    std::fs::remove_file(crate::utils::restart_record::restart_lock_path(
        &restart.root,
    ))
    .unwrap();
    let mut reads = 0;
    let response = follow_message(
        "stale-restart",
        &json!(19),
        &mut |_, _| {
            reads += 1;
            assert!(reads < 10, "a stale handoff must not retain a dead Bus");
            Err("connection refused".into())
        },
        &mut |_| {},
    );
    assert_eq!(response.error.unwrap().code, "bus_down");
    assert_eq!(reads, 4);
}

#[test]
fn async_follower_bounds_a_planned_restart_that_never_recovers() {
    let _restart = PlannedRestartFixture::new();
    let mut reads = 0;
    let mut paused = Duration::ZERO;
    let response = follow_message(
        "hung-restart",
        &json!(19),
        &mut |_, _| {
            reads += 1;
            assert!(
                reads <= 200,
                "a live helper is not an unlimited outage lease"
            );
            Err("connection refused".into())
        },
        &mut |duration| paused += duration,
    );
    assert_eq!(response.error.unwrap().code, "bus_down");
    assert!(
        reads > 4,
        "the planned restart must receive reconnect grace"
    );
    assert!(paused <= Duration::from_secs(180), "{paused:?}");
}

#[test]
fn async_follower_rejects_a_restart_for_another_root_or_generation() {
    for wrong_root in [false, true] {
        let restart = PlannedRestartFixture::new();
        let path = crate::utils::restart_record::handoff_path(&restart.root);
        let mut plan = crate::utils::restart_record::read_handoff(&path)
            .unwrap()
            .unwrap();
        if wrong_root {
            plan.root = restart.root.join("another-session");
        } else {
            // An unrelated second advance must not turn a stale plan into a
            // grace lease for a different server lifecycle.
            restart.generation(4);
        }
        crate::utils::restart_record::write_json(&path, &plan).unwrap();
        let mut reads = 0;
        let response = follow_message(
            "unrelated-restart",
            &json!(19),
            &mut |_, _| {
                reads += 1;
                if !wrong_root && reads == 1 {
                    restart.generation(6);
                }
                assert!(reads < 10);
                Err("connection refused".into())
            },
            &mut |_| {},
        );
        assert_eq!(response.error.unwrap().code, "bus_down");
        assert_eq!(reads, 4);
    }
}

#[test]
fn async_follower_follows_consumed_handoff_only_during_active_restart() {
    use crate::utils::restart_record as restart;
    for active in [true, false] {
        let fixture = PlannedRestartFixture::new();
        std::fs::rename(
            restart::handoff_path(&fixture.root),
            fixture.root.join("restarts/active.json"),
        )
        .unwrap();
        if active {
            std::fs::write(restart::marker_path(&fixture.root), "restarting").unwrap();
        }
        let mut reads = 0;
        let response = follow_message(
            "consumed-handoff",
            &json!(19),
            &mut |request, _| {
                reads += 1;
                if reads <= 6 {
                    return Err("connection refused".into());
                }
                fixture.generation(3);
                Ok(Response::success(
                    &request.id,
                    status(vec![recipient(1, "replied", true)]),
                ))
            },
            &mut |_| {},
        );
        if active {
            assert!(response.ok, "{response:?}");
            assert_eq!(reads, 7);
        } else {
            assert_eq!(response.error.unwrap().code, "bus_down");
            assert_eq!(reads, 4, "a past active.json is not a restart lease");
        }
    }
}

#[cfg(unix)]
#[test]
fn async_follower_matches_the_same_data_directory_through_a_path_alias() {
    struct Alias(std::path::PathBuf);
    impl Drop for Alias {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let fixture = PlannedRestartFixture::new();
    let alias = Alias(crate::utils::test_temp::unique_temp_path(
        "restart-parent-alias",
    ));
    std::os::unix::fs::symlink(fixture.root.parent().unwrap(), &alias.0).unwrap();
    let selected = alias.0.join(fixture.root.file_name().unwrap());
    assert_eq!(
        selected.canonicalize().unwrap(),
        fixture.root.canonicalize().unwrap()
    );
    wait::RESTART_TEST_ROOT.with(|slot| slot.replace(Some(selected)));
    let mut reads = 0;
    let response = follow_message(
        "root-alias",
        &json!(19),
        &mut |request, _| {
            reads += 1;
            if reads <= 6 {
                return Err("connection refused".into());
            }
            fixture.generation(3);
            Ok(Response::success(
                &request.id,
                status(vec![recipient(1, "replied", true)]),
            ))
        },
        &mut |_| {},
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(reads, 7);
}
