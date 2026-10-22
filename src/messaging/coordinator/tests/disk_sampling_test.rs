use super::*;
use crate::messaging::coordinator::disk_space::DiskSpace;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[test]
fn disk_sampling_is_async_uses_the_data_directory_and_runs_once_per_sixty_seconds() {
    let (mut worker, room, _, _, dir) = setup();
    let (started, observed) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let blocked = Mutex::new(blocked);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    worker.disk_space = DiskSpace::with_source(Arc::new(move |path| {
        counted.fetch_add(1, Ordering::SeqCst);
        started.send(path.to_path_buf()).unwrap();
        blocked
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        Ok(4 * GB)
    }));
    let now = Instant::now();
    let tick_started = Instant::now();
    worker.tick().unwrap();
    assert!(
        tick_started.elapsed() < Duration::from_secs(1),
        "a blocked source must not block the tick"
    );
    assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), dir);
    for _ in 0..5 {
        worker.tick().unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "only one sample in flight");
    assert!(alerts(&worker, room).is_empty());
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while alerts(&worker, room).is_empty() && Instant::now() < until {
        worker.tick().unwrap();
        std::thread::yield_now();
    }
    assert_eq!(alerts(&worker, room).len(), 1);
    worker
        .refresh_disk_space_at(now + Duration::from_secs(59))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    worker
        .refresh_disk_space_at(now + Duration::from_secs(60) + Duration::from_millis(50))
        .unwrap();
    assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), dir);
    release.send(()).unwrap();
    cleanup(worker, dir);
}

#[test]
fn disk_sampling_errors_keep_latches_and_retry_on_the_next_interval() {
    let (mut worker, room, _, _, dir) = setup();
    sample(&mut worker, 4 * GB);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    worker.disk_space = DiskSpace::with_source(Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("injected unavailable volume"))
    }));
    let now = Instant::now();
    worker.refresh_disk_space_at(now).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while worker.disk_space.in_flight() && Instant::now() < until {
        worker.refresh_disk_space_at(now).unwrap();
        std::thread::yield_now();
    }
    assert!(!worker.disk_space.in_flight());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    worker
        .refresh_disk_space_at(now + Duration::from_secs(59))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    worker
        .refresh_disk_space_at(now + Duration::from_secs(60))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while calls.load(Ordering::SeqCst) < 2 && Instant::now() < until {
        std::thread::yield_now();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    sample(&mut worker, 4 * GB);
    assert_eq!(
        alerts(&worker, room).len(),
        1,
        "failed measurements never rearm"
    );
    cleanup(worker, dir);
}

#[test]
fn disk_sample_completion_uses_current_shared_settings() {
    let (mut worker, room, _, _, dir) = setup();
    let (sent, measured) = mpsc::channel();
    worker.disk_space = DiskSpace::with_source(Arc::new(move |_| {
        sent.send(()).unwrap();
        Ok(15 * GB)
    }));
    worker.tick().unwrap();
    measured.recv_timeout(Duration::from_secs(1)).unwrap();
    std::fs::write(
        dir.join("settings.json"),
        json!({"disk_warning_gb": 30,"disk_critical_gb": 16}).to_string(),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while alerts(&worker, room).is_empty() && Instant::now() < until {
        worker.tick().unwrap();
        std::thread::yield_now();
    }
    assert_eq!(alerts(&worker, room).len(), 1);
    cleanup(worker, dir);
}
