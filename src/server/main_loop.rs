//! Headless server mode — runs the bus event loop without a real terminal.
//!
//! The server:
//! - Does not enter raw mode or read stdin
//! - Creates and listens on both `herdr.sock` (existing JSON API) and
//!   `herdr-client.sock` (new binary protocol)
//! - Initializes AppState and all PTYs from session restore or fresh state
//! - Runs the main event loop (drain events, drain API requests, scheduled tasks)
//! - Renders to a virtual ratatui Buffer in memory
//! - Accepts client connections on the client socket
//! - Streams frames to connected clients after each render
//! - Routes client input events through the existing input pipeline
//! - Continues running after client disconnect
//! - Handles stale socket cleanup, explicit server stop, minimum terminal size,
//!   and pane spawn failure during restore
use crate::protocol::api::schema;
use crate::server::clients::connection::latest_shell_client;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use interprocess::local_socket::traits::Listener as _;
#[cfg(windows)]
use interprocess::local_socket::traits::Stream as _;
#[cfg(unix)]
use interprocess::local_socket::ListenerNonblockingMode;
use tokio::sync::mpsc;
use tracing::info;
#[cfg(windows)]
use tracing::{debug, error, warn};

use crate::platform::ipc::{
    bind_local_listener, socket_file_identity, LocalListener, SocketFileIdentity,
};
use crate::server::app;
use crate::terminal::events::TerminalEvent;
use crate::utils::config;

use crate::protocol::wire::ServerMessage;
#[cfg(unix)]
use crate::server::clients::accept::accept_pending_client_connections;
use crate::server::clients::connection::ClientConnection;
use crate::server::clients::transport::ServerEvent;
use crate::server::socket_paths::{prepare_socket_path, restrict_socket_permissions};
use crate::utils::socket_paths::client_socket_path;

pub use crate::server::startup::run_server;

pub(super) fn notification_show_result(
    id: String,
    shown: bool,
    reason: schema::NotificationShowReason,
) -> String {
    serde_json::to_string(&schema::SuccessResponse {
        id,
        result: schema::ResponseResult::NotificationShow { shown, reason },
    })
    .unwrap_or_else(|_| "{}".to_string())
}

