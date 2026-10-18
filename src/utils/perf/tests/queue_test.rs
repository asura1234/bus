use super::*;
#[tokio::test]
async fn backlog_is_visible_while_consumer_does_nothing() {
    let (tx, mut rx) = channel(2);
    tx.try_send(10).unwrap();
    tx.try_reserve().unwrap().send(20);
    std::thread::sleep(std::time::Duration::from_millis(5));
    let state = tx.state.lock().unwrap();
    assert_eq!(state.times.len(), 2);
    assert!(now_us() - state.times[0] >= 5000);
    assert_eq!(state.high_water, 2);
    drop(state);
    assert_eq!(rx.recv().await, Some(10));
    assert_eq!(rx.recv().await, Some(20));
    assert!(tx.state.lock().unwrap().times.is_empty());
    assert_eq!(tx.state.lock().unwrap().high_water, 2);
}
#[test]
fn blocked_producer_unblocks_after_dequeue_and_close_does_not_leak_depth() {
    let (tx, mut rx) = channel(1);
    tx.blocking_send(1).unwrap();
    let producer = std::thread::spawn(move || {
        tx.blocking_send(2).unwrap();
        tx
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    assert_eq!(runtime.block_on(rx.recv()), Some(1));
    let tx = producer.join().unwrap();
    assert_eq!(runtime.block_on(rx.recv()), Some(2));
    drop(rx);
    assert!(tx.blocking_send(3).is_err());
    assert!(tx.state.lock().unwrap().times.is_empty());
}
