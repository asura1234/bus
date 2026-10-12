//! The server's perf metrics; the recorder writes them to `perf/server.jsonl`.
use crate::utils::perf::{Gauge, LoopWatch, Timer};

/// One main loop pass: draining inputs, scheduled work and rendering, or one loop event.
pub(crate) static SERVER_LOOP: LoopWatch = LoopWatch::new("server.loop");
/// Handling one API request on the main loop; slow ones are logged with their method.
pub(crate) static API: Timer = Timer::new("server.api");
/// API requests waiting behind the one being handled.
pub(crate) static API_QUEUE: Gauge = Gauge::new("server.api_queue");
/// Rendering and streaming frames to clients.
pub(crate) static RENDER: Timer = Timer::new("server.render");
/// Writing the session snapshot to disk.
pub(crate) static SESSION_SAVE: Timer = Timer::new("server.session_save");
