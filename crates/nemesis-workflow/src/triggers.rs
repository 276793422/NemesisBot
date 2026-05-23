//! Trigger manager for workflow automation.
//!
//! Manages workflow triggers including cron schedules, webhook endpoints,
//! event matching, and message patterns. Mirrors the Go `triggers.go`.

use std::collections::HashMap;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// Configuration for a single trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Trigger type: "cron", "webhook", "event", "message".
    pub trigger_type: String,
    /// Additional configuration for the trigger.
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Manager for workflow triggers.
///
/// Supports cron schedules, webhook endpoints, event matching, and message
/// patterns. Thread-safe via RwLock.
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
                ))
            }
        }

        // Track cron jobs separately
        if trigger.trigger_type == "cron" {
            if let Some(expr_val) = trigger.config.get("expression") {
                if let Some(expr) = expr_val.as_str() {
                    let mut cron = self.cron_jobs.write().unwrap();
                    cron.entry(workflow_name.to_string())
                        .or_default()
                        .push(expr.to_string());
                }
            }
        }

        let mut triggers = self.triggers.write().unwrap();
        triggers
            .entry(workflow_name.to_string())
            .or_default()
            .push(trigger);

        Ok(())
    }

    /// Remove all triggers for a workflow.
    pub fn remove_trigger(&self, workflow_name: &str) {
        self.triggers.write().unwrap().remove(workflow_name);
        self.cron_jobs.write().unwrap().remove(workflow_name);
    }

    /// Return workflow names that should be triggered by an event.
    ///
    /// The event type is matched against trigger types, and the data map
    /// is used for additional matching criteria.
    pub fn match_event(
        &self,
        event_type: &str,
        data: &HashMap<String, serde_json::Value>,
    ) -> Vec<String> {
        let triggers = self.triggers.read().unwrap();
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

    /// Return all workflow names that have cron triggers.
    pub fn get_cron_workflows(&self) -> HashMap<String, Vec<String>> {
        self.cron_jobs.read().unwrap().clone()
    }

    /// Return all workflow names that have webhook triggers.
    pub fn get_webhook_workflows(&self) -> Vec<String> {
        let triggers = self.triggers.read().unwrap();
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
            .unwrap()
            .get(workflow_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Return all registered triggers across all workflows.
    pub fn list_all_triggers(&self) -> HashMap<String, Vec<TriggerConfig>> {
        self.triggers.read().unwrap().clone()
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
