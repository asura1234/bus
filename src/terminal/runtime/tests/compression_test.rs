use super::*;

#[tokio::test]
async fn compression_permit_survives_an_aborted_async_waiter() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = semaphore.clone().acquire_owned().await.unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let handle = spawn_blocking_with_compression_permit(permit, move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
    });
    started_rx.await.unwrap();

    handle.abort();
    assert!(semaphore.clone().try_acquire_owned().is_err());

    release_tx.send(()).unwrap();
    handle.await.unwrap();
    assert!(semaphore.try_acquire_owned().is_ok());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn compression_task_rechecks_history_after_a_read() {
    let suffix = "x".repeat(66);
    let history = (1..=2_000)
        .map(|line| format!("{line:05} {suffix}\r\n"))
        .collect::<String>();
    let runtime = PaneRuntime::test_with_scrollback_bytes(80, 24, 20_000_000, history.as_bytes());

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while runtime.compression.completed_passes() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let completed_before_read = runtime.compression.completed_passes();

    let snapshot = runtime.recent_unwrapped_text_snapshot(usize::MAX);
    assert!(snapshot.text.contains("00001 "));
    assert!(snapshot.text.contains("02000 "));

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while runtime.compression.completed_passes() == completed_before_read {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn compressed_scrollback_survives_shrink_and_grow_resize() {
    let suffix = "x".repeat(66);
    let history = (1..=2_000)
        .map(|line| format!("{line:05} {suffix}\r\n"))
        .collect::<String>();
    let runtime = PaneRuntime::test_with_scrollback_bytes(80, 45, 20_000_000, history.as_bytes());

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while runtime.compression.completed_passes() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    runtime.resize(21, 80, 0, 0);
    let completed_after_shrink = runtime.compression.completed_passes();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while runtime.compression.completed_passes() == completed_after_shrink {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    runtime.resize(45, 80, 0, 0);

    assert_eq!(runtime.current_size(), (45, 80));
    assert_eq!(runtime.terminal_dimensions(), Some((80, 45)));
    assert_eq!(runtime.scroll_metrics().unwrap().viewport_rows, 45);
    let snapshot = runtime.recent_unwrapped_text_snapshot(usize::MAX);
    assert!(snapshot.text.contains("00001 "));
    assert!(snapshot.text.contains("02000 "));
}
