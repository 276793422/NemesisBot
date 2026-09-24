//! Todo 收尾提醒 hook（2026-09-24，`todowrite` 记账兜底）。
//!
//! 背景：`todowrite` 是全量提交式工具，清单收尾全靠模型自觉（H3 few-shot
//! 是软纪律，无硬约束）——模型经常把活干完、最终答复都发出去了，却忘记
//! 最后调一次 todowrite 把 `in_progress` 收掉（2026-09-24 用户实测：汇总
//! 报告正文已交付，清单停在 3/4）。本模块是 K2 turn-end lifecycle hook
//! 的内建实现：终答案被接受后读 todo 落盘文件，还有未完成项 →
//! [`TurnEndDecision::Continue`] 注入一次性提醒（模型下一轮先收尾清单
//! 再答复；清单收不回来的暂停项诚实改回 `pending`）。
//!
//! 缓存纪律（prompt cache 命中率不受影响的根据）：注入走 K2 既有路径
//! `instance.add_user_message(feedback)`——追加在消息列表**尾部**（本轮
//! 全量列表字节不变 → 增量前缀缓存全命中；只有提醒本体按新增输入计费），
//! 与 max_tokens 续写提示 / cc-hooks feedback 同槽位。绝不进头部 system
//! prompt 或每轮合并快照注入点（那两处字节变化会打穿缓存前缀）。
//!
//! 预算：`stop_hook_active`（本轮已有任何 Continue，含用户脚本）时自限
//! 跳过 = 每 turn 至多一次；loop 侧 [`crate::hooks::MAX_TURN_END_CONTINUES`]
//! 硬封顶 fail-open。heartbeat / GiveUp / 错误路径不经过 turn-end hooks
//! （`judge_final_answer` 只在正常 Accept 路径跑 hooks）。
//!
//! 装配：gateway 侧 agent_factory 主 loop / 项目 loop 各注册一次
//! （workspace 分别 = 主工作区 / 项目目录）；与 todowrite 工具落盘同根
//! 同 sanitize 真相源（`sanitize_path_segment`，F-U4-4）。

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

    /// 与 `TodoWriteTool::execute` 同构的落盘路径（同一 sanitize 真相源）。
    fn todo_path_for(&self, session_key: &str) -> PathBuf {
        let safe = nemesis_utils::sanitize::sanitize_path_segment(session_key);
        nemesis_path::resolve_sessions_dir_in_workspace(&self.workspace)
            .join(format!("todo_{safe}.json"))
    }
}

#[async_trait]
impl LifecycleHook for TodoCloseoutHook {
    fn name(&self) -> String {
        "todo-closeout-reminder".to_string()
    }

    async fn on_turn_end(&self, end: &HookTurnEnd) -> TurnEndDecision {
        // 每 turn 至多一次：本轮已有任何 hook Continue（含用户脚本）就
        // 不再叠加——防止提醒与用户 hook 轮流拉锯烧预算（fail-open 方向）。
        if end.stop_hook_active {
            return TurnEndDecision::Stop;
        }
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
        tracing::info!(
            "[todo-closeout] session '{}' ends with {} unfinished todo item(s); one more round",
            end.session_key,
            unfinished.len()
        );
        TurnEndDecision::Continue {
            feedback: format!(
                "<system-reminder>\n你要结束本轮，但本会话的 todo 清单还有 {} 项未完成：\n{}\n\
                 请先如实收尾再给最终答复：\n\
                 1. 已完成的项：调用 todowrite 标记 completed（全量提交——每次调用替换整个清单，\
                 所有条目都要带上，包括没变的）。\n\
                 2. 确实还没做/本轮不做的项：标记改回 pending，并在答复里如实说明进度与剩余工作。\n\
                 不要虚报完成；清单状态必须与实际一致。\n</system-reminder>",
                unfinished.len(),
                unfinished.join("\n")
            ),
        }
    }
}

#[cfg(test)]
mod todo_closeout_tests;
