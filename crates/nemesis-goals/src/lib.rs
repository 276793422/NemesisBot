//! NemesisBot Goals Module —— 目标文件驱动的自监督循环（goal-conditioned loop）。
//!
//! 「监督者从『人盯』变成『目标文件 + 评估闸盯』」：goal active 且绑定会话的
//! 每次 agent turn 结束，评估闸独立重读目标文件评估项目——DONE（唯一 done
//! 出口，agent 自称完成不生效）或 CONTINUE（注入 goal 轮继续循环）。
//!
//! 结构（三件）：
//! - [`state`]：注册表条目 / 评估记录 schema（`config/goals.json` +
//!   `goals/{id}.evals.jsonl`）；
//! - [`store`]：注册表原子读写（进程级单写者锁）+ goal.md / evals.jsonl
//!   存取 + id 分配 + 三清删除；
//! - [`gate`]：纯逻辑——状态转移校验 / 预算判定 / 评估提示词 / 判定解析 /
//!   注入消息构建 / 文件保护裁决（全部可单测，零 IO）。
//!
//! 解耦纪律（nemesis-loop 同款）：本 crate 不依赖 bus / estop / agent /
//! LLM——评估闸驱动器在 gateway 侧（goals_engine）：watcher 触发 → 本
//! crate 读状态出材料 → 宿主闭包调 LLM / 注入。estop 冻结由驱动器表达
//! （engaged 期跳过评估，goal 状态不动，release 后下一 turn 照常）。
//!
//! 状态机四态（dsh 惯例，D-Q11）：`active|paused|blocked|done`。
//! `paused` = 人关闭（可逆，人 enable 恢复）；`blocked` 合并全部失败面
//! （`round-limit` 预算耗尽 / `eval-failure` 评估解析失败），带 code + 人读
//! message，人 enable 可解锁；`done` 唯一出口 = 评估闸判定（D-Q7，agent
//! 无 done 能力）。agent 工具面只有 add / delete（仅自有）/ disable——
//! done 与 enable 归人与评估闸（防自锁）。
//!
//! 轮数预算（D-Q10）：`max_rounds` 默认 256、0=不限；只数评估闸 CONTINUE
//! 注入触发的 goal 轮（人在场 turn 照常评估但不占预算）。

pub mod gate;
pub mod state;
pub mod store;

#[cfg(test)]
mod gate_tests;
#[cfg(test)]
mod store_tests;

pub use gate::{
    budget_block_reason, build_eval_prompt, build_round_message, extract_turn_material,
    fence_denial, parse_verdict, should_eval_on_outbound, transition_allowed, ToolCallMaterial,
};
pub use state::{
    EvalRecord, EvalVerdict, GoalCreatedBy, GoalEntry, GoalStatus, DEFAULT_MAX_ROUNDS,
};
pub use store::{
    alloc_id, append_eval, goal_md_path, goals_dir, load_registry, read_evals_tail, read_goal_md,
    registry_path, remove_goal, save_registry, update_registry, write_goal_md, GOALS_DIR_NAME,
    GOALS_REGISTRY_FILE,
};
