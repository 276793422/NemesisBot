//! EventWatchManager：规则热重载 + 事件泵 + 三闸 + 注入。
//!
//! 泵任务单线程持有全部状态（[`ManagerState`]），无锁——唯一共享面是
//! 事件 channel（watcher 回调 → 泵）与两个闭包（注入 / 冻结判定），都由
//! gateway 装配侧提供（本 crate 不依赖 bus / estop）。
//!
//! 泵循环（~250ms 粒度）：
//! 1. `mtime` 热重载检查（变化才重载；解析失败**保留旧规则** + warn——
//!    半写文件不该把在监听的规则清空）；
//! 2. ingest channel 里的 fs 事件（规则匹配 + 同路径 debounce 折叠）；
//! 3. [`ManagerState::run_cycle`]：estop 冻结 → 丢弃全部 pending（诚实计数）；
//!    未冻结 → 到期 buffer 过三闸（冷却 / per-rule 小时窗 / 全局小时窗）→
//!    渲染 → 注入闭包（false = 会话缺席，诚实失败留痕不重试排队）。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::throttle::{CooldownGate, HourWindow, MergeBuffer};
use crate::watcher::{FsWatcher, RawFsEvent, now_ms};

/// 注入闭包：`(session_key, content) -> bool`（false = 会话缺席/注入被拒）。
/// gateway 侧实现 = chat_log 在位检查 + `bus.publish_inbound`
/// （web_approval 续行注入器同款先例）。
pub type InjectorFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// 冻结判定闭包：true = estop 在场（冻结期 pending 诚实丢弃，release 恢复）。
pub type FrozenFn = Arc<dyn Fn() -> bool + Send + Sync>;

const PUMP_TICK_MS: u64 = 250;
const EVENT_CHANNEL_CAP: usize = 1024;
/// 全局每小时投递上限（跨规则合计；0=不限）。规则级上限在 deliver.max_per_hour。
pub const DEFAULT_GLOBAL_HOURLY_CAP: u32 = 60;

/// 运行统计（观测面；P2 EventsView 消费）。
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ManagerStats {
    pub delivered_events: u64,
    pub delivered_messages: u64,
    pub dropped_estop: u64,
    pub dropped_cooldown: u64,
    pub dropped_hourly: u64,
    pub dropped_delivery: u64,
    pub dropped_reload: u64,
    pub deduped: u64,
    pub reloads: u64,
}

/// 单条规则的观测投影（生效值展开默认值；EventsView 规则表）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RuleView {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub glob: Option<String>,
    pub session_key: String,
    pub enabled: bool,
    pub cooldown_secs: u64,
    pub merge_window_secs: u64,
    pub max_per_hour: u32,
}

/// 在攒 buffer 投影（静默窗中待投递的规则）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingView {
    pub rule_id: String,
    pub count: u32,
}

/// 泵快照（每泵周期覆写；WSAPI `events.status` 消费）。
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct EventSnapshot {
    pub stats: ManagerStats,
    pub rules: Vec<RuleView>,
    pub pending: Vec<PendingView>,
}

/// 泵任务持有的全部状态（单线程所有权，无锁）。
pub struct ManagerState {
    pub rules: Vec<crate::rules::EventRule>,
    pub rules_mtime: Option<std::time::SystemTime>,
    pub pending: HashMap<String, MergeBuffer>,
    cooldowns: HashMap<String, CooldownGate>,
    hour_windows: HashMap<String, HourWindow>,
    global_window: HourWindow,
    global_hourly_cap: u32,
    pub stats: ManagerStats,
}

impl ManagerState {
    pub fn new(global_hourly_cap: u32) -> Self {
        Self {
            rules: Vec::new(),
            rules_mtime: None,
            pending: HashMap::new(),
            cooldowns: HashMap::new(),
            hour_windows: HashMap::new(),
            global_window: HourWindow::new(),
            global_hourly_cap,
            stats: ManagerStats::default(),
        }
    }

