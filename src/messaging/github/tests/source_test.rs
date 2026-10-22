use super::*;

fn request() -> Query {
    Query {
        repo: "owner/repo".into(),
        watermark: Watermark::default(),
        now_ms: 2 * LOOKBACK_MS,
    }
}
fn response() -> Value {
    json!({"data":{"repository":{"defaultBranchRef":{"name":"main"},
    "open":{"nodes":[],"pageInfo":{"hasNextPage":false}},
    "merged":{"nodes":[],"pageInfo":{"hasNextPage":false}}},
    "rateLimit":{"remaining":90,"resetAt":"2026-10-22T12:00:00Z"}}})
}
#[test]
fn github_watch_query_has_one_repository_request_and_no_author_ownership_lookup() {
    let mut q = request();
    q.watermark.open_cursor = Some("open-page".into());
    q.watermark.merged_cursor = Some("merge-page".into());
    let text = query(&q);
    assert_eq!(text.matches("repository(owner:").count(), 1);
    assert!(text.contains("after: \"open-page\""));
    assert!(text.contains("after: \"merge-page\""));
    assert!(!text.contains("ref(qualifiedName:"));
    assert!(!text.contains("author {"));
    assert!(text.contains("rateLimit"));
}
#[test]
fn github_watch_parser_honors_rate_reset_and_rejects_partial_data() {
    let q = request();
    let mut v = response();
    let p = parse(&q, &v).unwrap();
    assert!(p.retry_at_ms.is_some());
    v["errors"] = json!([{"message":"rate limited"}]);
    assert!(parse(&q, &v).unwrap_err().retry_at_ms.is_some());
    v = response();
    v["data"]["repository"]["open"]["pageInfo"] = json!({"hasNextPage":true});
    assert!(parse(&q, &v).is_err());
}
#[test]
fn github_watch_ci_requires_all_checks_finished_and_dedupes_reruns_by_identity() {
    let mut v = json!({"headRefOid":"head", "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE","contexts":{"pageInfo":{"hasNextPage":false},"nodes":[{"id":"1","status":"COMPLETED","conclusion":"FAILURE"},{"id":"2","status":"IN_PROGRESS"}]}}}}]}});
    assert_eq!(ci(&v).as_deref(), Some("pending"));
    v["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"][1]["status"] =
        json!("COMPLETED");
    let first = ci(&v).unwrap();
    assert!(first.starts_with("failure:"));
    v["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"][1]["id"] =
        json!("3");
    assert_ne!(ci(&v).unwrap(), first);
    v["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["pageInfo"]
        ["hasNextPage"] = json!(true);
    assert!(ci(&v).is_none());
}

#[test]
fn github_watch_ci_uses_full_counts_for_more_than_one_page_of_checks() {
    let mut v = json!({"headRefOid":"head", "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE","contexts":{"totalCount":101,"pageInfo":{"hasNextPage":true},"nodes":[{"id":"1","status":"COMPLETED"}],"checkRunCountsByState":[{"state":"SUCCESS","count":100},{"state":"IN_PROGRESS","count":1}],"statusContextCountsByState":[]}}}}]}});
    assert_eq!(ci(&v).as_deref(), Some("pending"));
    v["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["checkRunCountsByState"]
        [1]["state"] = json!("FAILURE");
    assert!(ci(&v).unwrap().starts_with("failure:"));
}

#[test]
fn github_watch_parser_reads_real_pr_shapes_and_pages_back_to_initial_scan_boundary() {
    let mut q = request();
    q.watermark.scan_started_ms = 172_800_000;
    q.now_ms = 259_200_000;
    let mut v = response();
    v["data"]["repository"]["b0"] = json!({"target":{"author":{"user":{"login":"alice"}}}});
    v["data"]["repository"]["open"] = json!({"pageInfo":{"hasNextPage":true,"endCursor":"open-next"},"nodes":[{"number":1,"headRefName":"fix/a","baseRefName":"main","author":{"login":"alice"},"isDraft":false,"mergeable":"CONFLICTING","headRefOid":"head"}]});
    v["data"]["repository"]["merged"] = json!({"pageInfo":{"hasNextPage":true,"endCursor":"merge-next"},"nodes":[{"number":137,"headRefName":"fix/a","baseRefName":"main","author":{"login":"alice"},"state":"MERGED","mergedAt":"1970-01-02T00:00:01Z","updatedAt":"1970-01-02T00:00:01Z","mergedBy":{"login":"merger"}}]});
    let page = parse(&q, &v).unwrap();
    assert_eq!(page.open_cursor.as_deref(), Some("open-next"));
    assert_eq!(
        page.merged_cursor.as_deref(),
        Some("merge-next"),
        "use scan start, not wall clock, across pagination"
    );
    assert!(page.open[0].status.ready);
    assert!(page.open[0].status.conflict);
    assert!(page.open[0].mergeable_known);
    assert_eq!(page.merged[0].merged_at_ms, Some(86_401_000));
    assert_eq!(page.merged[0].merged_by, "merger");
    v["data"]["repository"]["merged"]["nodes"][0]["updatedAt"] = json!("1970-01-01T00:00:00Z");
    assert!(parse(&q, &v).unwrap().merged_cursor.is_none());
}

fn owned_open(number: u64, branch: &str) -> PullRequest {
    PullRequest {
        number,
        branch: branch.into(),
        base: "main".into(),
        status: PrStatus {
            head: format!("head-{number}"),
            ready: true,
            ci: Some(format!("success:head-{number}:1")),
            conflict: false,
        },
        mergeable_known: true,
        ..Default::default()
    }
}
fn scope(room: u64, branch: &str) -> super::super::Scope {
    super::super::Scope {
        room,
        branches: [branch.to_string()].into(),
    }
}

