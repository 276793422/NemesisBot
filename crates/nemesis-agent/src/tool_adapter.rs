//! Adapter that exposes the agent-loop's tools through the
//! `nemesis_tools::registry::Tool` trait so the workflow engine's tool node
//! (`RealToolNodeExecutor`) can invoke them.
//!
//! ## Why this exists
//!
//! There are two incompatible `Tool` traits in this codebase:
//! - `crate::r#loop::Tool` — used by the agent loop. `execute(&self, args: &str,
//!   context: &RequestContext) -> Result<String, String>`, no `name()`,
//!   `description()` returns an owned `String`.
//! - `nemesis_tools::registry::Tool` — used by the workflow engine's
//!   `ToolRegistry`. `name() -> &str`, `execute(&self, args: &Value) ->
//!   ToolResult`.
//!
//! They cannot be unified. The workflow tool node (`RealToolNodeExecutor`)
//! holds a `nemesis_tools::registry::ToolRegistry`, and `gateway.rs` created
//! that registry *empty* — so every `tool` node failed with `tool not found`.
//! This adapter bridges the two: it wraps an agent tool and re-exposes it under
//! the nemesis-tools schema, then `gateway` registers the adapted tools into
//! the workflow registry.
//!
//! ## Security
//!
//! The workflow path previously bypassed the 8-layer security pipeline
//! entirely (`nemesis-workflow` has zero references to `nemesis-security`).
//! The adapter runs `SecurityPlugin::execute` — the **rule pipeline only**,
//! matching the agent path's pre-execution check. It deliberately does NOT
//! trigger the interactive approval popup nor the guardian LLM judge, so batch
//! workflows run unattended (per the workflow tool-node design decision).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::context::RequestContext;
use crate::r#loop::Tool as AgentTool;

/// Synthetic request context used when invoking agent tools from a workflow.
///
/// v1 limitation: the adapter is constructed once at registration time, and
/// `nemesis_tools::Tool::execute(&self, args)` does not receive per-call
/// workflow context (node id / execution id / trigger user). We therefore
/// synthesize a static context. The security check operates on `tool_name +
/// args` (the actual operation), which is what matters; threading
/// per-execution context (via `ToolRegistry::execute_with_full_context`) is a
/// future enhancement.
fn workflow_request_context() -> RequestContext {
    RequestContext::new("workflow", "workflow", "workflow", "workflow")
}

/// Adapter wrapping an agent-loop tool as a `nemesis_tools::registry::Tool`.
pub struct AgentToolAdapter {
    name: String,
    description: String,
    inner: Arc<dyn AgentTool>,
    #[cfg(feature = "security")]
    security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
}

impl AgentToolAdapter {
    /// Wrap a single agent tool. The agent `Tool` trait has no `name()`, so the
    /// outer HashMap key (the tool name) is passed in explicitly.
    #[cfg(feature = "security")]
    pub fn new(
        name: String,
        inner: Arc<dyn AgentTool>,
        security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    ) -> Arc<Self> {
        let description = inner.description();
        Arc::new(Self {
            name,
            description,
            inner,
            security,
        })
    }

    #[cfg(not(feature = "security"))]
    pub fn new(name: String, inner: Arc<dyn AgentTool>) -> Arc<Self> {
        let description = inner.description();
        Arc::new(Self {
            name,
            description,
            inner,
        })
    }
}

#[async_trait]
impl nemesis_tools::registry::Tool for AgentToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        // The agent tool already returns an OpenAI-compatible JSON Schema
        // (e.g. write_file's `{type:object, properties:{path,content},
        // required:[path,content]}`). so forward it verbatim.
        self.inner.parameters()
    }

    async fn execute(&self, args: &serde_json::Value) -> nemesis_tools::types::ToolResult {
        // 8-layer security pipeline (rule layers only). `execute` auto-allows
        // when security is disabled or the tool is unknown to the classifier,
        // so this never spuriously blocks benign/unknown tools.
        #[cfg(feature = "security")]
        {
            if let Some(ref security) = self.security {
                let invocation = nemesis_security::types::ToolInvocation {
                    tool_name: self.name.clone(),
                    args: args.clone(),
                    user: "workflow".to_string(),
                    source: "workflow".to_string(),
                    metadata: HashMap::new(),
                };
                let (allowed, reason) = security.execute(&invocation);
                if !allowed {
                    let reason_str = reason
                        .unwrap_or_else(|| "operation denied by security policy".to_string());
                    tracing::warn!(
                        tool = %self.name,
                        reason = %reason_str,
                        "[WorkflowToolAdapter] Security blocked tool"
                    );
                    // Mirror the agent path's explicit prefix so the failure
                    // reason is unambiguous in the workflow node result.
                    return nemesis_tools::types::ToolResult::error(&format!(
                        "⛔ SECURITY BLOCKED: {} — The security policy denied this operation.",
                        reason_str
                    ));
                }
            }
        }

        // The agent trait takes a JSON string of args.
        let args_str = match serde_json::to_string(args) {
            Ok(s) => s,
            Err(e) => {
                return nemesis_tools::types::ToolResult::error(&format!(
                    "failed to serialize tool args: {}",
                    e
                ));
            }
        };

        // Inject synthetic channel/chat_id for context-aware tools (message,
        // spawn, cluster_rpc, ...). No-op for tools that don't override
        // `set_context`.
        let ctx = workflow_request_context();
        self.inner.set_context(&ctx.channel, &ctx.chat_id);

        match self.inner.execute(&args_str, &ctx).await {
            Ok(output) => nemesis_tools::types::ToolResult::success(&output),
            Err(err) => nemesis_tools::types::ToolResult::error(&err),
        }
    }
}

#[cfg(test)]
mod tests;