    /// 加载 + 校验规则。Err（读/解析失败）= 保留旧规则（半写文件不清空在监听
    /// 规则）；Ok = 应用新集（消失规则的 pending 诚实丢弃计数）。
    pub fn load_rules(&mut self, path: &Path) {
        match crate::rules::load_rules(path) {
            Err(e) => {
                tracing::warn!(
                    "[Events] rules reload failed, keeping {} old rule(s): {e}",
                    self.rules.len()
                );
            }
            Ok(loaded) => {
                for w in &loaded.warnings {
                    tracing::warn!("[Events] {w}");
                }
                let new_ids: HashSet<&str> = loaded.rules.iter().map(|r| r.id.as_str()).collect();
                let gone: Vec<String> = self
                    .pending
                    .keys()
                    .filter(|id| !new_ids.contains(id.as_str()))
                    .cloned()
                    .collect();
                for id in gone {
                    if let Some(b) = self.pending.remove(&id) {
                        self.stats.dropped_reload += b.into_count() as u64;
                    }
                }
                self.rules = loaded.rules;
                self.rules_mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
                self.stats.reloads += 1;
                tracing::info!(
                    "[Events] rules loaded: {} rule(s), {} warning(s)",
                    self.rules.len(),
                    loaded.warnings.len()
                );
            }
        }
    }

    /// mtime 变化检测（含文件删除）。变化 = 重载并返回 true（调用方 rearm）。
    pub fn check_reload(&mut self, path: &Path) -> bool {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if mtime == self.rules_mtime {
            return false;
        }
        self.load_rules(path);
        true
    }

    /// 当前全部监视根（rearm 差分基准）。
    pub fn watch_roots(&self) -> BTreeSet<PathBuf> {
        self.rules.iter().map(|r| r.watch.path.clone()).collect()
    }

    /// 观测快照（P2 EventsView；泵每周期调用覆写共享句柄）。
    pub fn snapshot(&self) -> EventSnapshot {
        EventSnapshot {
            stats: self.stats.clone(),
            rules: self
                .rules
                .iter()
                .map(|r| RuleView {
                    id: r.id.clone(),
                    kind: r.watch.kind.clone(),
                    path: r.watch.path.display().to_string(),
                    glob: r.watch.glob.clone(),
                    session_key: r.deliver.session_key.clone(),
                    enabled: r.deliver.enabled_or_default(),
                    cooldown_secs: r.deliver.cooldown_secs_or_default(),
                    merge_window_secs: r.deliver.merge_window_secs_or_default(),
                    max_per_hour: r.deliver.max_per_hour_or_default(),
                })
                .collect(),
            pending: self
                .pending
                .iter()
                .map(|(id, b)| PendingView {
                    rule_id: id.clone(),
                    count: b.count(),
                })
                .collect(),
        }
    }

    /// 一条 fs 事件：规则匹配 + 同路径 debounce 折叠 + 入 pending。
    pub fn ingest(&mut self, ev: RawFsEvent) {
        let path = PathBuf::from(&ev.path);
        let matched: Vec<String> = self
            .rules
            .iter()
            .filter(|r| crate::rules::rule_matches(r, &path))
            .map(|r| r.id.clone())
            .collect();
        for id in matched {
            let Some(rule) = self.rules.iter().find(|r| r.id == id) else {
                continue;
            };
            let debounce = rule.watch.debounce_ms_or_default();
            let entry = self
                .pending
                .entry(id)
                .or_insert_with(|| MergeBuffer::new(ev.at_ms));
            if entry.duplicate(&ev.path, ev.at_ms, debounce) {
                self.stats.deduped += 1;
                continue;
            }
            entry.push(&ev.path, ev.at_ms);
        }
    }

