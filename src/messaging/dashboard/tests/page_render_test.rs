use super::response_for;
use std::path::Path;

#[test]
fn page_keeps_hourly_and_failure_sections_and_drops_recipient_grouping() {
    let page =
        String::from_utf8(response_for(Path::new("/nonexistent-dashboard-page"), "GET", "/").body)
            .unwrap();
    assert!(page.contains("aria-label=\"Provider delivery\""));
    assert!(page.contains("aria-label=\"Room delivery\""));
    assert!(page.contains("id=\"graph\""));
    assert!(!page.contains("aria-label=\"Hourly delivery\""));
    assert!(!page.contains("id=\"hours\""));
    assert!(page.contains("id=\"failures\""));
    assert!(page.contains("id=\"failure-section\""));
    assert!(!page.contains("id=\"reply-section\""));
    assert!(!page.contains("id=\"reply-caption\""));
    assert!(!page.contains("Reply capture"));
    assert!(!page.contains("reply_capture"));
    assert!(!page.contains("explicit bus send --reply-to"));
    assert!(!page.contains("not a delivery failure"));
    assert!(!page.contains("messages that expect a reply"));
    assert!(page.contains("id=\"bus-note\""));
    assert!(!page.contains("id=\"delay-section\""));
    assert!(page.contains("p50"));
    assert!(page.contains("p90"));
    assert!(page.contains("p99"));
    assert!(page.contains("send accepted to agent pickup"));
    assert!(!page.contains("/latency"));
    assert!(!page.contains("not measured yet"));
    assert!(page.contains("Delivery success"));
    assert!(page.contains("Average latency"));
    assert!(page.contains("no samples"));
    assert!(page.contains("accepted_to_picked"));
    assert!(page.contains("duration(e.mean)"));
    assert!(!page.contains("end_to_end"));
    assert!(!page.contains("reply_delivered_at"));
    assert!(!page.contains("reply_captured_at"));
    assert!(page.contains("e.n?`n=${e.n}`:'no samples'"));
    assert!(!page.contains("mean??0"));
    assert!(!page.contains("p50??0"));
    assert!(page.contains("selectGroup"));
    assert_eq!(
        page.matches("<th>p50</th><th>p90</th><th>p99</th><th>n</th>")
            .count(),
        2
    );
    assert!(page.contains("How these are counted"));
    assert!(!page.contains("id=\"group-mode\""));
    assert!(!page.contains(">Recipients<"));
    assert!(!page.contains("value=\"recipients\""));
    assert!(!page.contains("toFixed"));
    assert!(page.contains(".caption"));
    assert!(page.contains(
        "<th>Delivered</th><th>Attempted</th><th>Pending</th><th>Delivery %</th><th>Bus failures</th><th>External</th><th>p50</th><th>p90</th><th>p99</th><th>n</th>"
    ));
    assert!(!page.contains(">Bus reliability</th><th>Delivered"));
    assert!(page.contains("c.delivered"));
    assert!(page.contains("delivery_share"));
    assert!(page.contains("oldest_open_ms"));
    assert!(!page.contains("c.open+c.reply_pending"));
    assert!(!page.contains("c.replied),node('td',c.attempts)"));
    assert!(!page.contains(">Top causes<"));
    assert!(!page.contains("Delivered / attempts"));
    assert!(!page.contains("causeButtons"));
    let providers = page.find("aria-label=\"Provider delivery\"").unwrap();
    let bus = page.find("id=\"bus-note\"").unwrap();
    let failure = page.find("id=\"failure-section\"").unwrap();
    let note = &page[bus..failure];
    assert!(note.contains("how many succeeded"), "{note}");
    assert!(!note.contains("how many failed"), "{note}");
    assert!(!note.contains("reply capture"), "{note}");
    assert!(!note.contains("explicit repl"), "{note}");
    assert!(page.contains("series.rates.bus_reliability.caption"));
    assert!(!page.contains("series.rates.reply_capture"));
    assert!(!page.contains("how many failed"));
    assert!(providers < bus && bus < failure);
    assert!(page.contains("drill.mode==='providers'"));
    assert!(page.contains("drill.mode==='rooms'"));
    assert!(!page.contains("drill.mode==='recipients'"));
    assert!(!page.contains("metrics.recipients"));
    assert!(!page.contains("metrics?.recipients"));
}

