use super::response_for;
use std::path::Path;

#[test]
fn page_keeps_hourly_and_failure_sections_and_drops_recipient_grouping() {
    let page =
        String::from_utf8(response_for(Path::new("/nonexistent-dashboard-page"), "GET", "/").body)
            .unwrap();
    assert!(page.contains("aria-label=\"Provider delivery\""));
    assert!(page.contains("aria-label=\"Room delivery\""));
    assert!(page.contains("aria-label=\"Hourly delivery\""));
    assert!(page.contains("id=\"graph\""));
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
    assert!(page.contains("id=\"delay-section\""));
    assert!(page.contains("p50"));
    assert!(page.contains("p99"));
    assert!(page.contains("ends at room delivery"));
    assert!(page.contains("href=\"/latency\""));
    assert!(!page.contains("not measured yet"));
    assert!(page.contains("How these are counted"));
    assert!(!page.contains("id=\"group-mode\""));
    assert!(!page.contains(">Recipients<"));
    assert!(!page.contains("value=\"recipients\""));
    assert!(!page.contains("toFixed"));
    assert!(page.contains(".caption"));
    assert!(page.contains(
        "<th>Delivered</th><th>Attempted</th><th>Pending</th><th>Delivery %</th><th>Bus failures</th><th>External</th>"
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
    let delay = page.find("id=\"delay-section\"").unwrap();
    let failure = page.find("id=\"failure-section\"").unwrap();
    let note = &page[bus..failure];
    assert!(note.contains("how many succeeded"), "{note}");
    assert!(!note.contains("how many failed"), "{note}");
    assert!(!note.contains("reply capture"), "{note}");
    assert!(!note.contains("explicit repl"), "{note}");
    assert!(page.contains("series.rates.bus_reliability.caption"));
    assert!(!page.contains("series.rates.reply_capture"));
    assert!(!page.contains("how many failed"));
    assert!(providers < delay && delay < bus && bus < failure);
    assert!(page.contains("drill.mode==='providers'"));
    assert!(page.contains("drill.mode==='rooms'"));
    assert!(!page.contains("drill.mode==='recipients'"));
    assert!(!page.contains("metrics.recipients"));
    assert!(!page.contains("metrics?.recipients"));
}
