use super::*;

#[test]
fn a_missing_hit_lists_the_names_already_fetched() {
    let hit = |name: &str| Hit {
        name: name.into(),
        row: 0,
        col: 0,
        width: 1,
        height: 1,
    };
    let error = no_hit("settings", &[hit("composer"), hit("room:a"), hit("room:a")]);
    assert_eq!(error["error"]["code"], "not_found");
    assert_eq!(
        error["error"]["message"],
        "no hit named \"settings\"; visible: composer, room:a"
    );
}
