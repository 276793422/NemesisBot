//! WebSocket client - Connects child processes to the parent server.
//!
//! Handles authentication, message routing, request-response correlation,
//! and notification handling from the child side. Uses tokio-tungstenite
//! for actual WebSocket connections.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

use crate::websocket::dispatcher::Dispatcher;
use crate::websocket::protocol::Message;

/// Type alias for the WebSocket stream used in read/write tasks.
type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Shared state for the WebSocket client that can be sent across tasks.
struct ClientState {
    /// Pending request-response channels, keyed by message ID.
    pending: HashMap<String, oneshot::Sender<Message>>,
    /// Connected flag.
    connected: bool,
    /// Channel to send outgoing messages to the write task.
    send_tx: Option<tokio::sync::mpsc::Sender<String>>,
    /// Shutdown signal sender.
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
}

/// WebSocket key received during handshake.
#[derive(Debug, Clone)]
pub struct WebSocketKey {
    pub key: String,
    pub port: u16,
    pub path: String,
}

/// WebSocket client for child process communication with the parent.
///
/// Connects to the parent's WebSocket server using the key received
/// during pipe handshake. Provides request-response correlation via
/// pending map and dispatches incoming messages to registered handlers.
pub struct WebSocketClient {
    /// Client ID.
    id: String,
    /// Authentication key.
    key: String,
    /// Server URL.
    server_url: String,
    /// Message dispatcher for incoming requests/notifications.
    /// Wrapped in Arc so it can be shared with the spawned read task.
    dispatcher: Arc<Dispatcher>,
    /// Shared mutable state behind parking_lot::Mutex (Send-safe).
    state: parking_lot::Mutex<ClientState>,
}

impl WebSocketClient {
    /// Create a new WebSocket client from the handshake key.
    pub fn new(ws_key: &WebSocketKey) -> Self {
        let server_url = format!("ws://127.0.0.1:{}{}", ws_key.port, ws_key.path);
        Self {
            id: ws_key.key.clone(),
            key: ws_key.key.clone(),
            server_url,
            dispatcher: Arc::new(Dispatcher::new()),
            state: parking_lot::Mutex::new(ClientState {
                pending: HashMap::new(),
                connected: false,
                send_tx: None,
                shutdown_tx: None,
            }),
        }
    }

    /// Connect to the parent server.
    ///
    /// Performs the following steps:
    /// 1. Establishes a WebSocket connection to the parent server
    /// 2. Sends an authentication message containing the key
    /// 3. Starts background read and write tasks
    pub async fn connect(&self) -> Result<(), String> {
        info!("[WebSocketClient] {}: Connecting to {}", self.id, self.server_url);

        // Establish WebSocket connection
        let (ws_stream, _response) = tokio_tungstenite::connect_async(&self.server_url)
            .await
            .map_err(|e| format!("WebSocket connect failed: {}", e))?;

        info!("[WebSocketClient] {}: TCP connected", self.id);

        // Send authentication message
        let auth_msg = serde_json::json!({
            "type": "auth",
            "key": self.key,
        });
        let auth_str = serde_json::to_string(&auth_msg)
            .map_err(|e| format!("serialize auth: {}", e))?;

        let (mut ws_write, ws_read) = ws_stream.split();
        ws_write
            .send(tokio_tungstenite::tungstenite::Message::Text(auth_str.into()))
            .await
            .map_err(|e| format!("send auth: {}", e))?;

        info!("[WebSocketClient] {}: Authenticated", self.id);

        // Create channels for communication
        let (msg_tx, msg_rx) = tokio::sync::mpsc::channel::<String>(64);
        let (shutdown_tx, shutdown_rx1) = tokio::sync::broadcast::channel::<()>(1);
        let shutdown_rx2 = shutdown_tx.subscribe();

        // Store channels in state
        {
            let mut state = self.state.lock();
            state.connected = true;
            state.send_tx = Some(msg_tx);
            state.shutdown_tx = Some(shutdown_tx);
        }

        // Spawn write task
        let write_id = self.id.clone();
        tokio::spawn(async move {
            Self::write_loop(msg_rx, ws_write, shutdown_rx1, &write_id).await;
        });

        // Spawn read task
        let read_id = self.id.clone();
        let pending_map: Arc<parking_lot::Mutex<HashMap<String, oneshot::Sender<Message>>>> =
            Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let dispatcher = self.dispatcher.clone();
        let send_tx = {
            let state = self.state.lock();
            state.send_tx.clone()
        };

        // Move existing pending entries to the shared map (oneshot::Sender is not Clone,
        // so we drain from state into the Arc map)
        {
            let mut state = self.state.lock();
            let mut pending = pending_map.lock();
            for (k, v) in state.pending.drain() {
                pending.insert(k, v);
            }
        }

        tokio::spawn(async move {
            Self::read_loop(
                ws_read,
                shutdown_rx2,
                &read_id,
                &pending_map,
                &dispatcher,
                &send_tx,
            )
            .await;
        });

        Ok(())
    }

