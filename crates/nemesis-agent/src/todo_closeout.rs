//! Todo 收尾提醒 hook（2026-09-24，`todowrite` 记账兜底；2026-11-22
//! 语义反转：从「收摊整理器」改为「续跑发动机」，见
//! docs/REPORT/2026-11-22_长任务中途检查点停车根因分析 同日修复批）。
//!
//! 背景（v1）：`todowrite` 是全量提交式工具，清单收尾全靠模型自觉——
//! 模型经常把活干完、最终答复都发出去了，却忘记最后调一次 todowrite
//! 把 `in_progress` 收掉。v1 是 turn-end lifecycle hook：终答案被接受后
//! 读 todo 落盘文件，还有未完成项 → [`TurnEndDecision::Continue`] 注入
//! 一次性收尾提醒。
//!
//! v2 反转（2026-11-22 长任务停车根因批）：v1 实测把「汇报收摊」合法化
//! 了——feedback 文案引导「如实说明进度与剩余工作」，模型照做：拦停 →
//! todowrite → 检查点汇报照样发出。v2 改为两段升级模板：
//!
//! - **首次拦停**（`stop_hook_active=false`）：未完成项是用户任务的一部分，
//!   **继续用工具执行**；清单更新随活提交（下一轮干活顺带 todowrite，
//!   不单开记账轮）；全部完成才给最终答复；
//!   确属用户专属域阻塞的项才保留未完成（最终答复列明尝试记录）。
//! - **升级拦停**（`stop_hook_active=true`，即本轮已被 Continue 过一次）：
//!   「你已被告知继续执行却再次停车」——确属用户专属域阻塞就列证据清单
//!   （试了什么、为何只有用户能做）；否则立即继续执行工具。
//! - 拦停预算耗尽不再拦：loop 侧 `agents.turn_end_continues`
//!   （缺省 2=旧常量行为，显式 0=不限 opt-in；有限帽时第 N+1 次停车）
//!   fail-open 放行——真阻塞有合法出口，钩子故障也不会无限拉锯（兜底上界
//!   = max_tool_iterations 迭代闸，2026-11-23 起该键缺省 0=不限，仅在
//!   显式配置非 0 时存在）。
//!
//! 缓存纪律（prompt cache 命中率不受影响的根据）：注入走既有路径
//! `instance.add_user_message(feedback)`——追加在消息列表**尾部**（本轮
//! 全量列表字节不变 → 增量前缀缓存全命中；只有提醒本体按新增输入计费），
//! 与 max_tokens 续写提示 / cc-hooks feedback 同槽位。绝不进头部 system
//! prompt 或每轮合并快照注入点（那两处字节变化会打穿缓存前缀）。
//!
//! heartbeat / GiveUp / 错误路径不经过 turn-end hooks（
//! `judge_final_answer` 只在正常 Accept 路径跑 hooks）。
//!
//! 装配：gateway 侧 agent_factory 主 loop / 项目 loop 各注册一次
//! （workspace 分别 = 主工作区 / 项目目录）；与 todowrite 工具落盘同根
//! 同 sanitize 真相源（`sanitize_path_segment`，F）。

use std::path::PathBuf;

use async_trait::async_trait;

use crate::hooks::{HookTurnEnd, LifecycleHook, TurnEndDecision};

/// todo 收尾提醒 hook（构造时持 workspace 根，与 `TodoWriteTool` 同根）。
pub struct TodoCloseoutHook {
    /// 存储根 workspace（`sessions/` 目录挂其下）。
    workspace: PathBuf,
}

impl TodoCloseoutHook {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    /// 落盘路径走 nemesis-path 单一真相源（2026-11-05：与 TodoWriteTool
    /// 写路径、todo_get 读路径、回收站寻址同源）。
    fn todo_path_for(&self, session_key: &str) -> PathBuf {
        nemesis_path::session_todo_file_in_workspace(&self.workspace, session_key)
    }
}

