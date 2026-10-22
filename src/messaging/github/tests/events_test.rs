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
fn github_watch_unmatched_head_never_falls_back_to_the_default_or_feature_branch_author() {
    let mut page = page();
    let mut p = pr(1, "another-branch");
    p.status.ready = true;
    p.merged_at_ms = Some(LOOKBACK_MS * 3);
    page.merged = vec![p];
    let notices = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[scope(7, "fix/ours"), scope(8, "main")],
        LOOKBACK_MS * 3,
    );
    assert_eq!(notices.len(), 2);
    assert!(notices.iter().all(|n| !n.actionable));
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
    assert!(changes("o/r", &mut mark, &page, &scopes, LOOKBACK_MS * 3).is_empty());
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

#[test]
fn github_watch_same_github_author_does_not_make_every_room_own_every_pr() {
    // Every branch in a single-account repository has the same head-commit author.
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    let mut merged = pr(2, "fix/b");
    merged.merged_at_ms = Some(now - 1000);
    page.merged = vec![merged];
    let notices = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[scope(7, "fix/a"), scope(8, "fix/b")],
        now,
    );
    let owned: Vec<_> = notices
        .iter()
        .filter(|n| n.actionable)
        .map(|n| n.room)
        .collect();
    assert_eq!(
        owned,
        [8],
        "room 7 got an owned notice for room 8's merge: {notices:?}"
    );
}

#[test]
fn github_watch_first_poll_baselines_already_open_prs_without_ready_or_ci_notices() {
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    page.open = (1..=5)
        .map(|n| {
            let mut p = pr(n, "fix/ours");
            p.status.ready = true;
            p.status.ci = Some(format!("success:run{n}"));
            p
        })
        .collect();
    let notices = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[scope(7, "fix/ours")],
        now,
    );
    assert!(
        notices.is_empty(),
        "first poll announced {} transitions that happened before the watch: {notices:?}",
        notices.len()
    );
}

#[test]
fn github_watch_merge_time_uses_the_local_clock_like_chat_headers() {
    let merged_at = 1_792_667_738_000; // 2026-10-22T11:15:38Z
    let mut page = page();
    let mut merged = pr(137, "fix/ours");
    merged.merged_at_ms = Some(merged_at);
    page.merged = vec![merged];
    let notices = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[scope(7, "fix/ours")],
        merged_at + 1000,
    );
    let local = crate::platform::local_datetime_at((merged_at / 1000) as i64).unwrap();
    let shown = format!("at {:02}:{:02} ", local.hour(), local.minute());
    assert!(
        notices[0].text.contains(&shown),
        "expected local {shown:?}: {}",
        notices[0].text
    );
}

#[test]
fn github_watch_only_matching_heads_are_actionable() {
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    let mut merged = pr(1, "unmatched");
    merged.merged_at_ms = Some(now - 1000);
    page.merged = vec![merged];
    let scopes = [scope(7, "fix/a"), scope(8, "fix/b")];
    let owners = |page: &Page| {
        changes("o/r", &mut Watermark::default(), page, &scopes, now)
            .into_iter()
            .filter(|n| n.actionable)
            .map(|n| n.room)
            .collect::<Vec<_>>()
    };
    assert!(
        owners(&page).is_empty(),
        "an unmatched head is always informational"
    );
    page.merged[0].branch = "fix/b".into();
    assert_eq!(owners(&page), [8]);
}

#[test]
fn github_watch_new_room_baselines_without_suppressing_existing_rooms_edges() {
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    page.open = vec![pr(1, "fix/ours")];
    let mut mark = Watermark::default();
    assert!(changes("o/r", &mut mark, &page, &[scope(7, "fix/ours")], now).is_empty());
    page.open[0].status.ready = true;
    page.open[0].status.ci = Some("success:run1".into());
    page.open[0].status.conflict = true;
    let scopes = [scope(7, "fix/ours"), scope(8, "fix/ours")];
    let edges = changes("o/r", &mut mark, &page, &scopes, now + POLL_MS);
    assert_eq!(edges.len(), 3);
    assert!(edges.iter().all(|n| n.room == 7));
    assert_eq!(mark.observed[&8][&1].ci.as_deref(), Some("success:run1"));
    let mut recovered: Watermark =
        serde_json::from_str(&serde_json::to_string(&mark).unwrap()).unwrap();
    assert!(changes("o/r", &mut recovered, &page, &scopes, now + 2 * POLL_MS).is_empty());
}

#[test]
fn github_watch_previous_local_day_includes_date() {
    let at = 1_792_667_738_000;
    let mut page = page();
    let mut merged = pr(137, "fix/ours");
    merged.merged_at_ms = Some(at);
    page.merged = vec![merged];
    let local = crate::platform::local_datetime_at((at / 1000) as i64).unwrap();
    let notices = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[scope(7, "fix/ours")],
        at + LOOKBACK_MS,
    );
    let shown = format!(
        "at {} {:02}:{:02} ",
        local.date(),
        local.hour(),
        local.minute()
    );
    assert!(notices[0].text.contains(&shown), "{}", notices[0].text);
}

#[test]
fn github_watch_confirmed_merge_removes_open_observation_and_old_merge_ids_stay_deduped() {
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    let mut p = pr(1, "fix/ours");
    page.open = vec![p.clone()];
    let scopes = [scope(7, "fix/ours")];
    let mut mark = Watermark::default();
    assert!(changes("o/r", &mut mark, &page, &scopes, now).is_empty());
    assert!(mark.observed[&7].contains_key(&1));
    page.open.clear();
    p.merged_at_ms = Some(now);
    page.merged = vec![p];
    assert_eq!(
        changes("o/r", &mut mark, &page, &scopes, now + POLL_MS).len(),
        1
    );
    assert!(!mark.observed[&7].contains_key(&1));
    page.merged.clear();
    changes("o/r", &mut mark, &page, &scopes, now + 10 * LOOKBACK_MS);
    let mut recovered: Watermark =
        serde_json::from_str(&serde_json::to_string(&mark).unwrap()).unwrap();
    let mut updated = pr(1, "fix/ours");
    updated.merged_at_ms = Some(now);
    page.merged = vec![updated];
    assert!(changes(
        "o/r",
        &mut recovered,
        &page,
        &scopes,
        now + 11 * LOOKBACK_MS
    )
    .is_empty());
}

#[test]
fn github_watch_merge_from_a_room_whose_worker_left_is_not_claimed_by_the_only_other_authored_room()
{
    // Room 7's worker (branch fix/a) was deleted after opening its PR; the room
    // still covers the repository. Room 8 has one pushed branch by the same author.
    let now = LOOKBACK_MS * 3;
    let mut page = page();
    let mut merged = pr(2, "fix/a");
    merged.merged_at_ms = Some(now - 1000);
    page.merged = vec![merged];
    let notices = changes(
        "o/r",
        &mut Watermark::default(),
        &page,
        &[
            Scope {
                room: 7,
                branches: BTreeSet::new(),
            },
            scope(8, "fix/b"),
        ],
        now,
    );
    assert!(
        notices.iter().all(|n| !n.actionable),
        "room 8 told to clean up room 7's merge: {notices:?}"
    );
}
