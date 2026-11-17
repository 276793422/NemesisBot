//! 停车推送（无人值守加固 批三）：看板任务停车转人工时向已配置出站
//! 通道推一条停车摘要——「需要人时人必须在场」原则的使能件：人不盯
//! 看板也能第一时间知道有单停车。
//!
//! 结构：
//! - 发射面（本模块 [`emit_park_notice`]）：`park_with_handoff`
//!   （全部停车分支单一收口）调用；现读 `home/config.json` 的
//!   `board.notify`（改配置即时生效，`load_board_flags` 同款读法），
//!   `enabled && channel && chat_id` 齐备才调 hook——未配置 / hook 未
//!   注册 = 诚实 no-op。
//! - hook 槽（静态，插件 ToolSync hook 先例）：gateway 装配期注册一次
//!   闭包（捕获 MessageBus），发布走现成 ChannelManager 出站派发面，
//!   **不新建通道**。
//!
//! 推送失败只 WARN 不炸——停车现场（handoff 档案 + 看板评论）已落盘，
//! 推送是增值通知不是停车本体。

use std::sync::Arc;

/// 停车通知载荷。字段 = HandoffSpec 的人可读子集（不含 environment/
/// attempted 机器面——看板评论与 handoff 档案里有全量现场）。
pub(crate) struct ParkNotice {
    pub issue_number: String,
    pub issue_id: i64,
    pub stop_kind: &'static str,
    pub suggested_action: &'static str,
    pub hint: String,
    pub resume_entry: String,
}

/// hook 签名：发射面已做 config 门控，闭包只管投递（channel/chat_id 由
/// 发射面从 config 传入，闭包只捕获 bus）。
pub(crate) type ParkNotifyHook = Arc<dyn Fn(&ParkNotice, &str, &str) + Send + Sync>;

static PARK_NOTIFY_HOOK: std::sync::RwLock<Option<ParkNotifyHook>> = std::sync::RwLock::new(None);

/// gateway 装配期注册推送闭包（Some = 注册/替换，None = 摘除；测试可
/// 换装）。std RwLock：持锁只 clone Arc 不跨回调。
pub(crate) fn set_park_notify_hook(hook: Option<ParkNotifyHook>) {
    *PARK_NOTIFY_HOOK.write().expect("park notify hook 锁中毒") = hook;
}

/// park_with_handoff 发射面：现读 `board.notify` 齐备才调 hook。
/// config 读取失败 = 诚实按关处理（推送是增值动作，不炸停车收口）。
pub(crate) fn emit_park_notice(home: &std::path::Path, notice: &ParkNotice) {
    let notify = read_notify_config(home);
    let Some(n) = notify else {
        return;
    };
    if !n.enabled || n.channel.is_empty() || n.chat_id.is_empty() {
        return;
    }
    let hook = PARK_NOTIFY_HOOK
        .read()
        .expect("park notify hook 锁中毒")
        .clone();
    if let Some(hook) = hook {
        hook(notice, &n.channel, &n.chat_id);
    }
}

/// `board.notify` 现读（`load_board_flags` 同款读法：home/config.json →
/// board 段；缺段 = Default 关）。读失败 = None（no-op）。
fn read_notify_config(home: &std::path::Path) -> Option<nemesis_config::BoardNotifyConfig> {
    let path = home.join("config.json");
    let cfg = nemesis_config::load_config(&path).ok()?;
    cfg.board.map(|b| b.notify)
}

/// hook 是进程级全局槽：并行测试下装/拆会互扰（A 测试注册的 hook 可能
/// 收到 B 测试触发的 emit）——hook 相关测试统一持本锁串行。tokio
/// Mutex：持锁跨 `.await` 是本锁的常规用法（集成测试持锁跑完整验收），
/// 同步单测用 `blocking_lock()`（无 runtime 环境合法）。
#[cfg(test)]
pub(crate) static HOOK_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests;
