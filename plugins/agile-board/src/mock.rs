use crate::{
    error::Result,
    model::{BoardState, Column, NewTicketInput, UpdateTicketInput},
};

pub fn seed_mock_state(state: &mut BoardState, now_ms: i64) -> Result<()> {
    let hour = 3_600_000_i64;
    let day = 86_400_000_i64;

    // Register 3 neutral section labels
    state.add_room(1, "backend-api".into(), now_ms - 5 * day)?;
    state.add_room(2, "web-client".into(), now_ms - 5 * day)?;
    state.add_room(3, "storage-engine".into(), now_ms - 4 * day)?;

    // Section 1: backend-api
    // T-1: Done (completed 2 days ago -> appears in week & session, not day)
    let t1 = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Guard concurrent token refresh and retain failed session snapshots".into(),
            description: Some(
                "Fixes race reported in [Feishu buglist #4821](https://example.feishu.cn/wiki/wikcnBugList4821) per [Auth Spec v2](https://example.com/docs/auth-spec-v2).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/500".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Reproduced token overwrite when interrupted mid-refresh.".into()),
        },
        now_ms - 3 * day,
    )?;
    state.move_ticket(
        &t1.id,
        Column::Implementation,
        Some("Added atomic snapshot guard in token refresh path.".into()),
        now_ms - 3 * day + 2 * hour,
    )?;
    state.move_ticket(
        &t1.id,
        Column::Testing,
        Some("Added unit and integration regression tests.".into()),
        now_ms - 2 * day - 4 * hour,
    )?;
    state.move_ticket(
        &t1.id,
        Column::Review,
        Some("Code review approved at 9a8029a7.".into()),
        now_ms - 2 * day - 2 * hour,
    )?;
    state.move_ticket(
        &t1.id,
        Column::Done,
        Some("Merged #500 to main.".into()),
        now_ms - 2 * day,
    )?;

    // T-2: Investigation-only -> Done (today -> appears in day, week, session)
    let t2 = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Audit intermittent stale socket cleanup after forced SIGKILL".into(),
            description: Some(
                "Root-cause check for [Feishu buglist #4902](https://example.feishu.cn/wiki/wikcnBugList4902); already covered by listener flock check in #492.".into(),
            ),
            column: Column::Investigation,
            mr_url: None,
            author_model: None,
            reviewer_model: None,
            investigation_only: true,
            note: Some("Tracing socket bind recovery in isolated directories.".into()),
        },
        now_ms - 5 * hour,
    )?;
    state.move_ticket(
        &t2.id,
        Column::Done,
        Some("Confirmed handled by listener flock check; no code change required.".into()),
        now_ms - 4 * hour,
    )?;

    // T-3: Blocked (today -> needs decision)
    let t3 = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Add upstream gateway health probe and fallback routing".into(),
            description: Some(
                "Blocked on [Gateway PRD](https://example.com/docs/gateway-prd): refuse startup when primary upstream is missing from `/v1/health` or fall back silently?".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/508".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Verified /v1/health discovery against local gateway.".into()),
        },
        now_ms - 4 * hour,
    )?;
    state.move_ticket(
        &t3.id,
        Column::Implementation,
        Some("Wired health probe and endpoint check.".into()),
        now_ms - 3 * hour,
    )?;
    state.move_ticket(
        &t3.id,
        Column::Blocked,
        Some("Waiting on product decision for missing upstream policy.".into()),
        now_ms - 90 * 60_000,
    )?;

    // T-4: Implementation #1 in backend-api (today)
    let t4 = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Build read-only localhost web kanban and CLI".into(),
            description: Some(
                "Implements the seven-column board per [Board Design Doc](https://example.com/docs/kanban-board-design) and [Feishu task #5010](https://example.feishu.cn/wiki/wikcnTask5010).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/512".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Studied Kanboard, Wekan, Planka, Vikunja, Focalboard, and Taiga layouts.".into()),
        },
        now_ms - 2 * hour,
    )?;
    state.move_ticket(
        &t4.id,
        Column::Implementation,
        Some("Implemented 7-column swimlanes, transition guard, and JSON persistence.".into()),
        now_ms - 35 * 60_000,
    )?;

    // T-5: Implementation #2 in backend-api (today -> stacked in implementation column)
    let t5_impl = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Enforce per-endpoint rate limiter token bucket on /v1/events".into(),
            description: Some(
                "Adds burst-safe limiter per [Rate Limit Spec](https://example.com/docs/rate-limit-spec) and [Feishu buglist #5012](https://example.feishu.cn/wiki/wikcnBugList5012).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/514".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Benchmarked lock-free atomic bucket refill.".into()),
        },
        now_ms - 110 * 60_000,
    )?;
    state.move_ticket(
        &t5_impl.id,
        Column::Implementation,
        Some("Wired middleware and 429 Retry-After headers.".into()),
        now_ms - 30 * 60_000,
    )?;

    // T-6: Implementation #3 in backend-api (today -> stacked in implementation column)
    let t6_impl = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Stream chunked NDJSON export for large audit queries".into(),
            description: Some(
                "Avoids buffering full response in memory for [Feishu buglist #5015](https://example.feishu.cn/wiki/wikcnBugList5015) per [Streaming API Doc](https://example.com/docs/streaming-api).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/515".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Profiled peak RSS during 100k-row export.".into()),
        },
        now_ms - 95 * 60_000,
    )?;
    state.move_ticket(
        &t6_impl.id,
        Column::Implementation,
        Some("Added cursor-based chunked writer with backpressure check.".into()),
        now_ms - 22 * 60_000,
    )?;

    // T-7: Implementation #4 in backend-api (today -> 4 stacked tickets in implementation column)
    let t7_impl = state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Validate webhook signature timestamp skew within 5 minutes".into(),
            description: Some(
                "Rejects replayed webhook payloads from [Security Audit #5019](https://example.feishu.cn/wiki/wikcnSecAudit5019) and [Webhook Auth Doc](https://example.com/docs/webhook-auth).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/516".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Checked HMAC-SHA256 header format.".into()),
        },
        now_ms - 80 * 60_000,
    )?;
    state.move_ticket(
        &t7_impl.id,
        Column::Implementation,
        Some("Implemented constant-time signature and timestamp window verification.".into()),
        now_ms - 15 * 60_000,
    )?;

    // T-8: Todo (deferred work, created 3 days ago -> week & session)
    state.add_ticket(
        NewTicketInput {
            room: "backend-api".into(),
            title: "Compact archived audit log snapshots older than 14 days".into(),
            description: Some(
                "Deferred cleanup tracked in [Retention Spec](https://example.com/docs/retention-spec) until current release completes.".into(),
            ),
            column: Column::Todo,
            mr_url: None,
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("On hold until current release wave finishes.".into()),
        },
        now_ms - 3 * day,
    )?;

    // Section 2: web-client
    // T-6: Review (today)
    let t6 = state.add_ticket(
        NewTicketInput {
            room: "web-client".into(),
            title: "Clear pending attachment chips after cancel shortcut".into(),
            description: Some(
                "Resolves leftover chip state from [Feishu buglist #4990](https://example.feishu.cn/wiki/wikcnBugList4990) in the composer input.".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/499".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Traced paste chip state machine across cancel shortcut.".into()),
        },
        now_ms - 6 * hour,
    )?;
    state.move_ticket(
        &t6.id,
        Column::Implementation,
        Some("Reset pending attachment chip buffer on cancel.".into()),
        now_ms - 5 * hour,
    )?;
    state.move_ticket(
        &t6.id,
        Column::Testing,
        Some("Verified keyboard sequences in UI integration suite.".into()),
        now_ms - 3 * hour,
    )?;
    state.move_ticket(
        &t6.id,
        Column::Review,
        Some("Awaiting final sign-off on head c5753fe4.".into()),
        now_ms - 50 * 60_000,
    )?;

    // Additional stacked Review tickets in web-client (3 stacked in review column)
    let t6_rev2 = state.add_ticket(
        NewTicketInput {
            room: "web-client".into(),
            title: "Preserve horizontal scroll offset across live board refreshes".into(),
            description: Some(
                "Keeps scroll position stable for [Feishu buglist #5021](https://example.feishu.cn/wiki/wikcnBugList5021) per [UX Spec](https://example.com/docs/ux-scroll).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/517".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Verified DOM diff behavior during 3s polling.".into()),
        },
        now_ms - 4 * hour,
    )?;
    state.move_ticket(&t6_rev2.id, Column::Implementation, None, now_ms - 3 * hour)?;
    state.move_ticket(&t6_rev2.id, Column::Testing, None, now_ms - 2 * hour)?;
    state.move_ticket(
        &t6_rev2.id,
        Column::Review,
        Some("Ready for reviewer check.".into()),
        now_ms - 45 * 60_000,
    )?;

    let t6_rev3 = state.add_ticket(
        NewTicketInput {
            room: "web-client".into(),
            title: "Add keyboard Escape handler to cancel inline confirmation".into(),
            description: Some(
                "Implements accessibility shortcut from [Feishu buglist #5024](https://example.feishu.cn/wiki/wikcnBugList5024) and [A11y Guide](https://example.com/docs/a11y-guide).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/518".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Checked keydown listener lifecycle.".into()),
        },
        now_ms - 3 * hour,
    )?;
    state.move_ticket(&t6_rev3.id, Column::Implementation, None, now_ms - 2 * hour)?;
    state.move_ticket(&t6_rev3.id, Column::Testing, None, now_ms - 90 * 60_000)?;
    state.move_ticket(
        &t6_rev3.id,
        Column::Review,
        Some("All keyboard tests green.".into()),
        now_ms - 38 * 60_000,
    )?;

    // T-7: Testing (today)
    let t7 = state.add_ticket(
        NewTicketInput {
            room: "web-client".into(),
            title: "Fix inline image scroll clipping near viewport bottom border".into(),
            description: Some(
                "Clamps thumbnail row placement per [UI Rendering Doc](https://example.com/docs/ui-rendering) and [Feishu buglist #5004](https://example.feishu.cn/wiki/wikcnBugList5004).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/510".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Measured 8-row thumbnail viewport bounds.".into()),
        },
        now_ms - 3 * hour,
    )?;
    state.move_ticket(
        &t7.id,
        Column::Implementation,
        Some("Clamped placement rows before bottom border render.".into()),
        now_ms - 2 * hour,
    )?;
    state.move_ticket(
        &t7.id,
        Column::Testing,
        Some("Running viewport screenshot assertions.".into()),
        now_ms - 25 * 60_000,
    )?;

    // T-8: Investigation (today)
    state.add_ticket(
        NewTicketInput {
            room: "web-client".into(),
            title: "Investigate dropdown menu focus trap when window resizes".into(),
            description: Some(
                "Reproducing resize event ordering from [Feishu buglist #5018](https://example.feishu.cn/wiki/wikcnBugList5018) while menu overlay is open.".into(),
            ),
            column: Column::Investigation,
            mr_url: None,
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Checking resize event ordering while selector overlay is open.".into()),
        },
        now_ms - 40 * 60_000,
    )?;

    // Section 3: storage-engine
    // T-9: Blocked then unblocked into Testing (today)
    let t9 = state.add_ticket(
        NewTicketInput {
            room: "storage-engine".into(),
            title: "Deduplicate outbound webhook retries within 30-minute window".into(),
            description: Some(
                "Implements idempotency window from [Webhook Spec](https://example.com/docs/webhook-spec) and [Feishu buglist #4975](https://example.feishu.cn/wiki/wikcnBugList4975).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/506".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Checked outbound correlation table in SQLite spool.".into()),
        },
        now_ms - 7 * hour,
    )?;
    state.move_ticket(
        &t9.id,
        Column::Blocked,
        Some("Needed confirmation on whether 30m window scopes per sender or per channel.".into()),
        now_ms - 6 * hour,
    )?;
    state.move_ticket(
        &t9.id,
        Column::Implementation,
        Some("Confirmed scope per sender and fold replies to matching requests.".into()),
        now_ms - 4 * hour,
    )?;
    state.move_ticket(
        &t9.id,
        Column::Testing,
        Some("Running synthetic NDJSON replay and SQLite rollback tests.".into()),
        now_ms - 20 * 60_000,
    )?;

    // T-10: Older session ticket (created 10 days ago -> appears only in 'session' view, not 'day' or 'week')
    let t10 = state.add_ticket(
        NewTicketInput {
            room: "storage-engine".into(),
            title: "Enforce CLI >= 1.0.97 stdin body streaming check before send".into(),
            description: Some(
                "Prevents literal hyphen send bug documented in [Feishu buglist #4712](https://example.feishu.cn/wiki/wikcnBugList4712).".into(),
            ),
            column: Column::Investigation,
            mr_url: Some("https://github.com/example/platform/pull/481".into()),
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: Some("Older CLI treated '--text -' as literal hyphen.".into()),
        },
        now_ms - 10 * day,
    )?;
    state.update_ticket(
        UpdateTicketInput {
            id: t10.id.clone(),
            title: None,
            description: None,
            mr_url: Some("https://github.com/example/platform/pull/481".into()),
            author_model: None,
            reviewer_model: None,
            note: Some("Verified version parser against 1.0.96 and 1.0.97.".into()),
        },
        now_ms - 10 * day + hour,
    )?;
    state.move_ticket(
        &t10.id,
        Column::Implementation,
        Some("Added pre-send version check.".into()),
        now_ms - 10 * day + 2 * hour,
    )?;
    state.move_ticket(
        &t10.id,
        Column::Testing,
        Some("All synthetic version tests green.".into()),
        now_ms - 10 * day + 3 * hour,
    )?;
    state.move_ticket(
        &t10.id,
        Column::Review,
        Some("Reviewed and approved.".into()),
        now_ms - 10 * day + 4 * hour,
    )?;
    state.move_ticket(
        &t10.id,
        Column::Done,
        Some("Merged #481.".into()),
        now_ms - 10 * day + 5 * hour,
    )?;

    let unreproduced = state.add_ticket(
        NewTicketInput {
            room: "web-client".into(),
            title: "Investigate blank filter results after restoring a tab".into(),
            description: Some("Reported after a browser session restore.".into()),
            column: Column::Investigation,
            mr_url: None,
            author_model: None,
            reviewer_model: None,
            investigation_only: false,
            note: None,
        },
        now_ms - 3 * hour,
    )?;
    state.move_ticket(
        &unreproduced.id,
        Column::WontWork,
        Some("Could not reproduce after restoring tabs with saved filters.".into()),
        now_ms - hour,
    )?;

    Ok(())
}
