use super::*;

fn request() -> Query {
    Query {
        repo: "owner/repo".into(),
        branches: ["fix/\"branch".into()].into(),
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
fn github_watch_query_has_one_repository_request_and_both_connections_with_escaped_branches() {
    let mut q = request();
    q.watermark.open_cursor = Some("open-page".into());
    q.watermark.merged_cursor = Some("merge-page".into());
    let text = query(&q);
    assert_eq!(text.matches("repository(owner:").count(), 1);
    assert!(text.contains("after: \"open-page\""));
    assert!(text.contains("after: \"merge-page\""));
    assert!(text.contains("refs/heads/fix/\\\"branch"));
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
    v["data"]["repository"]["merged"] = json!({"pageInfo":{"hasNextPage":true,"endCursor":"merge-next"},"nodes":[{"number":137,"headRefName":"fix/a","baseRefName":"main","author":{"login":"alice"},"mergedAt":"1970-01-02T00:00:01Z","updatedAt":"1970-01-02T00:00:01Z","mergedBy":{"login":"merger"}}]});
    let page = parse(&q, &v).unwrap();
    assert_eq!(page.open_cursor.as_deref(), Some("open-next"));
    assert_eq!(
        page.merged_cursor.as_deref(),
        Some("merge-next"),
        "use scan start, not wall clock, across pagination"
    );
    assert_eq!(
        page.branch_authors.values().next().map(String::as_str),
        Some("alice")
    );
    assert!(page.open[0].status.ready);
    assert!(page.open[0].status.conflict);
    assert!(page.open[0].mergeable_known);
    assert_eq!(page.merged[0].merged_at_ms, Some(86_401_000));
    assert_eq!(page.merged[0].merged_by, "merger");
    v["data"]["repository"]["merged"]["nodes"][0]["updatedAt"] = json!("1970-01-01T00:00:00Z");
    assert!(parse(&q, &v).unwrap().merged_cursor.is_none());
}