pub(super) fn non_empty_body(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

// ---------------------------------------------------------------------------
// Loop event enum for the headless server event loop
// ---------------------------------------------------------------------------

/// Events that the headless server event loop can process.
enum LoopEvent {
    Timer,
    Internal(TerminalEvent),
    Api(Box<crate::server::api::ApiRequestMessage>),
    ServerEvent(ServerEvent),
    RenderRequested,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How often the idle headless loop wakes to poll the local listener for new
/// client connections.
///
/// The listener is non-blocking and not integrated into `tokio::select!`, so
/// a low-frequency wake is required to notice new thin-client attaches while
/// otherwise idle. Keep this much slower than the old resize-poll cadence to
/// avoid reintroducing the idle CPU spin.
const CLIENT_ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Headless server
// ---------------------------------------------------------------------------

/// The headless server — runs the bus event loop without a real terminal.
// Sibling loop, lifecycle, client, rendering and test modules share this state.
pub struct HeadlessServer {
    pub(super) app: app::App,
    // Held so dropping HeadlessServer owns API request channel and server shutdown.
    pub(super) _api_tx: Option<crate::server::api::ApiRequestSender>,
    pub(super) _api_server: Option<crate::server::api::ServerHandle>,
    #[cfg(unix)]
    pub(super) client_listener: LocalListener,
    pub(super) client_socket_path: PathBuf,
    pub(super) client_socket_identity: SocketFileIdentity,
    pub(super) clients: HashMap<u64, ClientConnection>,
    #[cfg(unix)]
    pub(super) next_client_id: u64,
    /// The client currently driving session-wide host presentation and side effects.
    pub(super) foreground_client_id: Option<u64>,
    /// Ephemeral shell connection controlling PTY geometry for each stable tab id.
    pub(super) tab_geometry_controllers: HashMap<String, u64>,
    /// Process-local identity used to reject shell replacements from an earlier server boot.
    pub(super) client_shell_boot_id: String,
    /// Outer window title last pushed, paired with the client that received it.
    /// Keying on the client means a newly attached terminal is written to even
    /// when the title itself has not changed, without every code path that
    /// changes the foreground client having to remember to invalidate this.
    pub(super) sent_window_title: Option<(u64, Option<String>)>,
    /// Window title set through `client.window_title.set`. While present it wins
    /// over the configured `ui.window_title` until the API clears it again.
    pub(super) api_window_title: Option<String>,
    /// Full server config warning shown to clients that use server keybindings.
    pub(super) server_config_diagnostic: Option<String>,
    /// Deferred application-history reads currently driving alternate-screen viewports.
    pub(super) pending_alt_screen_reads:
        Vec<crate::server::terminals::scrollback_read::PendingAltScreenRead>,
    /// Reads waiting for an alternate-screen traversal of the same terminal to finish.
    pub(super) deferred_alt_screen_reads: Vec<crate::server::api::ApiRequestMessage>,
    /// Monotonic activity counter used to pick the most recently active client.
    pub(super) next_activity_stamp: u64,
    /// Configured virtual terminal size used when no clients are connected.
    pub(super) headless_size: (u16, u16),
    /// Shared pane runtime size derived from the foreground client, or the
    /// configured headless size when no clients are connected.
    pub(super) effective_size: (u16, u16),
    /// Flag set when shutdown is initiated.
    pub(super) shutting_down: bool,
    /// Flag set by Ctrl+C or `server stop` signal.
    pub(super) should_quit: Arc<AtomicBool>,
    /// Channel for receiving server events from client connection threads.
    pub(super) server_event_rx: mpsc::Receiver<ServerEvent>,
    /// Sender for server events (cloned for each client thread).
    pub(super) server_event_tx: mpsc::Sender<ServerEvent>,
}

#[cfg(windows)]
pub(super) fn spawn_windows_client_accept_thread(
    listener: LocalListener,
    should_quit: Arc<AtomicBool>,
    server_event_tx: mpsc::Sender<ServerEvent>,
) {
    std::thread::spawn(move || {
        let mut next_client_id = 1_u64;
        while !should_quit.load(Ordering::Acquire) {
            let stream = match listener.accept() {
                Ok(stream) => stream,
                Err(err) => {
                    if should_quit.load(Ordering::Acquire) {
                        break;
                    }
                    error!(err = %err, "client listener accept failed");
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };

            let client_id = next_client_id;
            next_client_id = next_client_id.saturating_add(1);

            if let Err(err) = stream.set_nonblocking(true) {
                warn!(err = %err, "failed to set client stream nonblocking");
                continue;
            }

            let should_quit = should_quit.clone();
            let server_event_tx = server_event_tx.clone();
            std::thread::spawn(move || {
                if let Err(err) = crate::server::clients::transport::handle_client_handshake(
                    stream,
                    client_id,
                    &server_event_tx,
                    &should_quit,
                ) {
                    debug!(client_id, err = %err, "client handshake failed");
                }
            });
        }
    });
}

impl HeadlessServer {
    /// Creates and starts the headless server.
    ///
    /// This:
    /// 1. Prepares the client socket path (cleans up stale sockets)
    /// 2. Binds the client socket listener
    /// 3. Returns the server ready to run
    pub fn new(
        app: app::App,
        config_diagnostics: &[String],
        api_tx: Option<crate::server::api::ApiRequestSender>,
        api_server: Option<crate::server::api::ServerHandle>,
        should_quit: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let client_path = client_socket_path();
        prepare_socket_path(&client_path)?;

        let listener = bind_local_listener(&client_path)?;
        restrict_socket_permissions(&client_path)?;
        let client_socket_identity = socket_file_identity(&client_path)?;
        info!(path = %client_path.display(), "client protocol socket listening");

        // Set non-blocking on Unix so we can poll it from the event loop.
        #[cfg(unix)]
        listener.set_nonblocking(ListenerNonblockingMode::Accept)?;

        // Channel for server events from client threads.
        let (server_event_tx, server_event_rx) = mpsc::channel(64);
        #[cfg(windows)]
        spawn_windows_client_accept_thread(listener, should_quit.clone(), server_event_tx.clone());

        let headless_size = app.state.headless_size;
        let server_config_diagnostic = config::config_diagnostic_summary(config_diagnostics);
        Ok(Self {
            app,
            _api_tx: api_tx,
            _api_server: api_server,
            #[cfg(unix)]
            client_listener: listener,
            client_socket_path: client_path,
            client_socket_identity,
            clients: HashMap::new(),
            #[cfg(unix)]
            next_client_id: 1,
            foreground_client_id: None,
            tab_geometry_controllers: HashMap::new(),
            client_shell_boot_id: format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            sent_window_title: None,
            api_window_title: None,
            server_config_diagnostic,
            pending_alt_screen_reads: Vec::new(),
            deferred_alt_screen_reads: Vec::new(),
            next_activity_stamp: 1,
            headless_size,
            effective_size: headless_size,
            shutting_down: false,
            should_quit,
            server_event_rx,
            server_event_tx,
        })
    }

    /// Runs the headless server event loop until shutdown.
    ///
    /// This is the server's main runtime loop. It:
    /// - Drains internal events (pane death, state changes)
    /// - Drains API requests (from the JSON socket)
    /// - Accepts new client connections
    /// - Reads client messages and routes input
    /// - Handles scheduled tasks (session save, agent resumes, etc.)
    /// - Renders virtually and streams frames to clients
    pub async fn run(&mut self) -> io::Result<()> {
        // Register SIGINT handler for graceful shutdown.
        let should_quit = self.should_quit.clone();
        let quit_notify = self.server_event_tx.clone();
        ctrlc_handler(should_quit, quit_notify);

        let mut needs_render = true;
        let mut needs_full_render = true;

        loop {
            crate::utils::render::prof::event("loop.tick");
            crate::utils::render::prof::flush_if_due();
            self.app.reap_finished_detached_processes();

            // If shutdown has been initiated, complete it and exit.
            if self.shutting_down {
                self.complete_shutdown().await?;
                break;
            }

            // Check if we should start shutting down.
            if self.app.state.should_quit || self.should_quit.load(Ordering::Acquire) {
                self.drain_internal_events_with_forwarding_up_to(
                    crate::server::app::APP_EVENT_CHANNEL_CAPACITY,
                );
                self.initiate_shutdown();
                continue;
            }

            if !self.drain_loop_inputs(&mut needs_render, &mut needs_full_render)? {
                continue;
            }
            let now = Instant::now();
            self.schedule_loop_work(now, &mut needs_render, &mut needs_full_render);
            if self.render_pending_loop_work(now, &mut needs_render, &mut needs_full_render) {
                continue;
            }
            let event = self.wait_for_loop_event(now, needs_render).await;
            self.apply_loop_event(event, &mut needs_render, &mut needs_full_render);
        }

        // Save session on exit.
        if self.app.policy.persist_session {
            self.app.save_session_on_shutdown();
        }

        info!("headless server exiting");
        Ok(())
    }

    fn drain_loop_inputs(
        &mut self,
        needs_render: &mut bool,
        needs_full_render: &mut bool,
    ) -> io::Result<bool> {
        // 1. Check the coalesced render signal from PTY readers and generic runtime work.
        if self.app.render_dirty.is_pending() {
            *needs_render = true;
            crate::utils::render::prof::event("render.request.signal");
        }
        // 2. Drain a bounded internal-event batch. API handlers perform an
        // exhaustive forwarding-aware drain before reading pane/runtime state.
        if self.drain_internal_events_with_forwarding() {
            *needs_render = true;
            *needs_full_render = true;
            crate::utils::render::prof::event("full_render_cause.internal_events");
        }
        if self.should_quit.load(Ordering::Acquire) {
            return Ok(false);
        }
        // 3. Drain API requests.
        if self.drain_api_requests_with_shutdown_check() {
            *needs_render = true;
            *needs_full_render = true;
            crate::utils::render::prof::event("full_render_cause.api_requests");
        }
        if self.should_quit.load(Ordering::Acquire) {
            return Ok(false);
        }

        self.app.sync_focus_events();
        self.app.sync_session_save_schedule();

        // 4. Accept new client connections.
        self.accept_client_connections()?;

        // 5. Drain server events from client threads.
        if self.drain_server_events() {
            *needs_render = true;
            *needs_full_render = true;
            crate::utils::render::prof::event("full_render_cause.server_events");
        }
        if self.should_quit.load(Ordering::Acquire) {
            return Ok(false);
        }

        Ok(true)
    }

    fn schedule_loop_work(
        &mut self,
        now: Instant,
        needs_render: &mut bool,
        needs_full_render: &mut bool,
    ) {
        // 6. Handle scheduled tasks.
        if self.handle_scheduled_tasks_headless(now, *needs_render) {
            *needs_render = true;
            *needs_full_render = true;
            crate::utils::render::prof::event("full_render_cause.scheduled_tasks");
        }

        self.poll_pending_alt_screen_reads(now);
        if self.process_deferred_alt_screen_reads() {
            *needs_render = true;
            *needs_full_render = true;
        }

        if latest_shell_client(&self.clients).is_some() && self.app.ensure_default_workspace() {
            *needs_render = true;
            *needs_full_render = true;
            crate::utils::render::prof::event("full_render_cause.default_workspace");
        }

        self.drain_client_config_reload_request();
        self.sync_immediate_pty_sources();
        self.stream_host_mouse_capture_mode();
        self.stream_direct_terminal_keyboard_mode();
    }

    fn render_pending_loop_work(
        &mut self,
        now: Instant,
        needs_render: &mut bool,
        needs_full_render: &mut bool,
    ) -> bool {
        // 7. Render virtually and stream frames. Hidden-only PTY work keeps a
        // bounded classification cadence without delaying presentation work
        // that joins the same coalesced request.
        let render_cadence_due = self.app.can_render_now(now);
        if *needs_render
            && (render_cadence_due
                || (self.app.can_present_now(now)
                    && self.has_pending_presentation_work(*needs_full_render)))
        {
            crate::utils::render::prof::event("render.attempt");
            let render_request = self.app.render_dirty.take();
            let pty_dirty = !render_request.pty_sources.is_empty();
            if pty_dirty {
                crate::utils::render::prof::event("render.attempt.pty_dirty");
                crate::utils::render::prof::counter(
                    "render.attempt.pty_sources",
                    render_request.pty_sources.len() as u64,
                );
            }
            if render_request.generic {
                *needs_full_render = true;
                crate::utils::render::prof::event("full_render_cause.generic_dirty");
            }
            let (sidebar_title_changed, outer_title_synced) =
                self.sync_terminal_title_sources(&render_request.terminal_title_sources);
            if sidebar_title_changed {
                *needs_full_render = true;
                crate::utils::render::prof::event("full_render_cause.terminal_title_sidebar");
            }
            if *needs_full_render && !outer_title_synced {
                self.sync_window_title();
            }
            if !*needs_full_render && !pty_dirty {
                // A synchronized-output OSC title can be the only pending work.
                // Its deferred PTY repaint has its own signal; do not manufacture
                // a full UI render for this client-local side effect.
                *needs_render = false;
                return true;
            }
            let hidden_only = pty_dirty
                && !*needs_full_render
                && !self.pty_sources_visible_to_any_render_target(&render_request.pty_sources);
            if hidden_only {
                crate::utils::render::prof::event("render.skipped.hidden_sources");
            } else if !*needs_full_render
                && self.render_retained_pane_surface_and_stream(&render_request.pty_sources)
            {
                crate::utils::render::prof::event("retained_surface.invoke");
            } else {
                crate::utils::render::prof::event("full_render.invoke");
                self.render_and_stream();
            }
            self.app.record_render_attempt(now, !hidden_only);
            *needs_render = false;
            *needs_full_render = false;
            return true;
        }

        false
    }

    async fn wait_for_loop_event(&mut self, now: Instant, needs_render: bool) -> LoopEvent {
        // 8. Wait for next event.
        let next_deadline = self
            .app
            .next_headless_loop_deadline(now, needs_render)
            .map(|deadline| deadline.min(now + CLIENT_ACCEPT_POLL_INTERVAL))
            .or(Some(now + CLIENT_ACCEPT_POLL_INTERVAL));
        let next_deadline = self
            .pending_alt_screen_reads
            .iter()
            .map(|pending| pending.next_deadline())
            .fold(next_deadline, |deadline, pending| {
                Some(deadline.map_or(pending, |current| current.min(pending)))
            });
        {
            tokio::select! {
                maybe_api = self.app.api_rx.recv() => match maybe_api {
                    Some(msg) => LoopEvent::Api(Box::new(msg)),
                    None => LoopEvent::Timer,
                },
                maybe_ev = self.app.event_rx.recv() => match maybe_ev {
                    Some(ev) => LoopEvent::Internal(ev),
                    None => LoopEvent::Timer,
                },
                maybe_server_ev = self.server_event_rx.recv() => match maybe_server_ev {
                    Some(ev) => LoopEvent::ServerEvent(ev),
                    None => LoopEvent::Timer,
                },
                _ = sleep_until_or_pending(next_deadline) => LoopEvent::Timer,
                _ = self.app.render_notify.notified() => LoopEvent::RenderRequested,
            }
        }
    }

    fn apply_loop_event(
        &mut self,
        event: LoopEvent,
        needs_render: &mut bool,
        needs_full_render: &mut bool,
    ) {
        if self.should_quit.load(Ordering::Acquire) {
            match event {
                LoopEvent::Internal(ev) => {
                    self.handle_internal_event_with_forwarding(ev);
                }
                LoopEvent::ServerEvent(ServerEvent::ClientShellConnected { writer, .. }) => {
                    if let Ok(message) =
                        Self::frame_server_message(&ServerMessage::ServerShutdown {
                            reason: Some("server is shutting down".to_owned()),
                        })
                    {
                        let _ = writer.control.send(message);
                    }
                }
                _ => {}
            }
            return;
        }

        match event {
            LoopEvent::Timer => {}
            LoopEvent::Internal(ev) => {
                if self.handle_internal_event_with_forwarding(ev) {
                    *needs_render = true;
                    *needs_full_render = true;
                }
            }
            LoopEvent::Api(msg) => {
                if self.handle_api_request_with_shutdown_check(*msg) {
                    *needs_render = true;
                    *needs_full_render = true;
                }
            }
            LoopEvent::ServerEvent(ev) => {
                if self.handle_server_event(ev) {
                    *needs_render = true;
                    *needs_full_render = true;
                }
            }
            LoopEvent::RenderRequested => {
                if self.app.render_dirty.is_pending() {
                    *needs_render = true;
                }
            }
        }
    }

    /// Accepts pending client connections from the non-blocking listener.
    #[cfg(unix)]
    fn accept_client_connections(&mut self) -> io::Result<()> {
        accept_pending_client_connections(
            &self.client_listener,
            &mut self.next_client_id,
            &self.should_quit,
            &self.server_event_tx,
        )
    }

    /// Windows named-pipe clients can block in connect unless the server has a
    /// pending blocking accept. The dedicated accept thread handles that path.
    #[cfg(windows)]
    fn accept_client_connections(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Handle scheduled tasks for the headless server.
    ///
    /// Similar to the former App scheduler but without terminal resize polling.
    pub(super) fn handle_scheduled_tasks_headless(
        &mut self,
        now: Instant,
        geometry_dirty: bool,
    ) -> bool {
        let mut changed = false;

        // No resize polling needed — server has no terminal.
        // Client resize messages drive size changes instead.

        if self
            .app
            .config_diagnostic_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.config_diagnostic_deadline = None;
            self.app.state.config_diagnostic = None;
            changed = true;
        }

        if self
            .app
            .toast_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.toast_deadline = None;
            self.app.state.toast = None;
            changed = true;
        }

        if self
            .app
            .state
            .next_pending_agent_notification_deadline()
            .is_some_and(|deadline| now >= deadline)
        {
            let previous_toast = self.app.state.toast.clone();
            let mut deliveries = self.app.state.drain_due_agent_notifications(now);
            if !deliveries.is_empty() {
                self.app
                    .refresh_agent_notification_delivery_contexts(&mut deliveries);
                self.app.sync_toast_deadline(previous_toast);
                for delivery in &deliveries {
                    self.forward_agent_notification_delivery(delivery);
                }
                changed = true;
            }
        }

        if self
            .app
            .session_save_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.start_background_session_save();
        }

        if geometry_dirty {
            self.app.pending_agent_resume_deadline = None;
        } else {
            self.app.sync_pending_agent_resume_deadline(now);
            changed |= self
                .app
                .start_pending_agent_resumes(self.app.pending_agent_resume_due(now));
        }
        changed
    }
}

impl Drop for HeadlessServer {
    fn drop(&mut self) {
        let staged_files = self
            .clients
            .drain()
            .flat_map(|(_, client)| client.staged_clipboard_files)
            .collect::<Vec<_>>();
        crate::server::clients::clipboard_images::remove_files(staged_files);
        let _ = self.cleanup_sockets();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Installs a Ctrl+C handler that sets the should_quit flag and wakes up
/// the event loop by sending a QuitSignal on the server event channel.
fn ctrlc_handler(should_quit: Arc<AtomicBool>, server_event_tx: mpsc::Sender<ServerEvent>) {
    let _ = ctrlc::set_handler(move || {
        should_quit.store(true, Ordering::Release);
        // Wake up the event loop so the quit flag is checked promptly.
        let _ = server_event_tx.try_send(ServerEvent::QuitSignal);
    });
}

/// Sleep until a deadline, or return pending if none.
async fn sleep_until_or_pending(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => std::future::pending().await,
    }
}

pub(super) fn sanitize_notification_text(value: &str, max_chars: usize) -> Option<String> {
    let mut sanitized = String::new();
    let mut previous_space = false;
    for ch in value.chars() {
        let replacement = if ch == '\n' || ch == '\r' || ch == '\t' {
            Some(' ')
        } else if ch.is_control() {
            None
        } else {
            Some(ch)
        };
        let Some(ch) = replacement else {
            continue;
        };
        if ch.is_whitespace() {
            if previous_space {
                continue;
            }
            previous_space = true;
            sanitized.push(' ');
        } else {
            previous_space = false;
            sanitized.push(ch);
        }
        if sanitized.chars().count() >= max_chars {
            break;
        }
    }
    let sanitized = sanitized.trim().to_string();
    (!sanitized.is_empty()).then_some(sanitized)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// In-process tests are declared once from server::tests.
