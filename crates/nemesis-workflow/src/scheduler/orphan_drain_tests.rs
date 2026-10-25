//! R101（工作流 DAG 语义审计）：失败路径兄弟任务遗弃——schedule_inner 收集
//! 循环 `return Err` 时剩余 JoinHandle 被 drop = 任务 detach，工作流已宣告
//! 失败后兄弟节点仍在跑副作用；engine 失败路径只 remove token 不 cancel
//! （drop≠cancel，detach 任务持有的 clone 永不触发），且 token 已出 map、
//! cancel_execution 再无入口——孤儿任务带完整重试预算跑到自然结束。
//! 修复 = 收集到首个错误先 cancel 共享 token 再 join 兄弟（任务内 biased
//! select 在 cancel 臂 break，executor future 在 await 点被丢弃）。
//!
//! 真红注记：层内收集顺序不定（topological_sort 层序非确定），两种顺序下
//! 旧代码都红——fail_fast 先被收集：返回瞬间 slow 还睡着（dropped=false）；
//! slow 先被收集：等它**自然睡满跑完**（completed=true）才轮到报错，正是
//! 孤儿跑完副作用的症状本身。修复后确定性绿：completed=false + dropped=true。

use super::*;
use crate::context::WorkflowContext;
use crate::nodes::{NodeExecutor, NodeExecutorRegistry};
use crate::types::{Edge, ExecutionState, NodeDef, NodeResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// future 被 drop（cancel）或跑完时置 dropped=true 的哨兵。
struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// slow 节点：睡 5s 后成功。schedule 返回 Err 的瞬间 dropped 必须已为 true
/// （新语义：兄弟被 cancel+join 之后才返回；旧语义 = 任务还在睡，flag=false）。
struct SlowDropWatchExecutor {
    dropped: Arc<AtomicBool>,
    completed: Arc<AtomicBool>,
}

#[async_trait]
impl NodeExecutor for SlowDropWatchExecutor {
    async fn execute(
        &self,
        _node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let _guard = DropFlag(self.dropped.clone());
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.completed.store(true, Ordering::SeqCst);
        Ok(NodeResult {
            node_id: _node.id.clone(),
            output: serde_json::json!({"ok": true}),
            error: None,
            state: ExecutionState::Completed,
            started_at: chrono::Local::now(),
            ended_at: chrono::Local::now(),
            metadata: HashMap::new(),
        })
    }
}

/// 立即失败的节点。
struct FailFastExecutor;

#[async_trait]
impl NodeExecutor for FailFastExecutor {
    async fn execute(
        &self,
        _node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        Err("boom".to_string())
    }
}

#[tokio::test]
async fn failure_cancels_sibling_tasks_before_returning_err() {
    let dropped = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let registry = NodeExecutorRegistry::new();
    registry.register("r101_fail", Arc::new(FailFastExecutor));
    registry.register(
        "r101_slow",
        Arc::new(SlowDropWatchExecutor {
            dropped: dropped.clone(),
            completed: completed.clone(),
        }),
    );

    let mk = |id: &str, ty: &str| NodeDef {
        id: id.to_string(),
        node_type: ty.to_string(),
        config: HashMap::new(),
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    };
    // 同层两节点：fail_fast 立刻失败，slow 睡 5s。
    let nodes = vec![mk("fail_fast", "r101_fail"), mk("slow", "r101_slow")];
    let edges: Vec<Edge> = vec![];
    let mut wf_ctx = WorkflowContext::new(HashMap::new());
    let cancel = CancellationToken::new();

    let err = schedule(&nodes, &edges, &registry, &mut wf_ctx, cancel)
        .await
        .unwrap_err();
    assert!(err.contains("fail_fast"), "错误必须指向 fail_fast: {}", err);
    assert!(
        !completed.load(Ordering::SeqCst),
        "slow 节点不得在 workflow 失败后跑完（completed=true = 孤儿副作用         自然结束）"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "schedule 返回 Err 时 slow 节点的 future 必须已被丢弃（cancel+join）         ——flag=false = 兄弟任务被遗弃，仍在后台跑副作用"
    );
}