#[test]
fn github_watch_owned_pr_that_moves_pages_mid_scan_is_not_reannounced() {
    let scopes = [scope(7, "fix/a")];
    let mut mark = Watermark::default();
    let page = |open: Vec<PullRequest>, cursor: Option<&str>| Page {
        default_branch: "main".into(),
        open,
        open_cursor: cursor.map(Into::into),
        ..Default::default()
    };
    let x = owned_open(1, "fix/a");
    let first = super::super::changes("o/r", &mut mark, &page(vec![x.clone()], None), &scopes, 1);
    assert!(first.is_empty(), "first observation must baseline quietly");
    // Next scan spans two pages; #1 was on page two when page one was read,
    // then got updated and moved to page one before page two was read.
    super::super::changes(
        "o/r",
        &mut mark,
        &page(vec![owned_open(2, "x")], Some("c1")),
        &scopes,
        2,
    );
    super::super::changes(
        "o/r",
        &mut mark,
        &page(vec![owned_open(3, "y")], None),
        &scopes,
        3,
    );
    assert!(
        mark.observed[&7].contains_key(&1),
        "page movement discarded observation"
    );
    let again = super::super::changes("o/r", &mut mark, &page(vec![x.clone()], None), &scopes, 4);
    assert!(
        again.is_empty(),
        "unchanged PR state re-announced: {again:?}"
    );
    let mut changed = x;
    changed.status.ci = Some("failure:rerun".into());
    let next = super::super::changes("o/r", &mut mark, &page(vec![changed], None), &scopes, 5);
    assert_eq!(
        next.len(),
        1,
        "real transition after page movement was lost"
    );
    assert!(next[0].text.contains("CI completed: failure"));
}

#[test]
fn github_watch_merge_that_moves_pages_mid_scan_is_still_reported() {
    let now = 10 * LOOKBACK_MS;
    let scopes = [scope(7, "fix/a")];
    let mut mark = Watermark::default();
    let merged = |number: u64, at: u64| PullRequest {
        number,
        branch: format!("b{number}"),
        base: "main".into(),
        merged_at_ms: Some(at),
        ..Default::default()
    };
    let page = |merged: Vec<PullRequest>, cursor: Option<&str>| Page {
        default_branch: "main".into(),
        merged,
        merged_cursor: cursor.map(Into::into),
        ..Default::default()
    };
    super::super::changes(
        "o/r",
        &mut mark,
        &page(vec![merged(10, now - 1000)], Some("m1")),
        &scopes,
        now,
    );
    // #11 (merged 5h ago) was on page two, then got a comment and moved to page one.
    super::super::changes("o/r", &mut mark, &page(vec![], None), &scopes, now + 90_000);
    let later = super::super::changes(
        "o/r",
        &mut mark,
        &page(vec![merged(11, now - 5 * 3_600_000)], None),
        &scopes,
        now + 180_000,
    );
    assert!(
        later.iter().any(|n| n.text.contains("#11")),
        "default-branch merge #11 never reported: {later:?}"
    );
}

#[test]
fn github_watch_pr_on_one_rooms_branch_is_not_owned_by_another_room_with_same_author() {
    let page = Page {
        default_branch: "main".into(),
        open: vec![owned_open(5, "fix/a")],
        ..Default::default()
    };
    let scopes = [scope(7, "fix/a"), scope(8, "fix/b")];
    let mut mark = Watermark::default();
    let mut baseline = page.clone();
    baseline.open[0].status.ready = false;
    baseline.open[0].status.ci = None;
    super::super::changes("o/r", &mut mark, &baseline, &scopes, 1);
    let notices = super::super::changes("o/r", &mut mark, &page, &scopes, 2);
    assert_eq!(
        notices.len(),
        2,
        "ready and CI transitions must reach the owner"
    );
    assert!(
        notices.iter().all(|n| n.room == 7),
        "room 8 (branch fix/b) got actionable notices for room 7's PR: {notices:?}"
    );
}

fn git_repo_with_remote(tag: &str, name: &str, url: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bus-gh-discovery-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q"]);
    git(&["remote", "add", name, url]);
    dir
}

#[test]
fn github_watch_discovery_ignores_relative_path_remote() {
    let dir = git_repo_with_remote("relative", "mirror", "vendor/bus");
    let found = super::super::discovery::discover(&[super::super::Interest {
        room: 1,
        agents: vec![(dir.clone(), None)],
        ..Default::default()
    }]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        found.is_empty(),
        "local relative remote treated as github.com repo: {:?}",
        found.keys()
    );
}

#[test]
fn github_watch_discovery_accepts_github_remote_with_trailing_slash() {
    let dir = git_repo_with_remote("slash", "origin", "https://github.com/owner/repo/");
    let found = super::super::discovery::discover(&[super::super::Interest {
        room: 1,
        agents: vec![(dir.clone(), None)],
        ..Default::default()
    }]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        found.contains_key("owner/repo"),
        "github.com remote missed: {:?}",
        found.keys()
    );
}

#[test]
fn github_watch_fetches_closed_prs_without_reporting_them_as_merges() {
    let q = request();
    assert!(
        query(&q).contains("states: [MERGED, CLOSED]"),
        "closed PRs are never fetched"
    );
    let mut v = response();
    v["data"]["repository"]["merged"]["nodes"] = json!([{
        "number": 200, "headRefName": "fix/closed", "baseRefName": "main",
        "state": "CLOSED", "mergedAt": null, "updatedAt": "2026-10-22T12:00:00Z"
    }]);
    let page = parse(&q, &v).unwrap();
    assert!(page.merged.is_empty(), "a closed PR became a merge event");
    assert_eq!(page.closed.len(), 1);
    assert_eq!(page.closed[0].branch, "fix/closed");
}
