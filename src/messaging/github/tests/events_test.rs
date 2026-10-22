use super::*;

fn scope(room: u64, branch: &str) -> Scope {
    Scope {
        room,
        branches: [branch.into()].into(),
    }
}
fn pr(number: u64, branch: &str) -> PullRequest {
    PullRequest {
        number,
        branch: branch.into(),
        base: "main".into(),
        author: "alice".into(),
        merged_by: "merger".into(),
        mergeable_known: true,
        status: PrStatus {
            head: "abc".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn page() -> Page {
    Page {
        default_branch: "main".into(),
        ..Default::default()
    }
}

#[test]
fn github_watch_routes_owned_merges_and_batches_other_merges_per_room_after_restart() {
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    page.merged = (1..=3)
        .map(|n| {
            let mut p = pr(n, if n == 1 { "fix/ours" } else { "fix/theirs" });
            p.merged_at_ms = Some(now - 1000);
            p
        })
        .collect();
    let scopes = [scope(7, "fix/ours"), scope(8, "fix/other")];
    let mut mark = Watermark::default();
    let notices = changes("owner/repo", &mut mark, &page, &scopes, now);
    assert_eq!(
        notices.len(),
        3,
        "one owned merge + one informational batch per room"
    );
    let owned = notices
        .iter()
        .find(|n| n.actionable)
        .expect("owned merge notice");
    assert_eq!(owned.room, 7);
    assert!(owned.text.contains("#1 merged into main"));
    assert!(owned.text.contains("by merger (head fix/ours)"));
    assert!(owned.text.contains("Next: cleanup / verify / follow-up"));
    assert_eq!(notices.iter().filter(|n| !n.actionable).count(), 2);
    let mut recovered: Watermark =
        serde_json::from_str(&serde_json::to_string(&mark).unwrap()).unwrap();
    assert!(changes("owner/repo", &mut recovered, &page, &scopes, now + POLL_MS).is_empty());
}

#[test]
fn github_watch_reports_ready_ci_and_conflict_edges_without_pending_or_unknown_false_edges() {
    let now = LOOKBACK_MS * 3;
    let mut p = pr(4, "fix/ours");
    let mut page = page();
    page.open.push(p.clone());
    let scopes = [scope(7, "fix/ours")];
    let mut mark = Watermark::default();
    assert!(changes("o/r", &mut mark, &page, &scopes, now).is_empty());
    p.status.ready = true;
    p.status.ci = Some("success:run1".into());
    p.status.conflict = true;
    page.open = vec![p.clone()];
    let edges = changes("o/r", &mut mark, &page, &scopes, now + POLL_MS);
    assert_eq!(edges.len(), 3);
    assert!(edges.iter().any(|n| n.text.contains("ready for review")));
    assert!(edges
        .iter()
        .any(|n| n.text.contains("CI completed: success")));
    assert!(edges.iter().any(|n| n.text.contains("merge conflict")));
    p.status.ci = None;
    p.status.conflict = false;
    p.mergeable_known = false;
    page.open = vec![p.clone()];
    assert!(changes("o/r", &mut mark, &page, &scopes, now + 2 * POLL_MS).is_empty());
    p.status.ci = Some("failure:run2".into());
    p.status.conflict = true;
    p.mergeable_known = true;
    page.open = vec![p];
    let edges = changes("o/r", &mut mark, &page, &scopes, now + 3 * POLL_MS);
    assert_eq!(
        edges.len(),
        1,
        "unknown mergeability must not rearm an existing conflict"
    );
    assert!(edges[0].text.contains("CI completed: failure"));
}

#[test]
fn github_watch_uses_feature_branch_authors_but_never_the_default_branch_author() {
    let mut page = page();
    let mut p = pr(1, "another-branch");
    p.status.ready = true;
    page.open = vec![p];
    page.branch_authors = [
        ("fix/ours".into(), "alice".into()),
        ("main".into(), "alice".into()),
    ]
    .into();
    let edges = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[scope(7, "fix/ours"), scope(8, "main")],
        LOOKBACK_MS * 3,
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].room, 7);
}

#[test]
fn github_watch_keeps_merge_scan_watermark_behind_unread_pages_and_equal_time_merges() {
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    let mut a = pr(1, "fix/ours");
    a.merged_at_ms = Some(now - 1000);
    page.merged = vec![a.clone()];
    page.merged_cursor = Some("next".into());
    let mut mark = Watermark::default();
    let scopes = [scope(7, "fix/ours")];
    assert_eq!(changes("o/r", &mut mark, &page, &scopes, now).len(), 1);
    assert!(mark.merged_since_ms < now - 1000);
    a.number = 2;
    page.merged = vec![a];
    page.merged_cursor = None;
    assert_eq!(
        changes("o/r", &mut mark, &page, &scopes, now + POLL_MS).len(),
        1
    );
    assert_eq!(
        mark.merged_since_ms, now,
        "do not skip inserts made while paging"
    );
}

#[test]
fn github_watch_accepts_only_github_repo_coordinates_and_canonical_remotes() {
    for value in [
        "owner/repo",
        "https://github.com/owner/repo.git",
        "git@github.com:owner/repo.git",
        "ssh://git@github.com/owner/repo.git",
    ] {
        assert_eq!(repository(value).as_deref(), Some("owner/repo"), "{value}");
    }
    for value in [
        "https://github.com.evil/owner/repo",
        "git@gitlab.com:owner/repo",
        "https://token@github.com/owner/repo",
        "../repo",
        "owner/repo/extra",
        "owner/repo?x",
        "owner/..",
    ] {
        assert!(repository(value).is_none(), "{value}");
    }
}

#[test]
fn github_watch_pending_rerun_rearms_completion_and_ignores_merges_to_other_bases() {
    let mut page = page();
    let mut p = pr(1, "fix/ours");
    p.status.ci = Some("success:same".into());
    page.open = vec![p.clone()];
    let scopes = [scope(7, "fix/ours")];
    let mut mark = Watermark::default();
    assert_eq!(
        changes("o/r", &mut mark, &page, &scopes, LOOKBACK_MS * 3).len(),
        1
    );
    p.status.ci = Some("pending".into());
    page.open = vec![p.clone()];
    assert!(changes("o/r", &mut mark, &page, &scopes, LOOKBACK_MS * 3 + POLL_MS).is_empty());
    p.status.ci = Some("success:same".into());
    page.open = vec![p];
    let mut other = pr(2, "fix/ours");
    other.base = "release".into();
    other.merged_at_ms = Some(LOOKBACK_MS * 3);
    page.merged = vec![other];
    let notices = changes(
        "o/r",
        &mut mark,
        &page,
        &scopes,
        LOOKBACK_MS * 3 + 2 * POLL_MS,
    );
    assert_eq!(notices.len(), 1);
    assert!(notices[0].text.contains("CI completed: success"));
}
