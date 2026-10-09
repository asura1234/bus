//! Reliable control queue and bounded render slot for each client connection.
use super::events::ServerEvent;
use crate::platform::ipc::LocalStream;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::mpsc::{SendError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Channels owned by the server side of a client writer thread.
#[derive(Clone, Debug)]
pub(crate) struct ClientWriter {
    /// Reliable control messages such as shutdown, notifications, and clipboard writes.
    pub(crate) control: ClientControlWriter,
    /// Droppable render messages. Capacity is one so slow clients cannot build lag.
    pub(crate) render: ClientRenderWriter,
}

impl ClientWriter {
    /// Drops render-lane work that has not yet been claimed by the writer.
    pub(crate) fn discard_pending_render(&self) {
        self.render.queue.discard_pending_render();
    }

    #[cfg(test)]
    pub(crate) fn test_close(&self) {
        self.render.queue.close_writer();
    }

    #[cfg(test)]
    pub(crate) fn test_channel(
        control: std::sync::mpsc::Sender<Vec<u8>>,
        render: std::sync::mpsc::SyncSender<Vec<u8>>,
    ) -> Self {
        let queue = ClientWriterQueue::new();
        let drain = queue.clone();
        let control_writer = ClientControlWriter::queue(queue.clone());
        let mut render_writer = ClientRenderWriter::queue(queue);
        render_writer.test_render = Some(render.clone());
        let writer = Self {
            control: control_writer,
            render: render_writer,
        };
        std::thread::spawn(move || {
            while let Some(item) = drain.recv() {
                let sent = match item {
                    ClientWriteItem::Control(data) => control.send(data).is_ok(),
                    ClientWriteItem::Render(data) => render.send(data).is_ok(),
                };
                if !sent {
                    break;
                }
            }
            drain.close_writer();
        });
        writer
    }
}

#[derive(Debug)]
pub(crate) struct ClientControlWriter {
    queue: Arc<ClientWriterQueue>,
}

#[derive(Debug)]
pub(crate) struct ClientRenderWriter {
    queue: Arc<ClientWriterQueue>,
    #[cfg(test)]
    test_render: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

macro_rules! writer_handle {
    ($type:ty $(, $test_field:ident)?) => {
        impl Clone for $type {
            fn clone(&self) -> Self {
                self.queue.add_sender();
                Self {
                    queue: self.queue.clone(),
                    $(
                        #[cfg(test)]
                        $test_field: self.$test_field.clone(),
                    )?
                }
            }
        }
        impl Drop for $type {
            fn drop(&mut self) {
                self.queue.remove_sender();
            }
        }
    };
}
writer_handle!(ClientControlWriter);
writer_handle!(ClientRenderWriter, test_render);

impl ClientControlWriter {
    pub(super) fn queue(queue: Arc<ClientWriterQueue>) -> Self {
        queue.add_sender();
        Self { queue }
    }

    pub(crate) fn send(&self, data: Vec<u8>) -> Result<(), SendError<Vec<u8>>> {
        self.queue.send_control(data)
    }
}

impl ClientRenderWriter {
    pub(super) fn queue(queue: Arc<ClientWriterQueue>) -> Self {
        queue.add_sender();
        Self {
            queue,
            #[cfg(test)]
            test_render: None,
        }
    }

    pub(crate) fn try_send(&self, data: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        #[cfg(test)]
        if let Some(sender) = &self.test_render {
            return sender.try_send(data);
        }
        self.queue.try_send_render(data)
    }
}

#[derive(Debug)]
pub(super) struct ClientWriterQueue {
    state: Mutex<ClientWriterQueueState>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct ClientWriterQueueState {
    control: VecDeque<Vec<u8>>,
    render: Option<Vec<u8>>,
    senders: usize,
    writer_alive: bool,
}

enum ClientWriteItem {
    Control(Vec<u8>),
    Render(Vec<u8>),
}

impl ClientWriterQueue {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ClientWriterQueueState {
                writer_alive: true,
                ..ClientWriterQueueState::default()
            }),
            ready: Condvar::new(),
        })
    }

    fn add_sender(&self) {
        let mut state = self.lock_state();
        state.senders = state.senders.saturating_add(1);
    }

    fn remove_sender(&self) {
        let mut state = self.lock_state();
        state.senders = state.senders.saturating_sub(1);
        self.ready.notify_one();
    }

    fn send_control(&self, data: Vec<u8>) -> Result<(), SendError<Vec<u8>>> {
        let mut state = self.lock_state();
        if !state.writer_alive {
            return Err(SendError(data));
        }
        state.control.push_back(data);
        self.ready.notify_one();
        Ok(())
    }

    fn try_send_render(&self, data: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        let mut state = self.lock_state();
        if !state.writer_alive {
            return Err(TrySendError::Disconnected(data));
        }
        if state.render.is_some() {
            return Err(TrySendError::Full(data));
        }
        state.render = Some(data);
        self.ready.notify_one();
        Ok(())
    }

    fn discard_pending_render(&self) {
        let mut state = self.lock_state();
        state.render = None;
        self.ready.notify_all();
    }

    fn recv(&self) -> Option<ClientWriteItem> {
        let mut state = self.lock_state();
        loop {
            if let Some(data) = state.control.pop_front() {
                return Some(ClientWriteItem::Control(data));
            }
            if let Some(data) = state.render.take() {
                return Some(ClientWriteItem::Render(data));
            }
            if state.senders == 0 {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn close_writer(&self) {
        let mut state = self.lock_state();
        state.writer_alive = false;
        state.render = None;
        self.ready.notify_all();
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ClientWriterQueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The client writer loop — prioritizes control messages over render frames.
pub(super) fn client_writer_loop(
    mut stream: LocalStream,
    client_id: u64,
    writer_queue: Arc<ClientWriterQueue>,
    server_event_tx: mpsc::Sender<ServerEvent>,
) {
    while let Some(item) = writer_queue.recv() {
        let written = match item {
            ClientWriteItem::Control(data) => write_framed_bytes(&mut stream, &data),
            ClientWriteItem::Render(data) => {
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientWriterDrained { client_id });
                write_framed_bytes(&mut stream, &data)
            }
        };
        if !written {
            let _ = server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
            break;
        }
    }
    writer_queue.close_writer();
    debug!(target: "bus::server::clients::transport", "client writer thread exiting");
}

fn write_framed_bytes(stream: &mut LocalStream, data: &[u8]) -> bool {
    if let Err(err) = stream.write_all(data) {
        debug!(target: "bus::server::clients::transport", err = %err, "client write failed, closing writer");
        return false;
    }
    if let Err(err) = stream.flush() {
        debug!(target: "bus::server::clients::transport", err = %err, "client flush failed, closing writer");
        return false;
    }
    true
}

use crate::protocol::wire::{self as protocol, ServerMessage, MAX_FRAME_SIZE};
use crate::server::clients::connection::ClientConnectionMode;
use crate::server::main_loop::HeadlessServer;

impl HeadlessServer {
    /// Encodes a server message into a length-prefixed frame.
    pub(in crate::server) fn frame_server_message(
        msg: &ServerMessage,
    ) -> Result<Vec<u8>, protocol::FramingError> {
        Self::frame_server_message_with_max(msg, MAX_FRAME_SIZE)
    }

    /// Encodes a server message using an explicit payload cap.
    pub(in crate::server) fn frame_server_message_with_max(
        msg: &ServerMessage,
        max_frame_size: usize,
    ) -> Result<Vec<u8>, protocol::FramingError> {
        let mut framed = Vec::new();
        protocol::write_message(&mut framed, msg)?;
        let payload_len = framed.len().saturating_sub(4);
        if payload_len > max_frame_size {
            return Err(protocol::FramingError::Oversized {
                claimed: payload_len,
                max: max_frame_size,
            });
        }
        Ok(framed)
    }

    /// Sends a message to all connected clients.
    /// Broken connections are tracked and cleaned up.
    pub(in crate::server) fn send_to_all_clients(&mut self, msg: ServerMessage) {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(target: "bus::server::main_loop", err = %err, "failed to serialize message for clients");
                return;
            }
        };

        let mut broken_clients: Vec<u64> = Vec::new();
        for (&client_id, client) in &mut self.clients {
            if client.writer.control.send(serialized.clone()).is_err() {
                debug!(target: "bus::server::main_loop", client_id, "client writer channel closed during broadcast");
                broken_clients.push(client_id);
            }
        }

        // Remove broken clients.
        for client_id in broken_clients {
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    /// Sends an ephemeral semantic event to every connected client-rendered shell.
    pub(in crate::server) fn send_to_client_shells(&mut self, msg: ServerMessage) -> bool {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(target: "bus::server::main_loop", err = %err, "failed to serialize message for client shells");
                return false;
            }
        };
        let client_ids = self
            .clients
            .iter()
            .filter_map(|(&client_id, client)| {
                matches!(client.mode, ClientConnectionMode::ClientShell).then_some(client_id)
            })
            .collect::<Vec<_>>();
        let mut sent = false;
        for client_id in client_ids {
            let Some(client) = self.clients.get(&client_id) else {
                continue;
            };
            if client.writer.control.send(serialized.clone()).is_ok() {
                sent = true;
            } else {
                self.remove_client_and_resize_if_needed(client_id);
            }
        }
        sent
    }

    /// Sends a client-local side effect to the foreground client only.
    pub(in crate::server) fn send_to_foreground_client(&mut self, msg: ServerMessage) -> bool {
        let Some(client_id) = self.foreground_client_id else {
            return false;
        };
        self.send_to_client(client_id, msg)
    }

    /// Sends a message to a specific client. Returns false if the client
    /// was not found or the send failed (client removed).
    pub(in crate::server) fn send_to_client(&mut self, client_id: u64, msg: ServerMessage) -> bool {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(target: "bus::server::main_loop", client_id, err = %err, "failed to serialize message for client");
                return false;
            }
        };

        if let Some(client) = self.clients.get(&client_id) {
            if client.writer.control.send(serialized).is_err() {
                debug!(target: "bus::server::main_loop",
                    client_id,
                    "client writer channel closed during targeted send"
                );
                self.remove_client_and_resize_if_needed(client_id);
                return false;
            }
            true
        } else {
            false
        }
    }
}
