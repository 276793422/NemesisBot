//! Agent registry: manages multiple agent instances and routes messages to them.
//!
//! `AgentRegistry` provides a concurrent-safe store of named agents and supports
//! basic routing, default agent lookup, and sub-agent spawn permission checks.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::instance::AgentInstance;
use crate::types::AgentConfig;

/// Manages multiple agent instances with concurrent access.
pub struct AgentRegistry {
    agents: RwLock<HashMap<String, AgentInstance>>,
    /// Optional mapping of agent ID to sub-agent allow lists.
    subagent_allow: RwLock<HashMap<String, Vec<String>>>,
}

impl AgentRegistry {
    /// Create a new empty agent registry.
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            subagent_allow: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with a single default "main" agent.
    pub fn with_default(config: AgentConfig) -> Self {
        let instance = AgentInstance::new(config);
        let mut agents = HashMap::new();
        agents.insert("main".to_string(), instance);
        Self {
            agents: RwLock::new(agents),
            subagent_allow: RwLock::new(HashMap::new()),
        }
    }

    /// Register an agent instance with the given ID.
    pub fn register(&self, id: String, instance: AgentInstance) {
        let normalized = normalize_agent_id(&id);
        self.agents.write().unwrap().insert(normalized, instance);
    }

    /// Get a clone of the agent instance for a given ID.
    ///
    /// Since `AgentInstance` uses interior mutability (Mutex-based),
    /// cloning is not needed; instead we return a reference-like result.
    /// However, because `AgentInstance` is not `Clone` (it contains non-cloneable fields),
    /// we use a different approach: return a boolean indicating existence.
    ///
    /// For thread-safe access, callers should use `with_agent` instead.
    pub fn contains_agent(&self, agent_id: &str) -> bool {
        let agents = self.agents.read().unwrap();
        let id = normalize_agent_id(agent_id);
        agents.contains_key(&id)
    }

    /// Execute a closure with a reference to the agent, if it exists.
    ///
    /// Returns `Some(result)` if the agent was found, `None` otherwise.
    pub fn with_agent<F, R>(&self, agent_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&AgentInstance) -> R,
    {
        let agents = self.agents.read().unwrap();
        let id = normalize_agent_id(agent_id);
        agents.get(&id).map(f)
    }

    /// Execute a mutable closure with the agent, if it exists.
    pub fn with_agent_mut<F, R>(&self, agent_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&AgentInstance) -> R,
    {
        let agents = self.agents.read().unwrap();
        let id = normalize_agent_id(agent_id);
        agents.get(&id).map(f)
    }

    /// Get the ID of the default agent ("main" if it exists, otherwise the first registered).
    pub fn default_agent_id(&self) -> Option<String> {
        let agents = self.agents.read().unwrap();
        if agents.contains_key("main") {
            return Some("main".to_string());
        }
        agents.keys().next().cloned()
    }

    /// List all registered agent IDs.
    pub fn list_agent_ids(&self) -> Vec<String> {
        let agents = self.agents.read().unwrap();
        agents.keys().cloned().collect()
    }

    /// Returns the number of registered agents.
    pub fn len(&self) -> usize {
        self.agents.read().unwrap().len()
    }

    /// Returns true if no agents are registered.
    pub fn is_empty(&self) -> bool {
        self.agents.read().unwrap().is_empty()
    }

    /// Set the sub-agent allow list for a parent agent.
    ///
    /// The allow list can contain specific agent IDs or "*" to allow all.
    pub fn set_subagent_allow(&self, parent_id: &str, allowed: Vec<String>) {
        let id = normalize_agent_id(parent_id);
        self.subagent_allow
            .write()
            .unwrap()
            .insert(id, allowed);
    }

    /// Check if a parent agent is allowed to spawn a target agent.
    pub fn can_spawn_subagent(&self, parent_id: &str, target_id: &str) -> bool {
        let allow_map = self.subagent_allow.read().unwrap();
        let parent_key = normalize_agent_id(parent_id);

        match allow_map.get(&parent_key) {
            Some(allowed) => {
                let target_norm = normalize_agent_id(target_id);
                for a in allowed {
                    if a == "*" {
                        return true;
                    }
                    if normalize_agent_id(a) == target_norm {
                        return true;
                    }
                }
                false
            }
            None => false,
        }
    }

    /// Remove an agent from the registry.
    pub fn remove(&self, agent_id: &str) -> bool {
        let id = normalize_agent_id(agent_id);
        self.agents.write().unwrap().remove(&id).is_some()
    }
}

/// Normalize an agent ID to lowercase for consistent lookups.
fn normalize_agent_id(id: &str) -> String {
    id.to_lowercase()
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
