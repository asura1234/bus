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
    assert!(!page.contains("series.end_to_end"));
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
