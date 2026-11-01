use crate::messaging::model::{BusState, Provider, RoomId};
use std::path::Path;

#[test]
fn launching_into_a_shadowed_repo_posts_no_room_message_and_writes_one_log_line() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-shadow-log");
    let repo = home.join("repo");
    write_collisions(&home, &repo, 1..=2);
    git_init(&repo);

    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = launch_agent(&mut state, room, "shadow", Provider::ClaudeCode, &repo);

    let log = capture_log(|| super::log_warnings_at(&agent, &home));
    assert_eq!(
        notice_texts(&state, room).len(),
        0,
        "skill shadow must not post a room message, got {:?}",
        notice_texts(&state, room)
    );
    let lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("Skill shadow"))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    let line = lines[0];
    assert!(line.contains("claude"), "{line}");
    assert!(line.contains(&repo.display().to_string()), "{line}");
    assert!(line.contains("2 same-name skills"), "{line}");
    assert!(line.contains("skill-01"), "{line}");
    assert!(line.contains("skill-02"), "{line}");

    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_repo_that_never_shadowed_stays_silent() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-shadow-quiet");
    let repo = home.join("repo");
    git_init(&repo);

    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = launch_agent(&mut state, room, "quiet", Provider::ClaudeCode, &repo);

    let log = capture_log(|| super::log_warnings_at(&agent, &home));
    assert_eq!(notice_texts(&state, room).len(), 0);
    assert!(
        !log.contains("Skill shadow"),
        "a repo with no collisions stays off the log: {log}"
    );

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

fn launch_agent(
    state: &mut BusState,
    room: RoomId,
    name: &str,
    provider: Provider,
    repo: &Path,
) -> crate::messaging::model::RoomAgent {
    let agent_id = state
        .create_agent(room, name, provider, repo.to_path_buf(), None)
        .unwrap();
    state.agent(agent_id).unwrap().clone()
}

fn notice_texts(state: &BusState, room: RoomId) -> Vec<String> {
    state
        .notices(room)
        .map(|notice| notice.text.clone())
        .collect()
}

fn capture_log(body: impl FnOnce()) -> String {
    #[derive(Clone)]
    struct Writer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = Writer(buf.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, body);
    let bytes = buf.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}