fn page() -> String {
    String::from_utf8(response_for(Path::new("/nonexistent-dashboard-page"), "GET", "/").body)
        .unwrap()
}

#[test]
fn page_combines_pending_with_failed_and_drops_the_hourly_table() {
    let page = page();
    assert!(page.contains("combined.id='pending-failed'"));
    assert!(page.contains("Pending and failed"));
    assert!(page.contains("reasonList(c.delivery_bus_reasons)"));
    assert!(page.contains("reasonList(c.delivery_external_reasons)"));
    assert!(page.contains("No pending message"));
    assert!(page.contains("node('li','None')"));
    assert!(page.contains("$('cards').append(delivery,combined)"));
    assert!(!page.contains("node('div',undefined,'card'),failed"));
    assert!(!page.contains("aria-label=\"Hourly delivery\""));
    assert!(!page.contains("id=\"hours\""));
    assert!(!page.contains("Counts & reasons"));
    assert!(!page.contains("hourlyRow"));
    assert!(page.contains("role:'button'"));
    assert!(page.contains("g.addEventListener('click',show)"));
    assert!(page.contains("renderGraph(series)"));
    assert!(page.contains("function currentFilter()"));
}

/// Same rounding as the page `duration` function. Half away from zero.
fn format_latency(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let tenths = (ms + 50) / 100;
    if tenths < 600 {
        return format!("{}.{} s", tenths / 10, tenths % 10);
    }
    let total = (ms + 500) / 1000;
    format!("{} m {} s", total / 60, total % 60)
}

#[test]
fn latency_duration_uses_human_units_at_each_boundary() {
    let page = page();
    let duration = "function duration(ms){if(ms==null)return '—';if(ms<1000)return `${ms} ms`;const tenths=Math.round(ms/100);if(tenths<600){const whole=Math.trunc(tenths/10),frac=Math.abs(tenths%10);return `${whole}.${frac} s`}const total=Math.round(ms/1000),minutes=Math.floor(total/60),seconds=total%60;return `${minutes} m ${seconds} s`}";
    assert!(
        page.contains(duration),
        "page duration function drifted from the tested rounding"
    );
    assert_eq!(format_latency(812), "812 ms");
    assert_eq!(format_latency(999), "999 ms");
    assert_eq!(format_latency(1000), "1.0 s");
    assert_eq!(format_latency(59_900), "59.9 s");
    assert_eq!(format_latency(60_000), "1 m 0 s");
    assert_eq!(format_latency(81_220), "1 m 21 s");
    assert!(page.contains("duration(e.p50)"));
    assert!(page.contains("duration(e.p90)"));
    assert!(page.contains("duration(e.p99)"));
    assert!(page.contains("duration(e.mean)"));
    assert!(!page.contains("`${value} ms`"));
    assert!(!page.contains("`${e.mean} ms`"));
    assert!(!page.contains("toFixed"));
}

#[test]
fn latency_series_and_page_do_not_reference_reply_timestamps() {
    let page = include_str!("../page.html");
    let latency = include_str!("../latency.rs");
    let metrics = include_str!("../metrics.rs");
    for (name, source) in [("page", page), ("latency", latency), ("metrics", metrics)] {
        assert!(!source.contains("reply_delivered_at"), "{name}");
        assert!(!source.contains("reply_captured_at"), "{name}");
        assert!(!source.contains("end_to_end"), "{name}");
    }
    for phase in ["reply", "reply_delivery", "reply_capture"] {
        assert!(
            !latency.contains(&format!("\"{phase}\"")),
            "latency phase {phase}"
        );
    }
    assert!(latency.contains("accepted_to_picked"));
    assert!(page.contains("accepted_to_picked"));
    assert!(metrics.contains("accepted_to_picked"));
}

