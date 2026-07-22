//! `WorkflowCallStack` — runtime call-stack abstraction for nested workflow
//! invocations (milestone 1c-F2).
//!
//! Goal: give the engine a single place to observe every active workflow
//! execution and enforce [`MAX_RECURSION_DEPTH`] for `AgentTool`-triggered
//! nestings.
//!
//! **What this is NOT**: this is not the primary recursion-limiter — that
//! role belongs to `WorkflowRunTool`, which rejects outbound calls before
//! they reach the engine. The call stack is defense-in-depth: even if a
//! caller bypasses the tool, the engine still rejects depth-over-limit
//! pushes. It also doubles as an introspection surface (future
//! `workflow.status` WSAPI command can show "what's running right now").
//!
//! **Concurrency**: one stack per engine. `parking_lot::Mutex` is fine
//! because critical sections are tiny (push/pop/peek) and never await.

use parking_lot::Mutex;

use crate::types::{MAX_RECURSION_DEPTH, TriggerSource};

/// A single frame on the workflow call stack.
///
/// One frame == one in-flight `WorkflowEngine::run` invocation. Pushed when
/// the engine starts executing a workflow, popped when the execution
/// settles into a terminal state (Completed / Failed / Cancelled) or
/// pauses (Waiting).
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// The execution id of *this* frame's workflow run.
    pub execution_id: String,
    /// Workflow name being executed.
    pub workflow_name: String,
    /// Execution id of the enclosing workflow, if this frame was triggered
    /// by a `sub_workflow` node. `None` for top-level executions.
    pub parent_execution_id: Option<String>,
    /// Trigger source recorded for this execution. Used to recover the
    /// `recursion_depth` (only `AgentTool` triggers carry a non-zero depth).
    pub trigger_source: Option<TriggerSource>,
    /// Agent-tool recursion depth (0 for non-AgentTool triggers). Frames
    /// with `recursion_depth > MAX_RECURSION_DEPTH` are rejected on push.
    pub recursion_depth: u32,
}

impl CallFrame {
    /// Derive a frame's recursion depth from its trigger source.
    ///
    /// Only `AgentTool` triggers carry a depth (set by `WorkflowRunTool`).
    /// All other triggers — Cli, Cron, Webhook, Chat, Event — count as
    /// depth 0 and bypass the recursion limit entirely.
    pub fn depth_from_trigger(trigger: &Option<TriggerSource>) -> u32 {
        match trigger {
            Some(TriggerSource::AgentTool {
                recursion_depth, ..
            }) => *recursion_depth,
            _ => 0,
        }
    }
}

/// Per-engine call stack. Held inside `Arc<WorkflowEngine>` and shared
/// across all `run` / `start_async` invocations.
///
/// **Why a Mutex and not RwLock**: every operation (push/pop/peek) writes.
/// RwLock's reader-preference would slow down the (very common) push/pop
/// path for no benefit.
pub struct WorkflowCallStack {
    frames: Mutex<Vec<CallFrame>>,
}

impl WorkflowCallStack {
    pub fn new() -> Self {
        Self {
            frames: Mutex::new(Vec::new()),
        }
    }

    /// Push a new frame. Rejects with an error message if the frame's
    /// `recursion_depth` exceeds `MAX_RECURSION_DEPTH`.
    ///
    /// Returns `Ok(())` on success.
    pub fn push(&self, frame: CallFrame) -> Result<(), String> {
        if frame.recursion_depth > MAX_RECURSION_DEPTH {
            return Err(format!(
                "max recursion depth {} exceeded (attempted depth={})",
                MAX_RECURSION_DEPTH, frame.recursion_depth
            ));
        }
        let mut frames = self.frames.lock();
        frames.push(frame);
        Ok(())
    }

    /// Pop the top frame. Returns `None` if the stack is empty (this
    /// indicates a push/pop imbalance and should be investigated, but we
    /// treat it as a soft failure rather than panicking).
    pub fn pop(&self) -> Option<CallFrame> {
        self.frames.lock().pop()
    }

    /// Current stack depth (number of frames).
    pub fn depth(&self) -> usize {
        self.frames.lock().len()
    }

    /// Snapshot of all frames. Useful for diagnostics / future WSAPI.
    pub fn snapshot(&self) -> Vec<CallFrame> {
        self.frames.lock().clone()
    }

    /// True if no frames are currently on the stack.
    pub fn is_empty(&self) -> bool {
        self.frames.lock().is_empty()
    }
}

impl Default for WorkflowCallStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
