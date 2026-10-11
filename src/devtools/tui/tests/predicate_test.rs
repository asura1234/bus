use super::*;
use serde_json::json;

fn state() -> Value {
    json!({"visible_room": 2, "rooms": [{"name": "MASTER"}, {"name": "review"}],
           "agents": [{"status": "idle", "tags": ["a", "b"]}, {"status": "working"}]})
}

#[test]
fn paths_operators_and_wildcards() {
    let s = state();
    assert!(Predicate::parse("visible_room == 2").unwrap().holds(&s));
    assert!(Predicate::parse("visible_room == \"2\"").unwrap().holds(&s));
    assert!(Predicate::parse("rooms.1.name == review")
        .unwrap()
        .holds(&s));
    assert!(Predicate::parse("rooms.*.name contains rev")
        .unwrap()
        .holds(&s));
    assert!(Predicate::parse("agents.*.status == working")
        .unwrap()
        .holds(&s));
    assert!(Predicate::parse("agents.0.tags contains \"b\"")
        .unwrap()
        .holds(&s));
    assert!(Predicate::parse("visible_room >= 2").unwrap().holds(&s));
    assert!(Predicate::parse("visible_room > 1").unwrap().holds(&s));
    assert!(Predicate::parse("visible_room < 3").unwrap().holds(&s));
    assert!(Predicate::parse("visible_room <= 2").unwrap().holds(&s));
    assert!(!Predicate::parse("visible_room != 2").unwrap().holds(&s));
    assert!(Predicate::parse("missing.path != 1").unwrap().holds(&s));
    assert!(!Predicate::parse("missing.path == 1").unwrap().holds(&s));
    assert!(!Predicate::parse("rooms > 1").unwrap().holds(&s));
    assert!(Predicate::parse("*.0.status == idle").unwrap().holds(&s));
}

#[test]
fn malformed_predicates_are_usage_errors_and_actual_values_are_reported() {
    assert!(Predicate::parse("visible_room").is_err());
    assert!(Predicate::parse("visible_room ~= 2").is_err());
    let s = state();
    assert_eq!(
        Predicate::parse("visible_room == 9").unwrap().actual(&s),
        json!(2)
    );
    assert_eq!(
        Predicate::parse("nope == 9").unwrap().actual(&s),
        Value::Null
    );
    assert_eq!(
        Predicate::parse("agents.*.status == x").unwrap().actual(&s),
        json!(["idle", "working"])
    );
}
