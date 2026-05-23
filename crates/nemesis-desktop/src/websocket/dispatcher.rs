//! Message dispatcher - Routes JSON-RPC messages to registered handlers.
//!
//! Supports request handlers, notification handlers, and fallback handlers.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::websocket::protocol::Message;

/// Handler for request messages (returns a response).
pub type HandlerFunc = Box<dyn Fn(&Message) -> Result<Message, String> + Send + Sync>;

/// Handler for notification messages (no return value).
pub type NotificationFunc = Box<dyn Fn(&Message) + Send + Sync>;

/// Message dispatcher that routes JSON-RPC messages to registered handlers.
pub struct Dispatcher {
    /// Request handlers keyed by method name.
    handlers: RwLock<HashMap<String, Box<dyn Fn(&Message) -> Result<Message, String> + Send + Sync>>>,
    /// Notification handlers keyed by method name.
    notif_handlers: RwLock<HashMap<String, Box<dyn Fn(&Message) + Send + Sync>>>,
    /// Fallback handler for unknown methods.
    fallback: RwLock<Option<Box<dyn Fn(&Message) -> Result<Message, String> + Send + Sync>>>,
}

impl Dispatcher {
    /// Create a new dispatcher.
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            notif_handlers: RwLock::new(HashMap::new()),
            fallback: RwLock::new(None),
        }
    }

    /// Register a handler for a request method.
    pub fn register<F>(&self, method: &str, handler: F)
    where
        F: Fn(&Message) -> Result<Message, String> + Send + Sync + 'static,
    {
        self.handlers
            .write()
            .unwrap()
            .insert(method.to_string(), Box::new(handler));
    }

    /// Register a handler for a notification method.
    pub fn register_notification<F>(&self, method: &str, handler: F)
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.notif_handlers
            .write()
            .unwrap()
            .insert(method.to_string(), Box::new(handler));
    }

    /// Set a fallback handler for unknown request methods.
    pub fn set_fallback<F>(&self, handler: F)
    where
        F: Fn(&Message) -> Result<Message, String> + Send + Sync + 'static,
    {
        *self.fallback.write().unwrap() = Some(Box::new(handler));
    }

    /// Dispatch a message to the appropriate handler.
    ///
    /// For requests, returns the response message.
    /// For notifications, returns None.
    pub fn dispatch(&self, msg: &Message) -> Result<Option<Message>, String> {
        if msg.is_request() {
            let method = msg.method.as_deref().unwrap_or("");

            // Try registered handlers
            {
                let handlers = self.handlers.read().unwrap();
                if let Some(handler) = handlers.get(method) {
                    let resp = handler(msg)?;
                    return Ok(Some(resp));
                }
            }

            // Try fallback
            {
                let fallback = self.fallback.read().unwrap();
                if let Some(ref handler) = *fallback {
                    let resp = handler(msg)?;
                    return Ok(Some(resp));
                }
            }

            // Method not found
            let id = msg.id.as_deref().unwrap_or("");
            let resp = Message::new_error_response(
                id,
                crate::websocket::protocol::ERR_METHOD_NOT_FOUND,
                &format!("method not found: {}", method),
                None,
            );
            Ok(Some(resp))
        } else if msg.is_notification() {
            let method = msg.method.as_deref().unwrap_or("");
            let handlers = self.notif_handlers.read().unwrap();
            if let Some(handler) = handlers.get(method) {
                handler(msg);
            }
            Ok(None)
        } else {
            Err("message is neither request nor notification".to_string())
        }
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
