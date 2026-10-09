use super::*;

#[test]
fn windows_foreground_process_snapshot_is_shared_within_ttl() {
    let mut cache = super::super::ProcessSnapshotCache { cached: None };
    let mut builds = 0;
    let mut first_build_completed_at = None;

    let first = cache.snapshot(Duration::from_secs(60), || {
        builds += 1;
        let entries = vec![test_entry(10, 1, "powershell.exe", &["powershell.exe"])];
        first_build_completed_at = Some(Instant::now());
        entries
    });
    assert!(cache.cached.as_ref().unwrap().built_at >= first_build_completed_at.unwrap());
    let second = cache.snapshot(Duration::from_secs(60), || {
        builds += 1;
        Vec::new()
    });
    let refreshed = cache.snapshot(Duration::ZERO, || {
        builds += 1;
        vec![test_entry(20, 1, "pwsh.exe", &["pwsh.exe"])]
    });

    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&second, &refreshed));
    assert_eq!(builds, 2);
    assert_eq!(refreshed.entries[0].pid, 20);
}

#[test]
fn windows_foreground_selection_cache_reuses_live_agent_and_invalidates_changes() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
    ]);
    let job = super::super::foreground_job_from_entry(snapshot.entry(20).unwrap());
    let mut cache = super::super::ForegroundSelectionCache::default();
    cache.remember_for_test(10, &snapshot, &job);

    assert_eq!(cache.get(10, &snapshot), Some(job.clone()));

    cache.entries.get_mut(&10).unwrap().selected_identity = super::super::ProcessIdentity::Stub {
        running: false,
        creation_time: None,
    };
    assert_eq!(cache.get(10, &snapshot), None);

    cache.remember_for_test(10, &snapshot, &job);
    let overlap = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "claude.exe", &["claude.exe"]),
    ]);
    assert_eq!(cache.get(10, &overlap), None);

    cache.remember_for_test(10, &snapshot, &job);
    let changed = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "git.exe", &["git.exe"]),
    ]);
    assert_eq!(cache.get(10, &changed), None);

    cache.remember_for_test(10, &snapshot, &job);
    cache.entries.get_mut(&10).unwrap().verified_at = Instant::now()
        .checked_sub(super::super::FOREGROUND_SELECTION_RECHECK + Duration::from_secs(1))
        .unwrap();
    assert_eq!(cache.get(10, &snapshot), None);
}

#[test]
fn windows_foreground_selection_cache_rejects_reused_descendant_pid() {
    let original = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "node.exe", &["node.exe", "worker.js"]),
    ]);
    let job = super::super::foreground_job_from_entry(original.entry(20).unwrap());
    let mut cache = super::super::ForegroundSelectionCache::default();
    cache.remember_for_test(10, &original, &job);

    let cached = cache.entries.get_mut(&10).unwrap();
    let reused_index = cached
        .descendants
        .iter()
        .position(|entry| entry.pid == 30)
        .unwrap();
    cached.descendant_identities[reused_index] = super::super::ProcessIdentity::Stub {
        running: false,
        creation_time: None,
    };
    let replacement = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
        test_entry(30, 10, "node.exe", &["node.exe", "codex.js"]),
    ]);

    assert_eq!(cache.get(10, &replacement), None);
}

#[test]
fn windows_foreground_selection_cache_rejects_identity_change_before_insertion() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry_with_creation_time(10, 1, "powershell.exe", &["powershell.exe"], Some(1)),
        test_entry_with_creation_time(20, 10, "codex.exe", &["codex.exe"], Some(2)),
        test_entry_with_creation_time(30, 10, "node.exe", &["node.exe", "worker.js"], Some(3)),
    ]);
    let job = super::super::foreground_job_from_entry(snapshot.entry(20).unwrap());
    let descendants = snapshot.descendant_signatures(10);
    let descendant_identities = vec![
        super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(2),
        },
        super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(4),
        },
    ];
    let mut cache = super::super::ForegroundSelectionCache::default();

    let cached = super::super::CachedForegroundSelection::from_snapshot_with_identities(
        10,
        &snapshot,
        &job,
        descendants,
        descendant_identities,
        super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(1),
        },
        super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(2),
        },
    );
    cache.remember(10, cached);

    assert!(cache.entries.is_empty());
}

#[test]
fn windows_foreground_selection_cache_accepts_limited_information_identity() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry_without_cmdline(10, 1, "powershell.exe", 1),
        test_entry_without_cmdline(20, 10, "codex.exe", 2),
    ]);
    let job = super::super::foreground_job_from_entry(snapshot.entry(20).unwrap());
    let cached = super::super::CachedForegroundSelection::from_snapshot_with_identities(
        10,
        &snapshot,
        &job,
        snapshot.descendant_signatures(10),
        vec![super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(2),
        }],
        super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(1),
        },
        super::super::ProcessIdentity::Stub {
            running: true,
            creation_time: Some(2),
        },
    );

    assert!(cached.is_some());
    assert_eq!(job.processes[0].argv0.as_deref(), Some("codex.exe"));
    assert!(job.processes[0].cmdline.is_none());
}

#[test]
fn windows_foreground_selection_cache_prunes_unused_entries_on_insertion() {
    let first_snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "codex.exe", &["codex.exe"]),
    ]);
    let first_job = super::super::foreground_job_from_entry(first_snapshot.entry(20).unwrap());
    let mut cache = super::super::ForegroundSelectionCache::default();
    cache.remember_for_test(10, &first_snapshot, &first_job);
    cache.entries.get_mut(&10).unwrap().last_used = Instant::now()
        .checked_sub(super::super::FOREGROUND_SELECTION_CACHE_RETENTION + Duration::from_secs(1))
        .unwrap();

    let second_snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry(11, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(21, 11, "claude.exe", &["claude.exe"]),
    ]);
    let second_job = super::super::foreground_job_from_entry(second_snapshot.entry(21).unwrap());
    cache.remember_for_test(11, &second_snapshot, &second_job);

    assert!(!cache.entries.contains_key(&10));
    assert!(cache.entries.contains_key(&11));
}

#[test]
fn windows_foreground_selection_cache_does_not_retain_shell_result() {
    let snapshot = super::super::ProcessSnapshot::new(vec![test_entry(
        10,
        1,
        "powershell.exe",
        &["powershell.exe"],
    )]);
    let shell = super::super::foreground_job_from_entry(snapshot.entry(10).unwrap());
    let mut cache = super::super::ForegroundSelectionCache::default();

    cache.remember_for_test(10, &snapshot, &shell);

    assert!(cache.entries.is_empty());
}

#[test]
fn windows_foreground_selection_cache_retains_live_escaped_agent() {
    let snapshot = super::super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "bash.exe", &["bash.exe"]),
        test_entry(20, 99, "launcher.exe", &["codex.exe"]),
    ]);
    let escaped = super::super::foreground_job_from_entry(snapshot.entry(20).unwrap());
    let mut cache = super::super::ForegroundSelectionCache::default();

    cache.remember_for_test(10, &snapshot, &escaped);

    assert_eq!(cache.get(10, &snapshot), Some(escaped.clone()));
    cache.entries.get_mut(&10).unwrap().selected_identity = super::super::ProcessIdentity::Stub {
        running: false,
        creation_time: None,
    };
    assert_eq!(cache.get(10, &snapshot), None);
}
