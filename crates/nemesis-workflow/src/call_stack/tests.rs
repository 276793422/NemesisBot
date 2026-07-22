use super::*;
use crate::types::TriggerSource;

#[test]
fn push_and_pop_basic() {
    let stack = WorkflowCallStack::new();
    assert!(stack.is_empty());
    assert_eq!(stack.depth(), 0);

    let frame = CallFrame {
        execution_id: "exec-1".to_string(),
        workflow_name: "wf".to_string(),
        parent_execution_id: None,
        trigger_source: Some(TriggerSource::Cli),
        recursion_depth: 0,
    };
    stack.push(frame).unwrap();
    assert_eq!(stack.depth(), 1);
    assert!(!stack.is_empty());

    let popped = stack.pop().expect("frame should pop");
    assert_eq!(popped.execution_id, "exec-1");
    assert!(stack.is_empty());
}

#[test]
fn pop_empty_returns_none() {
    let stack = WorkflowCallStack::new();
    assert!(stack.pop().is_none());
}

#[test]
fn depth_from_trigger_extracts_agent_tool_recursion() {
    // AgentTool carries depth.
    let agent = Some(TriggerSource::AgentTool {
        tool_call_id: "tc-1".to_string(),
        recursion_depth: 2,
    });
    assert_eq!(CallFrame::depth_from_trigger(&agent), 2);

    // Non-AgentTool triggers are always depth 0.
    assert_eq!(CallFrame::depth_from_trigger(&Some(TriggerSource::Cli)), 0);
    assert_eq!(CallFrame::depth_from_trigger(&None), 0);
}

#[test]
fn push_rejects_depth_above_max() {
    let stack = WorkflowCallStack::new();
    let too_deep = CallFrame {
        execution_id: "x".to_string(),
        workflow_name: "wf".to_string(),
        parent_execution_id: None,
        trigger_source: Some(TriggerSource::AgentTool {
            tool_call_id: "tc".to_string(),
            recursion_depth: MAX_RECURSION_DEPTH + 1,
        }),
        recursion_depth: MAX_RECURSION_DEPTH + 1,
    };
    let err = stack
        .push(too_deep)
        .expect_err("over-limit push should reject");
    assert!(err.contains("max recursion depth"), "got: {}", err);
    assert!(stack.is_empty(), "rejected push must not leave a frame");
}

#[test]
fn push_accepts_depth_at_max() {
    let stack = WorkflowCallStack::new();
    let at_max = CallFrame {
        execution_id: "x".to_string(),
        workflow_name: "wf".to_string(),
        parent_execution_id: None,
        trigger_source: Some(TriggerSource::AgentTool {
            tool_call_id: "tc".to_string(),
            recursion_depth: MAX_RECURSION_DEPTH,
        }),
        recursion_depth: MAX_RECURSION_DEPTH,
    };
    stack.push(at_max).expect("depth==MAX should be accepted");
    assert_eq!(stack.depth(), 1);
}

#[test]
fn snapshot_is_a_copy() {
    let stack = WorkflowCallStack::new();
    stack
        .push(CallFrame {
            execution_id: "e1".to_string(),
            workflow_name: "wf".to_string(),
            parent_execution_id: None,
            trigger_source: Some(TriggerSource::Cli),
            recursion_depth: 0,
        })
        .unwrap();
    stack
        .push(CallFrame {
            execution_id: "e2".to_string(),
            workflow_name: "wf".to_string(),
            parent_execution_id: Some("e1".to_string()),
            trigger_source: Some(TriggerSource::Cli),
            recursion_depth: 0,
        })
        .unwrap();

    let snap = stack.snapshot();
    assert_eq!(snap.len(), 2);
    assert_eq!(snap[0].execution_id, "e1");
    assert_eq!(snap[1].parent_execution_id.as_deref(), Some("e1"));

    // Mutating snapshot doesn't affect stack.
    drop(snap);
    assert_eq!(stack.depth(), 2);
}

#[test]
fn lifo_order_preserved() {
    let stack = WorkflowCallStack::new();
    for id in ["a", "b", "c"] {
        stack
            .push(CallFrame {
                execution_id: id.to_string(),
                workflow_name: "wf".to_string(),
                parent_execution_id: None,
                trigger_source: Some(TriggerSource::Cli),
                recursion_depth: 0,
            })
            .unwrap();
    }
    assert_eq!(stack.depth(), 3);

    // Pop order is LIFO.
    assert_eq!(stack.pop().unwrap().execution_id, "c");
    assert_eq!(stack.pop().unwrap().execution_id, "b");
    assert_eq!(stack.pop().unwrap().execution_id, "a");
    assert!(stack.is_empty());
}
