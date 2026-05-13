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
mod tests {
    use super::*;

    fn test_config(name: &str) -> AgentConfig {
        AgentConfig {
            model: format!("test-model-{}", name),
            system_prompt: Some(format!("You are {}.", name)),
            max_turns: 5,
            tools: vec![],
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = AgentRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn with_default_creates_main_agent() {
        let registry = AgentRegistry::with_default(test_config("main"));
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
        assert!(registry.contains_agent("main"));
        assert_eq!(registry.default_agent_id(), Some("main".to_string()));
    }

    #[test]
    fn register_and_lookup() {
        let registry = AgentRegistry::new();
        registry.register("agent_a".to_string(), AgentInstance::new(test_config("a")));
        registry.register("agent_b".to_string(), AgentInstance::new(test_config("b")));

        assert_eq!(registry.len(), 2);
        assert!(registry.contains_agent("agent_a"));
        assert!(registry.contains_agent("agent_b"));
        assert!(!registry.contains_agent("agent_c"));
    }

    #[test]
    fn case_insensitive_lookup() {
        let registry = AgentRegistry::new();
        registry.register("myagent".to_string(), AgentInstance::new(test_config("my")));

        assert!(registry.contains_agent("MyAgent"));
        assert!(registry.contains_agent("myagent"));
        assert!(registry.contains_agent("MYAGENT"));
    }

    #[test]
    fn list_agent_ids() {
        let registry = AgentRegistry::new();
        registry.register("alpha".to_string(), AgentInstance::new(test_config("a")));
        registry.register("beta".to_string(), AgentInstance::new(test_config("b")));

        let mut ids = registry.list_agent_ids();
        ids.sort();
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn default_agent_id_falls_back_to_first() {
        let registry = AgentRegistry::new();
        registry.register("other".to_string(), AgentInstance::new(test_config("other")));

        // "main" doesn't exist, so it falls back to the first agent.
        let default = registry.default_agent_id();
        assert!(default.is_some());
        assert_eq!(default.unwrap(), "other");
    }

    #[test]
    fn default_agent_id_none_when_empty() {
        let registry = AgentRegistry::new();
        assert!(registry.default_agent_id().is_none());
    }

    #[test]
    fn with_agent_executes_closure() {
        let registry = AgentRegistry::new();
        registry.register("test".to_string(), AgentInstance::new(test_config("test")));

        let result = registry.with_agent("test", |instance| {
            instance.state()
        });
        assert_eq!(result, Some(crate::types::AgentState::Idle));
    }

    #[test]
    fn with_agent_returns_none_for_missing() {
        let registry = AgentRegistry::new();
        let result = registry.with_agent("nonexistent", |_instance| 42);
        assert!(result.is_none());
    }

    #[test]
    fn can_spawn_subagent_with_wildcard() {
        let registry = AgentRegistry::new();
        registry.set_subagent_allow("parent", vec!["*".to_string()]);

        assert!(registry.can_spawn_subagent("parent", "child_a"));
        assert!(registry.can_spawn_subagent("parent", "child_b"));
    }

    #[test]
    fn can_spawn_subagent_with_specific_ids() {
        let registry = AgentRegistry::new();
        registry.set_subagent_allow("parent", vec!["child_a".to_string(), "child_b".to_string()]);

        assert!(registry.can_spawn_subagent("parent", "child_a"));
        assert!(registry.can_spawn_subagent("parent", "child_b"));
        assert!(!registry.can_spawn_subagent("parent", "child_c"));
    }

    #[test]
    fn can_spawn_subagent_no_allow_list() {
        let registry = AgentRegistry::new();
        assert!(!registry.can_spawn_subagent("parent", "child"));
    }

    #[test]
    fn remove_agent() {
        let registry = AgentRegistry::new();
        registry.register("to_remove".to_string(), AgentInstance::new(test_config("rm")));
        assert!(registry.contains_agent("to_remove"));

        assert!(registry.remove("to_remove"));
        assert!(!registry.contains_agent("to_remove"));
        assert!(!registry.remove("to_remove")); // Already removed
    }

    // --- Additional registry tests ---

    #[test]
    fn default_impl() {
        let registry = AgentRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn with_agent_mut_executes_closure() {
        let registry = AgentRegistry::new();
        registry.register("test".to_string(), AgentInstance::new(test_config("test")));

        let result = registry.with_agent_mut("test", |instance| {
            instance.state()
        });
        assert_eq!(result, Some(crate::types::AgentState::Idle));
    }

    #[test]
    fn with_agent_mut_returns_none_for_missing() {
        let registry = AgentRegistry::new();
        let result = registry.with_agent_mut("nonexistent", |_instance| 99);
        assert!(result.is_none());
    }

    #[test]
    fn register_overwrites_existing() {
        let registry = AgentRegistry::new();
        registry.register("agent".to_string(), AgentInstance::new(test_config("v1")));
        registry.register("agent".to_string(), AgentInstance::new(test_config("v2")));

        assert_eq!(registry.len(), 1);
        let model = registry.with_agent("agent", |i| i.config().model.clone());
        assert_eq!(model, Some("test-model-v2".to_string()));
    }

    #[test]
    fn case_insensitive_subagent_check() {
        let registry = AgentRegistry::new();
        registry.set_subagent_allow("Parent", vec!["Child_A".to_string()]);

        assert!(registry.can_spawn_subagent("parent", "child_a"));
        assert!(registry.can_spawn_subagent("PARENT", "CHILD_A"));
    }

    #[test]
    fn multiple_agents_default_is_main() {
        let registry = AgentRegistry::new();
        registry.register("worker".to_string(), AgentInstance::new(test_config("worker")));
        registry.register("main".to_string(), AgentInstance::new(test_config("main")));

        assert_eq!(registry.default_agent_id(), Some("main".to_string()));
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let registry = AgentRegistry::new();
        assert!(!registry.remove("nonexistent"));
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(AgentRegistry::new());
        let mut handles = Vec::new();

        for i in 0..10 {
            let reg = registry.clone();
            handles.push(thread::spawn(move || {
                reg.register(format!("agent_{}", i), AgentInstance::new(test_config(&format!("t{}", i))));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(registry.len(), 10);
        for i in 0..10 {
            assert!(registry.contains_agent(&format!("agent_{}", i)));
        }
    }

    #[test]
    fn set_subagent_allow_overwrites() {
        let registry = AgentRegistry::new();
        registry.set_subagent_allow("parent", vec!["child_a".to_string()]);
        assert!(registry.can_spawn_subagent("parent", "child_a"));
        assert!(!registry.can_spawn_subagent("parent", "child_b"));

        registry.set_subagent_allow("parent", vec!["child_b".to_string()]);
        assert!(!registry.can_spawn_subagent("parent", "child_a"));
        assert!(registry.can_spawn_subagent("parent", "child_b"));
    }
}