#[test]
fn page_splits_delivery_launches_and_failures_across_three_tabs() {
    let page = page();
    assert!(!page.contains("id=\"view-mode\""));
    assert!(!page.contains("<select"));
    assert!(page.contains("role=\"tablist\""));
    assert!(page.contains("aria-label=\"Dashboard\""));
    assert!(page.contains(
        ".tabs button[aria-selected=true]{color:var(--accent);background:var(--selection);border-bottom-color:var(--accent)}"
    ));
    let messaging_tab = page.find("id=\"tab-messaging\"").unwrap();
    let agent_tab = page.find("id=\"tab-agent\"").unwrap();
    let errors_tab = page.find("id=\"tab-errors\"").unwrap();
    assert!(messaging_tab < agent_tab && agent_tab < errors_tab);
    assert!(page.contains("tabindex=\"0\" aria-selected=\"true\">Messaging</button>"));
    assert!(page.contains("tabindex=\"-1\" aria-selected=\"false\">Agent</button>"));
    assert!(page.contains("tabindex=\"-1\" aria-selected=\"false\">Errors</button>"));
    assert!(page.contains("ArrowRight:1"));
    assert!(page.contains("ArrowLeft:-1"));
    assert!(page.contains("showTab(next,true)"));

    let messaging = page.find("id=\"messaging-view\"").unwrap();
    let agent = page.find("id=\"agent-view\"").unwrap();
    let errors = page.find("id=\"errors-view\"").unwrap();
    let script = page.find("<script>").unwrap();
    assert!(page.find("id=\"dashboard-tabs\"").unwrap() < messaging);
    assert!(messaging < agent && agent < errors && errors < script);
    assert!(
        page.contains("id=\"messaging-view\" role=\"tabpanel\" aria-labelledby=\"tab-messaging\">")
    );
    assert!(
        page.contains("id=\"agent-view\" role=\"tabpanel\" aria-labelledby=\"tab-agent\" hidden")
    );
    assert!(
        page.contains("id=\"errors-view\" role=\"tabpanel\" aria-labelledby=\"tab-errors\" hidden")
    );
    assert_eq!(page.matches("role=\"tabpanel\"").count(), 3);
    assert_eq!(page.matches("role=\"tab\"").count(), 3);

    let messaging_html = &page[messaging..agent];
    let agent_html = &page[agent..errors];
    let errors_html = &page[errors..script];
    assert!(messaging_html.contains("id=\"cards\""));
    assert!(messaging_html.contains("aria-label=\"Provider delivery\""));
    assert!(messaging_html.contains("aria-label=\"Room delivery\""));
    assert!(messaging_html.contains("id=\"graph\""));
    assert!(messaging_html.contains("<th>p50</th><th>p90</th><th>p99</th><th>n</th>"));
    assert!(messaging_html.contains("id=\"bus-note\""));
    assert!(!messaging_html.contains("id=\"failures\""));
    assert!(!messaging_html.contains("How these are counted"));
    assert!(page.contains("Delivery success"));
    assert!(page.contains("Average latency"));
    assert!(page.contains("Pending and failed"));
    assert!(page.contains("combined.id='pending-failed'"));

    assert!(agent_html.contains("Launches by provider"));
    assert!(agent_html.contains("aria-label=\"Agent launch success by provider\""));
    assert!(agent_html.contains("id=\"launch-graph\""));
    assert!(agent_html.contains("id=\"launch-failure-section\""));
    assert!(agent_html.contains("aria-label=\"Failed agent launch evidence\""));
    assert!(!agent_html.contains("id=\"failures\""));

    assert!(errors_html.contains("id=\"failure-section\""));
    assert!(errors_html.contains("id=\"failures\""));
    assert!(errors_html.contains("aria-label=\"Failed request evidence\""));
    assert!(errors_html.contains(">Stage</th>"));
    assert!(errors_html.contains(">Reason</th>"));
    assert!(errors_html.contains(">Responsibility</th>"));
    assert!(errors_html.contains("How these are counted"));
    assert!(errors_html.contains("aria-label=\"Outcome classification\""));
    assert!(errors_html.contains("aria-label=\"Failure responsibility\""));
    assert!(errors_html.contains(">External</td>"));
    assert!(errors_html.contains(">Bus-caused</td>"));
    assert!(page.contains("showTab('errors')"));
    assert!(page.contains("drill.mode==='providers'"));
    assert!(page.contains("drill.mode==='rooms'"));
    assert!(page.contains("f.hour_start_ms===drill.hour"));
    assert!(page.contains("f.reason===drill.reason"));
    assert!(page.contains("f.cause===drill.cause"));
    assert!(page.contains("f.stage"));
    assert!(page.contains("cause:'bus'"));
    assert!(page.contains("cause:'external'"));
    assert!(page.contains("fetch('/data'"));
}

