//! Trigger manager for workflow automation.
//!
//! Manages workflow triggers including cron schedules, webhook endpoints,
//! event matching, and message patterns. Mirrors the Go `triggers.go`.

use std::collections::HashMap;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Borrowed view of an inbound message — used by [`TriggerManager::match_message`]
/// so this crate doesn't need to depend on `nemesis-types`. The gateway wires
/// real `nemesis_types::InboundMessage` into this shape at the call site.
#[derive(Debug, Clone, Copy)]
pub struct InboundMessageRef<'a> {
    pub channel: &'a str,
    pub sender_id: &'a str,
    pub chat_id: &'a str,
    pub content: &'a str,
}

/// Configuration for a single trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Trigger type: "cron", "webhook", "event", "message".
    pub trigger_type: String,
    /// Additional configuration for the trigger.
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Timezone a cron trigger evaluates its expression in (milestone 1b-C4).
///
/// Most users write cron expressions thinking of wall-clock local time
/// ("0 9 * * * *" = "9am my time"), so [`CronTimezone::Local`] is the
/// default. Workloads that need to fire simultaneously across regions
/// (e.g. "midnight UTC everywhere") opt in via `"timezone": "utc"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronTimezone {
    /// Evaluate against the host's local timezone (default).
    Local,
    /// Evaluate against UTC.
    Utc,
}

impl CronTimezone {
    /// Parse a config string into a [`CronTimezone`]. Accepts
    /// case-insensitive `"local"` / `"utc"`. Returns `None` for anything
    /// else so the caller can warn + fall back to local.
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "utc" => Some(Self::Utc),
            _ => None,
        }
    }

    /// Human-readable label for logging.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Utc => "utc",
        }
    }
}

/// Manager for workflow triggers.
///
/// Supports cron schedules, webhook endpoints, event matching, and message
/// patterns. Thread-safe via RwLock (parking_lot — no poisoning if a node
/// executor panics while a guard is held).
pub struct TriggerManager {
    /// Maps workflow names to their trigger configs.
    triggers: RwLock<HashMap<String, Vec<TriggerConfig>>>,
    /// Maps workflow names to cron expressions (subset for quick lookup).
    cron_jobs: RwLock<HashMap<String, Vec<String>>>,
}

impl TriggerManager {
    /// Create a new trigger manager.
    pub fn new() -> Self {
        Self {
            triggers: RwLock::new(HashMap::new()),
            cron_jobs: RwLock::new(HashMap::new()),
        }
    }

    /// Register a trigger for a workflow.
    ///
    /// Returns an error if the trigger type is unknown.
    pub fn register_trigger(
        &self,
        workflow_name: &str,
        trigger: TriggerConfig,
    ) -> Result<(), String> {
        match trigger.trigger_type.as_str() {
            "cron" | "webhook" | "event" | "message" => {}
            other => {
                return Err(format!(
                    "unknown trigger type {:?} for workflow {:?}",
                    other, workflow_name
                ));
            }
        }

        // Track cron jobs separately. Field name is `schedule` to match
        // `engine.rs` (cron_next_fire_at_from_trigger / spawn_cron_triggers /
        // list_cron_workflows). Older code read `expression`; that name was
        // never honoured by the actual scheduler, so writing `expression` in
        // YAML silently failed to schedule. Accept both keys here for
        // backward compat, but new YAML should use `schedule`.
        if trigger.trigger_type == "cron" {
            let expr_val = trigger
                .config
                .get("schedule")
                .or_else(|| trigger.config.get("expression"));
            if let Some(v) = expr_val.and_then(|v| v.as_str()) {
                let mut cron = self.cron_jobs.write();
                cron.entry(workflow_name.to_string())
                    .or_default()
                    .push(v.to_string());
            }
        }

        let mut triggers = self.triggers.write();
        triggers
            .entry(workflow_name.to_string())
            .or_default()
            .push(trigger);

        Ok(())
    }