/// 首次拦停模板：继续执行，不是汇报收摊。
fn first_block_feedback(unfinished: &[String]) -> String {
    format!(
        "<system-reminder>\n你要结束本轮，但本会话的 todo 清单还有 {} 项未完成：\n{}\n\
         未完成项是用户任务的一部分，**继续用工具执行**，不要以汇报收摊：\n\
         1. 按清单继续调用工具执行剩余项；清单更新随活提交——在下一次干活的同一轮里\
         顺带调用 todowrite 标记 completed（全量提交），不要为更新清单单开一轮。\n\
         2. 全部完成后才给最终答复。\n\
         3. 确属只有用户能做的阻塞（凭据/授权/物理接入/外部系统权限，且已穷尽替代手段）\
         的项才保留未完成；最终答复中列明每项的尝试记录与需要用户做什么。\n\
         不要虚报完成；清单状态必须与实际一致。\n</system-reminder>",
        unfinished.len(),
        unfinished.join("\n")
    )
}

/// 升级拦停模板（本轮已被拦过一次仍停车）：要么列证据，要么继续干活。
fn escalation_feedback(unfinished: &[String]) -> String {
    format!(
        "<system-reminder>\n你已被告知继续执行却再次以未完成清单停车（仍有 {} 项）：\n{}\n\
         二选一，不要再用进度总结收摊：\n\
         1. 若剩余项确属只有用户能做的阻塞：在最终答复列出证据清单——每项写清\
         试了什么手段、失败原因、为何落在用户专属域（凭据/授权/物理接入/外部系统权限）。\n\
         2. 否则立即继续调用工具执行剩余项，完成后再给最终答复。\n</system-reminder>",
        unfinished.len(),
        unfinished.join("\n")
    )
}

#[async_trait]
impl LifecycleHook for TodoCloseoutHook {
    fn name(&self) -> String {
        "todo-closeout-reminder".to_string()
    }

    async fn on_turn_end(&self, end: &HookTurnEnd) -> TurnEndDecision {
        // 读清单：缺失 = 会话没用过 todowrite（没有可收尾的东西）；损坏 =
        // fail-open（提醒兜底不该把会话卡死）。两种情况都放行停轮。
        let raw = match std::fs::read_to_string(self.todo_path_for(&end.session_key)) {
            Ok(raw) => raw,
            Err(_) => return TurnEndDecision::Stop,
        };
        let todos: Vec<nemesis_types::agent::TodoItem> = match serde_json::from_str(&raw) {
            Ok(todos) => todos,
            Err(_) => return TurnEndDecision::Stop,
        };
        let unfinished: Vec<String> = todos
            .iter()
            .filter(|t| t.status != nemesis_types::agent::TodoStatus::Completed)
            .map(|t| {
                let state = if t.status == nemesis_types::agent::TodoStatus::InProgress {
                    "进行中"
                } else {
                    "待办"
                };
                format!("  - [{state}] {}", t.content)
            })
            .collect();
        if unfinished.is_empty() {
            return TurnEndDecision::Stop;
        }
        // stop_hook_active = 本轮已被任何 hook Continue 过（含用户脚本）——
        // 升级信号：模型被告知继续后仍停车，换升级模板再拦一次；拦停预算
        // 耗尽（agents.turn_end_continues，缺省 2=旧常量行为、显式
        // 0=不限 opt-in）由 loop 侧 fail-open 放行。
        let feedback = if end.stop_hook_active {
            escalation_feedback(&unfinished)
        } else {
            first_block_feedback(&unfinished)
        };
        tracing::info!(
            "[todo-closeout] session '{}' ends with {} unfinished todo item(s); {} round",
            end.session_key,
            unfinished.len(),
            if end.stop_hook_active {
                "escalation"
            } else {
                "one more"
            }
        );
        TurnEndDecision::Continue { feedback }
    }
}

#[cfg(test)]
mod todo_closeout_tests;