#[test]
fn errors_tab_ranks_failures_and_explains_deliberate_exclusion() {
    let page =
        String::from_utf8(response_for(Path::new("/nonexistent-dashboard-page"), "GET", "/").body)
            .unwrap();
    assert!(page.contains("id=\"failure-ranks\""));
    assert!(page.contains("Top failures by reason"));
    assert!(page.contains("<th>Rank</th><th>Reason</th><th>Responsibility</th><th>Count</th><th>Share</th><th>Stages</th>"));
    assert!(
        page.contains("renderFailureRanking();"),
        "the ranking renders with the page"
    );
    assert!(
        page.contains("Deliberate closures are excluded from dashboard totals, failure shares, delivery rates and latency samples"),
        "deliberate closures belong in message history"
    );
    assert!(
        !page.contains("<th>Request</th>"),
        "the request column was noise beside the message"
    );
}

#[test]
fn errors_tab_tracks_bus_caused_failures_by_hour() {
    let page = page();
    let errors = page.find("id=\"errors-view\"").unwrap();
    let card = page.find("id=\"bus-error-card\"").unwrap();
    let graph = page.find("id=\"bus-error-graph\"").unwrap();
    let filter = page.find("id=\"failure-responsibility\"").unwrap();
    let ranking = page.find("id=\"failure-ranking\"").unwrap();
    let ranks = page.find("id=\"failure-ranks\"").unwrap();
    assert!(errors < card && card < graph && graph < ranking);
    assert!(ranking < filter && filter < ranks);
    assert!(page.contains(">Bus errors</h2>"));
    assert!(page.contains("id=\"bus-error-total\""));
    assert!(page.contains("id=\"bus-error-share\""));
    assert!(page.contains("id=\"bus-error-busiest\""));
    assert!(page.contains("id=\"bus-error-graph\" class=\"scroll\""));
    assert!(page.contains("#bus-error-graph .bar{fill:var(--failure)}"));
    assert!(page.contains(
        "id=\"failure-responsibility\" role=\"group\" aria-label=\"Filter by responsibility\""
    ));
    assert!(page.contains("data-responsibility=\"all\" aria-pressed=\"true\">All</button>"));
    assert!(page.contains("data-responsibility=\"bus\" aria-pressed=\"false\">Bus-caused</button>"));
    assert!(!page.contains("Caller error"));
    assert!(
        page.contains("data-responsibility=\"external\" aria-pressed=\"false\">External</button>")
    );
    assert!(page.contains("failures.filter(f=>f.cause==='bus')"));
    assert!(page.contains("byHour.get(f.hour_start_ms)"));
    assert!(page.contains("f.hour_start_ms===drill.hour"));
    assert!(page.contains("hour:hour.hour_start_ms"));
    assert!(page.contains("renderBusErrors();"));
    assert!(page.contains("renderFailureRanking();"));
    assert!(page.contains(
        "<th>Rank</th><th>Reason</th><th>Responsibility</th><th>Count</th><th>Share</th><th>Stages</th>"
    ));
    assert_eq!(page.matches("role=\"tabpanel\"").count(), 3);
    assert_eq!(page.matches("role=\"tab\"").count(), 3);
    assert!(!page.contains("<select"));
    assert!(!page.contains("how many failed"));
}
