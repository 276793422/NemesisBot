//! NemesisBot Events Module —— 事件驱动唤醒（质变计划 项1 P1 + 事件源扩展）。
//!
//! 「出事它主动叫你」：文件系统变化 / 外部 webhook 推送 / 后台任务输出正则 /
//! 系统指标阈值，按规则唤醒指定会话，agent 主动行动。
//!
//! 结构（五件）：
//! - [`rules`]：`<workspace>/config/events.json` 规则 schema + 加载/校验（按
//!   kind 分派字段段：fs/webhook/proc/sys；probe/cron 为 P2，加载期剔除）；
//! - [`glob`]：极简通配符匹配（`*` / `?`，不引 glob crate）；
//! - [`throttle`]：防风暴三闸纯逻辑（per-rule 冷却 / 滑动小时窗 / burst 合并）；
//! - [`deliver`]：模板渲染（fs=`{count}`/`{path}`/`{paths}`；proc=`{line}`/
//!   `{job_id}`；webhook/sys=`{payload}`）；
//! - [`watcher`] + [`manager`]：notify 包装 + 多源泵任务（热重载 → 匹配 →
//!   三闸 → 注入；webhook 经 [`manager::WebhookSubmitter`] 提交，proc 经
//!   [`manager::ProcReaderFn`] 增量读，sys 走 sysinfo 轮询 + 滞回）。
//!
//! 解耦纪律：本 crate 不依赖 bus / estop——注入与冻结都走闭包
//! （[`manager::InjectorFn`] / [`manager::FrozenFn`]），装配在 gateway 侧完成
//! （runtime.rs，复用 web_approval 的 bus 注入先例）。
//!
//! 三闸语义（跨源统一）：
//! 1. per-rule 冷却（`cooldown_secs`，0=关）——冷却期内的后续爆发不投递（丢弃计数）；
//! 2. 小时窗（per-rule `max_per_hour` + 全局上限，0=不限）——滑动 1h 窗口；
//! 3. estop 冻结（`FrozenFn`）——冻结期 pending 诚实丢弃，release 后重新积累。
//!
//! 已知边界：sys/proc 采样是轮询（系统指标与子进程输出没有推模式，注明）；
//! 合并丢失中间明细（计数注记补偿）；目标会话缺席 = 投递诚实失败留痕，
//! 不静默创建会话；webhook token 明文存 config（与 credentials.yaml 同级信任）。

pub mod deliver;
pub mod glob;
pub mod manager;
pub mod rules;
pub mod throttle;
pub mod watcher;

pub use deliver::{render_prompt, render_prompt_vars};
pub use glob::wildcard_match;
pub use manager::{
    EventSnapshot, EventWatchManager, FrozenFn, InjectorFn, ManagerStats, PendingView,
    ProcReaderFn, RuleView, SysCurrentView, SysDiskView, SysSample, WebhookMsg, WebhookSubmitter,
};
pub use rules::{
    DeliverSpec, EventRule, LoadedRules, WatchSpec, load_rules, rule_matches, save_rules,
};
pub use throttle::{CooldownGate, HourWindow, MergeBuffer};
pub use watcher::{FsWatcher, RawFsEvent};
