//! Bounded event channels with producer-side backlog evidence.
use super::now_us;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::mpsc;

#[derive(Default)]
struct State {
    times: VecDeque<u64>,
    high_water: usize,
    blocked_us: u64,
    capacity: usize,
    closed: bool,
}
static QUEUES: OnceLock<Mutex<Vec<Weak<Mutex<State>>>>> = OnceLock::new();

pub(crate) struct Sender<T> {
    tx: mpsc::Sender<T>,
    state: Arc<Mutex<State>>,
}
pub(crate) struct Receiver<T> {
    rx: mpsc::Receiver<T>,
    state: Arc<Mutex<State>>,
}
pub(crate) struct Permit<'a, T> {
    permit: mpsc::Permit<'a, T>,
    state: &'a Mutex<State>,
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            state: self.state.clone(),
        }
    }
}
pub(crate) fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel(capacity);
    let state = Arc::new(Mutex::new(State {
        times: VecDeque::with_capacity(capacity + 1),
        capacity,
        ..State::default()
    }));
    if let Ok(mut queues) = QUEUES.get_or_init(Mutex::default).lock() {
        queues.retain(|q| q.strong_count() > 0);
        queues.push(Arc::downgrade(&state));
    }
    (
        Sender {
            tx,
            state: state.clone(),
        },
        Receiver { rx, state },
    )
}
impl<T> Sender<T> {
    #[cfg(test)]
    pub(crate) fn try_send(&self, value: T) -> Result<(), mpsc::error::TrySendError<T>> {
        match self.try_reserve() {
            Ok(permit) => {
                permit.send(value);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(())) => Err(mpsc::error::TrySendError::Full(value)),
            Err(mpsc::error::TrySendError::Closed(())) => {
                Err(mpsc::error::TrySendError::Closed(value))
            }
        }
    }

    pub(crate) fn blocking_send(&self, value: T) -> Result<(), mpsc::error::SendError<T>> {
        let start = now_us();
        let reserved = match self.try_reserve() {
            Ok(permit) => {
                permit.send(value);
                return Ok(());
            }
            Err(mpsc::error::TrySendError::Closed(())) => {
                return Err(mpsc::error::SendError(value))
            }
            Err(mpsc::error::TrySendError::Full(())) => block_on(self.tx.reserve()),
        };
        let permit = match reserved {
            Ok(permit) => permit,
            Err(_) => return Err(mpsc::error::SendError(value)),
        };
        if let Ok(mut state) = self.state.lock() {
            state.blocked_us += now_us().saturating_sub(start);
        }
        Permit {
            permit,
            state: &self.state,
        }
        .send(value);
        Ok(())
    }
    pub(crate) fn try_reserve(&self) -> Result<Permit<'_, T>, mpsc::error::TrySendError<()>> {
        Ok(Permit {
            permit: self.tx.try_reserve()?,
            state: &self.state,
        })
    }
}
impl<T> Permit<'_, T> {
    pub(crate) fn send(self, value: T) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.closed {
            state.times.push_back(now_us());
        }
        state.high_water = state.high_water.max(state.times.len());
        self.permit.send(value);
    }
}
impl<T> Receiver<T> {
    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        let value = self.rx.try_recv()?;
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .times
            .pop_front();
        Ok(value)
    }

    pub(crate) async fn recv(&mut self) -> Option<T> {
        let value = self.rx.recv().await?;
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .times
            .pop_front();
        Some(value)
    }
    pub(crate) fn len(&self) -> usize {
        self.rx.len()
    }
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.times.clear();
    }
}
/// Reserve waits outside bookkeeping locks, using the current producer thread's
/// park token. The wake token survives a wake between poll and park.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Wake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}
pub(super) fn snapshots() -> Vec<serde_json::Value> {
    let Some(queues) = QUEUES.get() else {
        return Vec::new();
    };
    let Ok(queues) = queues.lock() else {
        return Vec::new();
    };
    queues
        .iter()
        .filter_map(|q| {
            let state = q.upgrade()?;
            let state = state.lock().ok()?;
            Some(
                serde_json::json!({"depth":state.times.len(), "high_water":state.high_water,
            "oldest_ms":state.times.front().map_or(0, |at| now_us().saturating_sub(*at) / 1000),
            "producer_wait_us":state.blocked_us, "capacity":state.capacity}),
            )
        })
        .collect()
}
#[cfg(test)]
#[path = "tests/queue_test.rs"]
mod tests;
