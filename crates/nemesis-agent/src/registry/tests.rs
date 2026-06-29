use super::*;

fn test_config(name: &str) -> AgentConfig {
    AgentConfig {
        model: format!("test-model-{}", name),
        system_prompt: Some(format!("You are {}.", name)),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
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
