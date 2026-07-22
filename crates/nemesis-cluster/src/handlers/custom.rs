//! Custom action handler - routes user-defined actions.
//!
//! Allows registering custom action handlers at runtime for extensibility.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// A custom action handler function.
pub type CustomActionFn =
    Arc<dyn Fn(&str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;

/// Handler for custom (user-defined) cluster actions.
pub struct CustomHandler {
    handlers: Mutex<HashMap<String, CustomActionFn>>,
}

impl CustomHandler {
    /// Create a new custom handler registry.
    pub fn new() -> Self {
        Self {
            handlers: Mutex::new(HashMap::new()),
        }
    }

    /// Register a custom action handler.
    pub fn register(&self, action: &str, handler: CustomActionFn) {
        self.handlers.lock().insert(action.to_string(), handler);
    }

    /// Unregister a custom action handler.
    pub fn unregister(&self, action: &str) -> bool {
        self.handlers.lock().remove(action).is_some()
    }

    /// Check if a handler is registered for the given action.
    pub fn has_handler(&self, action: &str) -> bool {
        self.handlers.lock().contains_key(action)
    }

    /// Execute a custom action.
    pub fn execute(
        &self,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let handlers = self.handlers.lock();
        match handlers.get(action) {
            Some(handler) => handler(action, payload),
            None => Err(format!("No handler registered for action: {}", action)),
        }
    }

    /// List all registered custom action names.
    pub fn list_actions(&self) -> Vec<String> {
        self.handlers.lock().keys().cloned().collect()
    }
}

impl Default for CustomHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