    /// Background write loop - sends outgoing messages through the WebSocket.
    async fn write_loop(
        mut msg_rx: tokio::sync::mpsc::Receiver<String>,
        mut write: futures_util::stream::SplitSink<
            WsStream,
            tokio_tungstenite::tungstenite::Message,
        >,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
        id: &str,
    ) {
        loop {
            tokio::select! {
                msg = msg_rx.recv() => {
                    match msg {
                        Some(data) => {
                            if let Err(e) = write
                                .send(tokio_tungstenite::tungstenite::Message::Text(data.into()))
                                .await
                            {
                                warn!("[WebSocketClient] {}: Write error: {}", id, e);
                                break;
                            }
                        }
                        None => {
                            debug!("[WebSocketClient] {}: Send channel closed", id);
                            break;
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    debug!("[WebSocketClient] {}: Shutdown signal received", id);
                    break;
                }
            }
        }
        debug!("[WebSocketClient] {}: Write loop exiting", id);
    }

    /// Background read loop - receives incoming messages and routes them.
    ///
    /// Routes responses to pending channels, and dispatches incoming
    /// requests and notifications to the registered dispatcher handlers.
    async fn read_loop(
        mut read: futures_util::stream::SplitStream<WsStream>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
        id: &str,
        pending: &Arc<parking_lot::Mutex<HashMap<String, oneshot::Sender<Message>>>>,
        dispatcher: &Arc<Dispatcher>,
        send_tx: &Option<tokio::sync::mpsc::Sender<String>>,
    ) {
        loop {
            tokio::select! {
                msg = read.next() => {
                    match msg {
                        Some(Ok(ws_msg)) => {
                            match ws_msg {
                                tokio_tungstenite::tungstenite::Message::Text(text) => {
                                    Self::handle_message(&text, id, pending, dispatcher, send_tx);
                                }
                                tokio_tungstenite::tungstenite::Message::Close(_) => {
                                    info!("[WebSocketClient] {}: Close frame received", id);
                                    break;
                                }
                                _ => {
                                    // Ignore binary, ping, pong
                                }
                            }
                        }
                        Some(Err(e)) => {
                            warn!("[WebSocketClient] {}: Read error: {}", id, e);
                            break;
                        }
                        None => {
                            info!("[WebSocketClient] {}: Stream ended", id);
                            break;
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    debug!("[WebSocketClient] {}: Shutdown signal received", id);
                    break;
                }
            }
        }
        debug!("[WebSocketClient] {}: Read loop exiting", id);
    }

    /// Handle an incoming text message.
    ///
    /// For responses: routes to the pending channel for request-response
    /// correlation.
    /// For requests: dispatches to the registered handler and sends the
    /// response back through the WebSocket.
    /// For notifications: dispatches to the registered notification handler.
    fn handle_message(
        text: &str,
        id: &str,
        pending: &parking_lot::Mutex<HashMap<String, oneshot::Sender<Message>>>,
        dispatcher: &Dispatcher,
        send_tx: &Option<tokio::sync::mpsc::Sender<String>>,
    ) {
        let msg: Message = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                warn!("[WebSocketClient] {}: JSON decode error: {}", id, e);
                return;
            }
        };

        if msg.jsonrpc != crate::websocket::protocol::VERSION {
            debug!(
                "[WebSocketClient] {}: Non-protocol message ignored",
                id
            );
            return;
        }

        // Route response to pending channel
        if msg.is_response() {
            if let Some(msg_id) = msg.id.clone() {
                let mut pending_map = pending.lock();
                if let Some(tx) = pending_map.remove(&msg_id) {
                    if tx.send(msg).is_err() {
                        warn!(
                            "[WebSocketClient] {}: Pending channel dropped for id={}",
                            id, msg_id
                        );
                    }
                } else {
                    debug!(
                        "[WebSocketClient] {}: No pending request for id={}",
                        id, msg_id
                    );
                }
            }
            return;
        }

        // Dispatch incoming requests to registered handlers
        if msg.is_request() {
            match dispatcher.dispatch(&msg) {
                Ok(Some(resp_msg)) => {
                    if let Some(tx) = send_tx {
                        let resp_str =
                            serde_json::to_string(&resp_msg).unwrap_or_default();
                        if let Err(e) = tx.try_send(resp_str) {
                            warn!(
                                "[WebSocketClient] {}: Failed to send dispatch response: {}",
                                id, e
                            );
                        }
                    } else {
                        warn!(
                            "[WebSocketClient] {}: No send channel for dispatch response",
                            id
                        );
                    }
                }
                Ok(None) => {
                    // Handler returned no response (should not happen for requests)
                }
                Err(e) => {
                    warn!(
                        "[WebSocketClient] {}: Dispatch error for request: {}",
                        id, e
                    );
                }
            }
            return;
        }

        // Dispatch incoming notifications to registered handlers
        if msg.is_notification() {
            if let Err(e) = dispatcher.dispatch(&msg) {
                warn!(
                    "[WebSocketClient] {}: Dispatch error for notification: {}",
                    id, e
                );
            }
        }
    }

    /// Send a raw JSON-RPC message to the server.
    fn send_raw(&self, msg: &Message) -> Result<(), String> {
        let state = self.state.lock();
        if !state.connected {
            return Err("not connected".to_string());
        }

        let data = serde_json::to_string(msg).map_err(|e| format!("serialize: {}", e))?;

        if let Some(ref tx) = state.send_tx {
            tx.try_send(data)
                .map_err(|e| format!("send failed: {}", e))?;
            Ok(())
        } else {
            Err("no send channel".to_string())
        }
    }

    /// Send a notification to the parent (no response expected).
    pub fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), String> {
        {
            let state = self.state.lock();
            if !state.connected {
                return Err("not connected".to_string());
            }
        }

        let msg = Message::new_notification(method, params);
        self.send_raw(&msg)
    }

    /// Send a request to the parent and wait for a response.
    ///
    /// Creates a pending channel keyed by message ID, sends the request,
    /// and awaits the response with a 30-second timeout.
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Message, String> {
        {
            let state = self.state.lock();
            if !state.connected {
                return Err("not connected".to_string());
            }
        }

        let msg = Message::new_request(method, params);
        let msg_id = msg.id.clone().unwrap_or_default();

        // Send the request first
        if let Err(e) = self.send_raw(&msg) {
            return Err(format!("send request: {}", e));
        }

        // Create a pending channel
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.state.lock();
            state.pending.insert(msg_id.clone(), tx);
        }

        // Wait for response with timeout
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx).await;

