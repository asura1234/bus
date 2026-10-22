use super::*;
#[test]
fn grok_allowance_remains_explicitly_unavailable() {
    let reason = allowance_unavailable_reason();
    assert!(reason.contains("no verified local allowance source"));
}
