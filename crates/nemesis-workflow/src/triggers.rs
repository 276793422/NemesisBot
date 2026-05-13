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
mod tests {
    use super::*;

    fn make_trigger(trigger_type: &str, config: HashMap<&str, &str>) -> TriggerConfig {
        let mut c = HashMap::new();
        for (k, v) in config {
            c.insert(k.to_string(), serde_json::json!(v));
        }
        TriggerConfig {
            trigger_type: trigger_type.to_string(),
            config: c,
        }
    }

    #[test]
    fn test_register_cron_trigger() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("cron", HashMap::from([("expression", "0 * * * *")]));
        mgr.register_trigger("test_wf", trigger).unwrap();

        let cron = mgr.get_cron_workflows();
        assert!(cron.contains_key("test_wf"));
        assert_eq!(cron["test_wf"], vec!["0 * * * *"]);
    }

    #[test]
    fn test_register_unknown_trigger_type() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("unknown", HashMap::new());
        let result = mgr.register_trigger("test_wf", trigger);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_trigger() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("webhook", HashMap::new());
        mgr.register_trigger("test_wf", trigger).unwrap();
        mgr.remove_trigger("test_wf");

        assert!(mgr.list_triggers("test_wf").is_empty());
        assert!(mgr.get_cron_workflows().is_empty());
    }

    #[test]
    fn test_match_event() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
        mgr.register_trigger("file_processor", trigger).unwrap();

        let mut data = HashMap::new();
        data.insert("type".to_string(), serde_json::json!("file_created"));

        let matched = mgr.match_event("event", &data);
        assert_eq!(matched, vec!["file_processor"]);
    }

    #[test]
    fn test_match_event_no_match() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
        mgr.register_trigger("file_processor", trigger).unwrap();

        let mut data = HashMap::new();
        data.insert("type".to_string(), serde_json::json!("file_deleted"));

        let matched = mgr.match_event("event", &data);
        assert!(matched.is_empty());
    }

    #[test]
    fn test_match_event_no_filter() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("event", HashMap::new());
        mgr.register_trigger("catch_all", trigger).unwrap();

        let data = HashMap::new();
        let matched = mgr.match_event("event", &data);
        assert_eq!(matched, vec!["catch_all"]);
    }

    #[test]
    fn test_get_webhook_workflows() {
        let mgr = TriggerManager::new();
        mgr.register_trigger("wf1", make_trigger("webhook", HashMap::new()))
            .unwrap();
        mgr.register_trigger("wf2", make_trigger("cron", HashMap::new()))
            .unwrap();
        mgr.register_trigger("wf3", make_trigger("webhook", HashMap::new()))
            .unwrap();

        let mut webhooks = mgr.get_webhook_workflows();
        webhooks.sort();
        assert_eq!(webhooks, vec!["wf1", "wf3"]);
    }

    #[test]
    fn test_list_all_triggers() {
        let mgr = TriggerManager::new();
        mgr.register_trigger("wf1", make_trigger("cron", HashMap::new()))
            .unwrap();
        mgr.register_trigger("wf2", make_trigger("webhook", HashMap::new()))
            .unwrap();

        let all = mgr.list_all_triggers();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_glob_matching() {
        assert!(match_glob("foo*", "foobar"));
        assert!(match_glob("*bar", "foobar"));
        assert!(match_glob("foo*bar", "fooXbar"));
        assert!(!match_glob("foo*bar", "bazbar"));
        assert!(match_glob("exact", "exact"));
        assert!(!match_glob("exact", "other"));
    }

    // ============================================================
    // Additional trigger tests: serialization, edge cases
    // ============================================================

    #[test]
    fn test_trigger_config_serialization() {
        let config = TriggerConfig {
            trigger_type: "cron".to_string(),
            config: {
                let mut m = HashMap::new();
                m.insert("expression".to_string(), serde_json::json!("0 * * * *"));
                m
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: TriggerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.trigger_type, "cron");
    }

    #[test]
    fn test_trigger_manager_default() {
        let mgr = TriggerManager::default();
        assert!(mgr.list_all_triggers().is_empty());
    }

    #[test]
    fn test_glob_matching_empty_pattern() {
        assert!(match_glob("", ""));
        assert!(!match_glob("", "something"));
    }

    #[test]
    fn test_glob_matching_star_only() {
        assert!(match_glob("*", "anything"));
        assert!(match_glob("*", ""));
    }

    #[test]
    fn test_glob_matching_multiple_stars() {
        assert!(match_glob("a*b*c", "aXbYc"));
        assert!(!match_glob("a*b*c", "aXbYd"));
    }

    #[test]
    fn test_value_to_string() {
        assert_eq!(value_to_string(&serde_json::json!("hello")), "hello");
        assert_eq!(value_to_string(&serde_json::json!(42)), "42");
        assert_eq!(value_to_string(&serde_json::json!(true)), "true");
        assert_eq!(value_to_string(&serde_json::json!(null)), "null");
    }

    #[test]
    fn test_register_multiple_triggers_same_workflow() {
        let mgr = TriggerManager::new();
        mgr.register_trigger("wf1", make_trigger("cron", HashMap::from([("expression", "0 * * * *")])))
            .unwrap();
        // Re-registering should update
        mgr.register_trigger("wf1", make_trigger("cron", HashMap::from([("expression", "0 0 * * *")])))
            .unwrap();
        let cron = mgr.get_cron_workflows();
        assert!(cron.contains_key("wf1"));
    }

    #[test]
    fn test_remove_nonexistent_trigger() {
        let mgr = TriggerManager::new();
        // Should not panic
        mgr.remove_trigger("nonexistent");
    }

    #[test]
    fn test_list_triggers_for_specific_workflow() {
        let mgr = TriggerManager::new();
        mgr.register_trigger("wf1", make_trigger("cron", HashMap::new()))
            .unwrap();
        mgr.register_trigger("wf2", make_trigger("webhook", HashMap::new()))
            .unwrap();

        let wf1_triggers = mgr.list_triggers("wf1");
        assert_eq!(wf1_triggers.len(), 1);
        let wf2_triggers = mgr.list_triggers("wf2");
        assert_eq!(wf2_triggers.len(), 1);
        let wf3_triggers = mgr.list_triggers("wf3");
        assert!(wf3_triggers.is_empty());
    }

    #[test]
    fn test_match_event_with_glob_filter() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("event", HashMap::from([("type", "file_*")]));
        mgr.register_trigger("glob_processor", trigger).unwrap();

        let mut data = HashMap::new();
        data.insert("type".to_string(), serde_json::json!("file_created"));
        let matched = mgr.match_event("event", &data);
        assert_eq!(matched, vec!["glob_processor"]);
    }

    #[test]
    fn test_match_event_wrong_channel() {
        let mgr = TriggerManager::new();
        let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
        mgr.register_trigger("file_processor", trigger).unwrap();

        let mut data = HashMap::new();
        data.insert("type".to_string(), serde_json::json!("file_created"));
        // Matching against a different channel should not match
        let matched = mgr.match_event("webhook", &data);
        assert!(matched.is_empty());
    }
}
