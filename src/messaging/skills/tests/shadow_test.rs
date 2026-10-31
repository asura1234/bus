use crate::messaging::model::{BusState, Provider, RoomId};
use std::path::Path;

#[test]
fn one_digest_per_repo_until_the_shadowed_set_changes() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-shadow-digest");
    let repo = home.join("repo");
    write_collisions(&home, &repo, 1..=24);
    git_init(&repo);

    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent_id = state
        .create_agent(room, "shadow", Provider::ClaudeCode, repo.clone(), None)
        .unwrap();
    let agent = state.agent(agent_id).unwrap().clone();

    super::post_warnings_at(&mut state, &agent, &home).unwrap();
    let first = notice_texts(&state, room);
    assert_eq!(
        first.len(),
        1,
        "24 collisions in one repo produce one message, got {first:?}"
    );
    assert!(
        first[0].contains("Claude Code gives the user copy precedence"),
        "the digest names which side wins"
    );
    assert_eq!(first[0].matches("Same-name repo skill").count(), 24);
    for index in 1..=24 {
        assert!(first[0].contains(&format!("skill-{index:02}")));
    }

    super::post_warnings_at(&mut state, &agent, &home).unwrap();
    assert_eq!(
        notice_texts(&state, room).len(),
        1,
        "a second call with the same set produces none"
    );

    write_collisions(&home, &repo, 25..=25);
    super::post_warnings_at(&mut state, &agent, &home).unwrap();
    let notices = notice_texts(&state, room);
    assert_eq!(notices.len(), 2, "adding a path produces one new digest");
    assert_eq!(notices[1].matches("Same-name repo skill").count(), 25);
    assert!(notices[1].contains("skill-25"));
    assert!(!notices[0].contains("skill-25"));

    std::fs::remove_dir_all(home).unwrap();
}

fn write_collisions(home: &Path, repo: &Path, indexes: impl Iterator<Item = u32>) {
    for index in indexes {
        let name = format!("skill-{index:02}");
        for directory in [
            home.join(".claude/skills").join(&name),
            repo.join(".claude/skills").join(&name),
        ] {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("SKILL.md"), "skill").unwrap();
        }
    }
}

fn git_init(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    let output = std::process::Command::new("git")
        .arg("init")
        .arg(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git init: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn notice_texts(state: &BusState, room: RoomId) -> Vec<String> {
    state
        .notices(room)
        .map(|notice| notice.text.clone())
        .collect()
}