    /// Remove all triggers for a workflow.
    pub fn remove_trigger(&self, workflow_name: &str) {
        self.triggers.write().remove(workflow_name);
        self.cron_jobs.write().remove(workflow_name);
    }

    /// Return workflow names that should be triggered by an event.
    ///
    /// The event type is matched against trigger types, and the data map
    /// is used for additional matching criteria.
    ///
    /// **Historical note**: this method predates the proper `match_trigger_event`
    /// helper below. Its semantics are unusual — it matches `trigger_type`
    /// against the supplied `event_type` string rather than filtering by
    /// `trigger_type == "event"` and then matching `config.event_type`. New
    /// callers should use [`Self::match_trigger_event`] instead.
    pub fn match_event(
        &self,
        event_type: &str,
        data: &HashMap<String, serde_json::Value>,
    ) -> Vec<String> {
        let triggers = self.triggers.read();
        let mut matched = Vec::new();

        for (wf_name, wf_triggers) in triggers.iter() {
            for trigger in wf_triggers {
                if trigger.trigger_type != event_type {
                    continue;
                }

                if self.match_trigger_data(trigger, data) {
                    matched.push(wf_name.clone());
                    break; // one match per workflow is enough
                }
            }
        }

        matched
    }

    /// Match a typed [`TriggerEvent`] against all registered `event` triggers.
    ///
    /// This is the *correct* event-matching path: filters by
    /// `trigger_type == "event"`, then matches `config.event_type` against the
    /// event's `event_type` field (glob allowed, e.g. `"workflow.*"`), then
    /// matches any remaining config keys against `event.data`.
    ///
    /// Returns workflow names with at least one matching trigger.
    pub fn match_trigger_event(
        &self,
        event: &crate::event_dispatcher::TriggerEvent,
    ) -> Vec<String> {
        let triggers = self.triggers.read();
        let mut matched = Vec::new();

        for (wf_name, wf_triggers) in triggers.iter() {
            for trigger in wf_triggers {
                if trigger.trigger_type != "event" {
                    continue;
                }

                // First key to match is event_type itself.
                let expected_event_type =
                    match trigger.config.get("event_type").and_then(|v| v.as_str()) {
                        Some(pattern) => pattern,
                        None => continue, // event trigger must declare event_type
                    };
                if !match_glob(expected_event_type, &event.event_type) {
                    continue;
                }

                // Remaining config keys match against event.data.
                if self.match_event_data(trigger, &event.data) {
                    matched.push(wf_name.clone());
                    break;
                }
            }
        }

        matched
    }

    /// Match an inbound message against all registered `message` triggers.
    ///
    /// A `message` trigger config supports these keys (all optional except
    /// matching is empty-match-all by default):
    ///   - `channel`: glob match against the channel name (e.g. `"web"`, `"telegram"`, `"*"`)
    ///   - `content`: glob match against the message text (e.g. `"*hello*"`, `"/cmd *"`)
    ///   - `sender_id`: glob match against the sender id
    ///   - `chat_id`: glob match against the chat id
    ///
    /// Returns workflow names with at least one matching trigger.
    pub fn match_message(&self, msg: &InboundMessageRef<'_>) -> Vec<String> {
        let triggers = self.triggers.read();
        let mut matched = Vec::new();

        for (wf_name, wf_triggers) in triggers.iter() {
            for trigger in wf_triggers {
                if trigger.trigger_type != "message" {
                    continue;
                }

                if !self.match_message_data(trigger, msg) {
                    continue;
                }

                matched.push(wf_name.clone());
                break;
            }
        }

        matched
    }

    /// Check if the event data matches the trigger's config criteria,
    /// **ignoring** the `event_type` key (which is matched separately by
    /// `match_trigger_event`). All other keys must match (glob allowed).
    fn match_event_data(
        &self,
        trigger: &TriggerConfig,
        data: &HashMap<String, serde_json::Value>,
    ) -> bool {
        for (key, expected) in &trigger.config {
            if key == "event_type" {
                continue; // already matched
            }
            let actual = match data.get(key) {
                Some(v) => v,
                None => return false,
            };
            let expected_str = value_to_string(expected);
            let actual_str = value_to_string(actual);
            if expected_str.contains('*') {
                if !match_glob(&expected_str, &actual_str) {
                    return false;
                }
            } else if expected_str != actual_str {
                return false;
            }
        }
        true
    }

