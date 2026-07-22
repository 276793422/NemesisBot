//! Trigger driver status — the single source of truth for which trigger
//! types currently have a runtime driver wired up in the gateway.
//!
//! This exists so the UI never has to hardcode "event/message triggers are
//! not driven" — instead it queries the backend and renders whatever the
//! backend says. When [`crate::triggers::TriggerManager::match_event`] gets
//! a real event-bus subscription in the external-integration phase, only
//! the matches in this file need to flip from `undriven` to `driven`.

use serde::{Deserialize, Serialize};

/// Status of a single trigger type's runtime driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerDriverStatus {
    pub trigger_type: String,
    pub driven: bool,
    /// Why the trigger is not driven. Absent when `driven == true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl TriggerDriverStatus {
    fn driven(trigger_type: &str) -> Self {
        Self {
            trigger_type: trigger_type.to_string(),
            driven: true,
            reason: None,
        }
    }

    fn undriven(trigger_type: &str, reason: impl Into<String>) -> Self {
        Self {
            trigger_type: trigger_type.to_string(),
            driven: false,
            reason: Some(reason.into()),
        }
    }
}

/// Query the driver status for a trigger type. Unknown types return
/// `undriven` so typos surface in the UI rather than silently doing
/// nothing.
pub fn driver_status_for(trigger_type: &str) -> TriggerDriverStatus {
    match trigger_type {
        "cron" => TriggerDriverStatus::driven("cron"),
        "webhook" => TriggerDriverStatus::driven("webhook"),
        "event" => TriggerDriverStatus::driven("event"),
        "message" => TriggerDriverStatus::driven("message"),
        other => TriggerDriverStatus::undriven(other, format!("unknown trigger type: {}", other)),
    }
}

/// All known trigger types and their driver status. Useful for returning
/// a global capability declaration to the UI (so it can render a "trigger
/// picker" without hardcoding anything).
pub fn all_known_trigger_types() -> Vec<&'static str> {
    vec!["cron", "webhook", "event", "message"]
}

/// Build a map of `trigger_type -> TriggerDriverStatus` for every known
/// trigger type. The UI uses this as a global capability declaration.
pub fn all_driver_statuses() -> std::collections::HashMap<String, TriggerDriverStatus> {
    let mut m = std::collections::HashMap::new();
    for t in all_known_trigger_types() {
        let s = driver_status_for(t);
        m.insert(t.to_string(), s);
    }
    m
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
