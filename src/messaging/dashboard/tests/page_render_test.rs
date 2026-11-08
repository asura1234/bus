use super::response_for;
use std::path::Path;

#[test]
fn page_keeps_hourly_and_failure_sections_and_drops_recipient_grouping() {
    let page =
        String::from_utf8(response_for(Path::new("/nonexistent-dashboard-page"), "GET", "/").body)
            .unwrap();
    assert!(page.contains("aria-label=\"Provider reliability\""));
    assert!(page.contains("aria-label=\"Room reliability\""));
    assert!(page.contains("aria-label=\"Hourly delivery reliability\""));
    assert!(page.contains("id=\"graph\""));
    assert!(page.contains("id=\"failures\""));
    assert!(page.contains("id=\"failure-section\""));
    assert!(page.contains("How these are counted"));
    assert!(!page.contains("id=\"group-mode\""));
    assert!(!page.contains(">Recipients<"));
    assert!(!page.contains("value=\"recipients\""));
    assert!(!page.contains("toFixed"));
    assert!(page.contains(".caption"));
    assert!(page.contains(
        "Bus reliability</th><th>Delivered</th><th>Attempted</th><th>Pending</th><th>Bus failures</th><th>External</th>"
    ));
    assert!(page.contains("c.open+c.reply_pending"));
    assert!(!page.contains(">Top causes<"));
    assert!(!page.contains("Delivered / attempts"));
    assert!(!page.contains("causeButtons"));
    assert!(page.contains("drill.mode==='providers'"));
    assert!(page.contains("drill.mode==='rooms'"));
    assert!(!page.contains("drill.mode==='recipients'"));
    assert!(!page.contains("metrics.recipients"));
    assert!(!page.contains("metrics?.recipients"));
}