    /// Check if an inbound message matches the trigger's config criteria.
    fn match_message_data(&self, trigger: &TriggerConfig, msg: &InboundMessageRef<'_>) -> bool {
        for (key, expected) in &trigger.config {
            let expected_str = value_to_string(expected);
            let actual_str = match key.as_str() {
                "channel" => msg.channel.to_string(),
                "content" => msg.content.to_string(),
                "sender_id" => msg.sender_id.to_string(),
                "chat_id" => msg.chat_id.to_string(),
                _ => continue, // unknown keys ignored — be permissive
            };
            if expected_str.contains('*') {
                if !match_glob(&expected_str, &actual_str) {
                    return false;
                }
            } else if expected_str != actual_str {
                return false;
            }
        }
        true
    }

    /// Return all workflow names that have cron triggers.
    pub fn get_cron_workflows(&self) -> HashMap<String, Vec<String>> {
        self.cron_jobs.read().clone()
    }

    /// Return all workflow names that have webhook triggers.
    pub fn get_webhook_workflows(&self) -> Vec<String> {
        let triggers = self.triggers.read();
        let mut names = Vec::new();

        for (wf_name, wf_triggers) in triggers.iter() {
            for t in wf_triggers {
                if t.trigger_type == "webhook" {
                    names.push(wf_name.clone());
                    break;
                }
            }
        }

        names
    }

    /// Register all triggers defined in a Workflow.
    ///
    /// Iterates over the workflow's trigger configs and registers each one.
    /// Returns the first error encountered, if any.
    pub fn register_workflow_triggers(
        &self,
        workflow_name: &str,
        triggers: &[crate::types::TriggerConfig],
    ) -> Result<(), String> {
        for trigger in triggers {
            let t = TriggerConfig {
                trigger_type: trigger.trigger_type.clone(),
                config: trigger.config.clone(),
            };
            self.register_trigger(workflow_name, t)?;
        }
        Ok(())
    }

    /// Return all triggers for a specific workflow.
    pub fn list_triggers(&self, workflow_name: &str) -> Vec<TriggerConfig> {
        self.triggers
            .read()
            .get(workflow_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Return all registered triggers across all workflows.
    pub fn list_all_triggers(&self) -> HashMap<String, Vec<TriggerConfig>> {
        self.triggers.read().clone()
    }

    /// Check if the event data matches the trigger's config criteria.
    fn match_trigger_data(
        &self,
        trigger: &TriggerConfig,
        data: &HashMap<String, serde_json::Value>,
    ) -> bool {
        if trigger.config.is_empty() {
            return true; // no filter means match all
        }

        for (key, expected) in &trigger.config {
            let actual = match data.get(key) {
                Some(v) => v,
                None => return false,
            };

            let expected_str = value_to_string(expected);
            let actual_str = value_to_string(actual);

            if expected_str.contains('*') {
                if !match_glob(&expected_str, &actual_str) {
                    return false;
                }
            } else if expected_str != actual_str {
                return false;
            }
        }

        true
    }
}

impl Default for TriggerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a JSON value to a string for comparison.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Simple glob matching with "*" wildcard.
fn match_glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == s;
    }

    // Must start with first part
    if !s.starts_with(parts[0]) {
        return false;
    }

    // Must end with last part
    if !s.ends_with(parts[parts.len() - 1]) {
        return false;
    }

    // Check middle parts appear in order
    let mut idx = parts[0].len();
    for i in 1..parts.len() - 1 {
        match s[idx..].find(parts[i]) {
            Some(pos) => idx += pos + parts[i].len(),
            None => return false,
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