    /// 一个泵周期：estop 冻结臂 + 到期 buffer 过三闸投递。
    /// `now_ms` 由调用方传入（真实时钟 or 测试时钟）。
    pub fn run_cycle(&mut self, now_ms: u64, frozen: bool, injector: &dyn Fn(&str, &str) -> bool) {
        if frozen {
            let dropped: u64 = self.pending.values().map(|b| b.count() as u64).sum();
            if dropped > 0 {
                self.stats.dropped_estop += dropped;
                tracing::warn!(
                    "[Events] estop engaged: dropped {dropped} pending event(s) (release 后重新积累)"
                );
            }
            self.pending.clear();
            return;
        }
        let due: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.deliver.enabled_or_default())
            .filter(|r| {
                self.pending.get(&r.id).is_some_and(|b| {
                    b.should_flush(now_ms, r.deliver.merge_window_secs_or_default())
                })
            })
            .map(|r| r.id.clone())
            .collect();
        for id in due {
            let Some(rule) = self.rules.iter().find(|r| r.id == id) else {
                continue;
            };
            let Some(buf) = self.pending.remove(&id) else {
                continue;
            };
            let count = buf.count();
            let paths = buf.paths().to_vec();
            // 闸 1：per-rule 冷却（0=关）。冷却期内爆发不投递（丢弃计数，
            // 不排队不合并——项1 验收钦定语义）。
            let gate = self.cooldowns.entry(id.clone()).or_default();
            if !gate.allow(now_ms, rule.deliver.cooldown_secs_or_default()) {
                self.stats.dropped_cooldown += count as u64;
                tracing::debug!("[Events] rule {id}: cooldown active, dropped {count} event(s)");
                continue;
            }
            // 闸 2：per-rule 小时窗（0=不限）。
            let hw = self.hour_windows.entry(id.clone()).or_default();
            if !hw.allow(now_ms, rule.deliver.max_per_hour_or_default()) {
                self.stats.dropped_hourly += count as u64;
                tracing::warn!(
                    "[Events] rule {id}: per-rule hourly cap hit, dropped {count} event(s)"
                );
                continue;
            }
            // 闸 3：全局小时窗（跨规则合计；0=不限）。
            if !self.global_window.allow(now_ms, self.global_hourly_cap) {
                self.stats.dropped_hourly += count as u64;
                tracing::warn!(
                    "[Events] global hourly cap ({}) hit, dropped {count} event(s)",
                    self.global_hourly_cap
                );
                continue;
            }
            let rendered = crate::deliver::render_prompt(&rule.deliver.prompt, count, &paths);
            let content = crate::deliver::compose_content(&id, &rendered);
            if injector(&rule.deliver.session_key, &content) {
                // 冷却/小时窗只在成功投递时记账（诚实失败不消耗配额不启动冷却）。
                if let Some(gate) = self.cooldowns.get_mut(&id) {
                    gate.record(now_ms);
                }
                hw.record(now_ms);
                self.global_window.record(now_ms);
                self.stats.delivered_events += count as u64;
                self.stats.delivered_messages += 1;
                tracing::info!(
                    "[Events] rule {id}: delivered {count} event(s) -> {}",
                    rule.deliver.session_key
                );
            } else {
                self.stats.dropped_delivery += count as u64;
                tracing::warn!(
                    "[Events] rule {id}: delivery failed (session {} absent?), dropped {count} event(s)",
                    rule.deliver.session_key
                );
            }
        }
    }
}

/// 事件唤醒管理器。`start()` 拉起泵任务（gateway 生命周期；返回 JoinHandle
/// 供测试 abort / 停机 detach）。观测快照句柄在 `new()` 即创建——gateway
/// 可在 start 之前先把它注入 web 面（P2 EventsView）。
pub struct EventWatchManager {
    rules_path: PathBuf,
    injector: InjectorFn,
    is_frozen: FrozenFn,
    global_hourly_cap: u32,
    snapshot: Arc<parking_lot::RwLock<EventSnapshot>>,
}

impl EventWatchManager {
    pub fn new(rules_path: PathBuf, injector: InjectorFn, is_frozen: FrozenFn) -> Self {
        Self {
            rules_path,
            injector,
            is_frozen,
            global_hourly_cap: DEFAULT_GLOBAL_HOURLY_CAP,
            snapshot: Arc::new(parking_lot::RwLock::new(EventSnapshot::default())),
        }
    }

    /// 全局每小时上限覆盖（测试用；生产走默认 60）。
    pub fn with_global_hourly_cap(mut self, cap: u32) -> Self {
        self.global_hourly_cap = cap;
        self
    }

    /// 观测快照共享句柄（P2：gateway 注入 web AppState；泵每周期覆写）。
    pub fn snapshot_handle(&self) -> Arc<parking_lot::RwLock<EventSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    /// 规则文件路径（日志/UI 展示）。
    pub fn rules_path(&self) -> &Path {
        &self.rules_path
    }

    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<RawFsEvent>(EVENT_CHANNEL_CAP);
            let mut watcher = match FsWatcher::new(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("[Events] manager not started: {e}");
                    return;
                }
            };
            let mut st = ManagerState::new(self.global_hourly_cap);
            st.load_rules(&self.rules_path);
            watcher.rearm(&st.watch_roots());
            tracing::info!(
                "[Events] manager started (rules: {}, global cap {}/h)",
                self.rules_path.display(),
                self.global_hourly_cap
            );
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(PUMP_TICK_MS));
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    maybe = rx.recv() => match maybe {
                        Some(ev) => st.ingest(ev),
                        // watcher 被 drop（停机路径）：channel 关闭即退。
                        None => break,
                    },
                }
                if st.check_reload(&self.rules_path) {
                    watcher.rearm(&st.watch_roots());
                }
                let frozen = (self.is_frozen)();
                st.run_cycle(now_ms(), frozen, self.injector.as_ref());
                *self.snapshot.write() = st.snapshot();
            }
            watcher.shutdown();
            tracing::info!("[Events] manager stopped");
        })
    }
}

#[cfg(test)]
mod tests;