        // Clean up pending
        {
            let mut state = self.state.lock();
            state.pending.remove(&msg_id);
        }

        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err("response channel dropped".to_string()),
            Err(_) => Err("call timeout (30s)".to_string()),
        }
    }

    /// Register a request handler.
    pub fn register_handler<F>(&self, method: &str, handler: F)
    where
        F: Fn(&Message) -> Result<Message, String> + Send + Sync + 'static,
    {
        self.dispatcher.register(method, handler);
    }

    /// Register a notification handler.
    pub fn register_notification_handler<F>(&self, method: &str, handler: F)
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.dispatcher.register_notification(method, handler);
    }

    /// Set a fallback handler for unknown methods.
    pub fn set_fallback<F>(&self, handler: F)
    where
        F: Fn(&Message) -> Result<Message, String> + Send + Sync + 'static,
    {
        self.dispatcher.set_fallback(handler);
    }

    /// Close the connection.
    pub fn close(&self) {
        info!("[WebSocketClient] {}: Closing", self.id);

        let mut state = self.state.lock();

        // Send shutdown signal to read/write tasks
        if let Some(ref tx) = state.shutdown_tx {
            let _ = tx.send(());
        }

        state.connected = false;
        state.send_tx = None;
        state.pending.clear();
    }

    /// Check if connected.
    pub fn is_connected(&self) -> bool {
        self.state.lock().connected
    }

    /// Get the client ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the server URL.
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// Get a reference to the dispatcher (for advanced use).
    pub fn dispatcher(&self) -> &Dispatcher {
        &self.dispatcher
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
#[cfg(test)]
mod extra_tests;
