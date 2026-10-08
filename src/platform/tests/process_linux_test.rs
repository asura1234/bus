#[test]
fn process_detection_mode_requires_explicit_child_groups_value() {
    assert_eq!(
        parse_process_detection_mode(None),
        Ok(ProcessDetectionMode::Native)
    );
    assert_eq!(
        parse_process_detection_mode(Some("")),
        Ok(ProcessDetectionMode::Native)
    );
    assert_eq!(
        parse_process_detection_mode(Some("native")),
        Ok(ProcessDetectionMode::Native)
    );
    assert_eq!(
        parse_process_detection_mode(Some("child-groups")),
        Ok(ProcessDetectionMode::ChildGroups)
    );
    assert_eq!(parse_process_detection_mode(Some("gvisor")), Err("gvisor"));
}

#[test]
fn child_groups_foreground_group_picks_the_newest_job() {
    let tasks = HashMap::from([(100, vec![100])]);
    let children = HashMap::from([((100, 100), vec![200, 300])]);
    let groups = HashMap::from([(200, 200), (300, 300)]);

    let group = child_groups_foreground_process_group_with(
        100,
        100,
        |pid| tasks.get(&pid).cloned().unwrap_or_default(),
        |pid, tid| children.get(&(pid, tid)).cloned().unwrap_or_default(),
        |pid| groups.get(&pid).copied(),
    );

    assert_eq!(group, Some(300));
}

#[test]
fn child_groups_foreground_group_returns_to_the_shell_group() {
    let tasks = HashMap::from([(100, vec![100])]);
    let children = HashMap::from([((100, 100), vec![150, 160])]);
    let groups = HashMap::from([(150, 90), (160, 90)]);

    let group = child_groups_foreground_process_group_with(
        100,
        90,
        |pid| tasks.get(&pid).cloned().unwrap_or_default(),
        |pid, tid| children.get(&(pid, tid)).cloned().unwrap_or_default(),
        |pid| groups.get(&pid).copied(),
    );

    assert_eq!(group, Some(90));
}

#[test]
fn child_groups_foreground_group_skips_the_shell_group() {
    let tasks = HashMap::from([(100, vec![100])]);
    let children = HashMap::from([((100, 100), vec![150, 160, 300])]);
    let groups = HashMap::from([(150, 90), (160, 90), (300, 300)]);

    let group = child_groups_foreground_process_group_with(
        100,
        90,
        |pid| tasks.get(&pid).cloned().unwrap_or_default(),
        |pid, tid| children.get(&(pid, tid)).cloned().unwrap_or_default(),
        |pid| groups.get(&pid).copied(),
    );

    assert_eq!(group, Some(300));
}

#[test]
fn child_groups_foreground_group_fails_closed_at_the_scan_limit() {
    let children: Vec<u32> = (1..=(CHILD_GROUPS_SCAN_LIMIT as u32 + 10)).collect();
    let mut inspected = 0usize;

    let group = child_groups_foreground_process_group_with(
        100,
        100,
        |_| vec![100],
        |_, _| children.clone(),
        |pid| {
            inspected += 1;
            Some(pid as i32)
        },
    );

    assert_eq!(inspected, CHILD_GROUPS_SCAN_LIMIT);
    assert_eq!(group, None);
}

#[test]
fn foreground_members_follow_the_pane_tree_and_filter_by_process_group() {
    let tasks = HashMap::from([
        (100, vec![100, 101]),
        (200, vec![200]),
        (201, vec![201]),
        (210, vec![210]),
        (220, vec![220]),
        (221, vec![221]),
        (300, vec![300]),
    ]);
    let children = HashMap::from([
        ((100, 100), vec![200, 201, 300]),
        ((100, 101), vec![210]),
        ((200, 200), vec![220]),
        ((220, 220), vec![221]),
    ]);
    let processes = HashMap::from([
        (100, (100, "shell")),
        (200, (200, "leader")),
        (201, (200, "pipeline")),
        (210, (200, "thread-child")),
        (220, (220, "intermediate")),
        (221, (200, "nested-agent")),
        (300, (300, "background")),
        (9999, (200, "unrelated-host-process")),
    ]);
    let task_reads = RefCell::new(Vec::new());
    let child_reads = RefCell::new(Vec::new());
    let member_reads = RefCell::new(Vec::new());

    let members = foreground_process_group_members_with(
        100,
        200,
        |pid| {
            task_reads.borrow_mut().push(pid);
            tasks.get(&pid).cloned().unwrap_or_default()
        },
        |pid, tid| {
            child_reads.borrow_mut().push((pid, tid));
            children.get(&(pid, tid)).cloned().unwrap_or_default()
        },
        |process_group_id, pid| {
            member_reads.borrow_mut().push(pid);
            let (pgrp, comm) = processes.get(&pid)?;
            (*pgrp == process_group_id).then(|| ProcGroupMember {
                pid,
                comm: (*comm).to_string(),
            })
        },
    )
    .unwrap();

    assert_eq!(
        members
            .into_iter()
            .map(|member| (member.pid, member.comm))
            .collect::<Vec<_>>(),
        vec![
            (200, "leader".to_string()),
            (201, "pipeline".to_string()),
            (210, "thread-child".to_string()),
            (221, "nested-agent".to_string()),
        ]
    );
    assert!(child_reads.borrow().contains(&(100, 101)));
    assert!(task_reads.borrow().contains(&220));
    assert!(!task_reads.borrow().contains(&9999));
    assert!(!member_reads.borrow().contains(&9999));
}

#[test]
fn foreground_members_degrade_to_the_direct_group_leader() {
    let members = foreground_process_group_members_with(
        100,
        200,
        |_| Vec::new(),
        |_, _| Vec::new(),
        |process_group_id, pid| {
            (pid == process_group_id).then(|| ProcGroupMember {
                pid,
                comm: "leader".to_string(),
            })
        },
    )
    .unwrap();

    assert_eq!(
        members,
        vec![ProcGroupMember {
            pid: 200,
            comm: "leader".to_string()
        }]
    );
}

#[test]
fn foreground_members_observe_new_children_without_a_snapshot_cache() {
    let children = RefCell::new(HashMap::from([((100, 100), vec![200])]));
    let discover = || {
        foreground_process_group_members_with(
            100,
            200,
            |pid| vec![pid],
            |pid, tid| {
                children
                    .borrow()
                    .get(&(pid, tid))
                    .cloned()
                    .unwrap_or_default()
            },
            |process_group_id, pid| {
                [200, 201]
                    .contains(&pid)
                    .then(|| ProcGroupMember {
                        pid,
                        comm: format!("member-{pid}"),
                    })
                    .filter(|_| process_group_id == 200)
            },
        )
        .unwrap()
        .into_iter()
        .map(|member| member.pid)
        .collect::<Vec<_>>()
    };

    assert_eq!(discover(), vec![200]);
    children.borrow_mut().insert((100, 100), vec![200, 201]);
    assert_eq!(discover(), vec![200, 201]);
}

#[test]
fn proc_stat_parsing_keeps_group_leader_inputs_live() {
    assert_eq!(
        process_pgrp_and_comm_from_stat("123 (name with ) paren) S 1 456 789 0 456"),
        Some((456, "name with ) paren".to_string()))
    );
}
