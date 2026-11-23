//! EventWatchManager：规则热重载 + 多源事件泵 + 三闸 + 注入。
//!
//! 泵任务单线程持有全部状态（[`ManagerState`]），无锁——唯一共享面是
//! fs 事件 channel（watcher 回调 → 泵）、webhook channel（HTTP handler →
//! [`WebhookSubmitter`] → 泵）、probe 结果 channel（探测任务 → 泵）与两个
//! 闭包（注入 / 冻结判定），都由 gateway 装配侧提供（本 crate 不依赖
//! bus / estop）。
//!
//! 事件源（2026-11-16 扩展；cron 已砍单——nemesis-cron 已覆盖到点唤醒）：
//! - **fs**：notify 推送（OS 事件），watcher 臂；可选 `content_pattern`
//!   按变更文件新增行正则过滤（外部程序写的日志文件唯一过滤机制）；
//! - **webhook**：外部 POST（token 即凭据），[`WebhookMsg`] 臂；
//! - **proc**：后台任务输出行正则（[`ProcReaderFn`] 增量读，1s 采样轮询）；
//! - **sys**：CPU/内存/磁盘使用率阈值（sysinfo 2s 采样轮询 + 滞回）；
//! - **probe**：外部端点存活翻转（http/tcp/command，每规则独立周期；探测在
//!   独立 tokio 任务跑——网络 IO 不阻塞泵，结果经 channel 回泵应用状态机；
//!   状态翻转触发，首探只定基态；estop 冻结期暂停排程，在途回结果照常
//!   应用——投递冻结在 run_cycle 诚实丢弃）。
//!
//! 泵循环（~250ms 粒度）：
//! 1. `mtime` 热重载检查（变化才重载；解析失败**保留旧规则** + warn——
//!    半写文件不该把在监听的规则清空；proc/sys/probe/content 采样状态按
//!    watch 段对账保留）；
//! 2. ingest channel 里的 fs 事件 / webhook 推送 / probe 结果；
//! 3. proc（1s）/ sys（2s）/ probe 到期检查（1s）轮询臂；
//! 4. [`ManagerState::run_cycle`]：estop 冻结 → 丢弃全部 pending（诚实计数）；
//!    未冻结 → 到期 buffer 过三闸（冷却 / per-rule 小时窗 / 全局小时窗）→
//!    按 kind 渲染 → 注入闭包（false = 会话缺席，诚实失败留痕不重试排队）。

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

/// 后台任务输出增量读闭包：`(job_id, offset) -> JSON`（background_registry
/// `output()` 同款响应：`chunk` / `next_offset` / `running` / `command`）。
/// gateway 侧闭包持有 registry Arc；None（headless/测试）= proc 规则采样
/// 跳过（一次性 warn + stats 计数，诚实缺省）。
pub type ProcReaderFn = Arc<
    dyn Fn(
            u64,
            u64,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
        > + Send
        + Sync,
>;

/// webhook 提交消息（web handler → 泵）。`resp` 回传命中的规则 id 列表
///（空 = token 无规则认领，调用方诚实 404）。
pub struct WebhookMsg {
    pub token: String,
    pub payload: String,
    pub resp: tokio::sync::oneshot::Sender<Vec<String>>,
}

/// webhook 提交句柄（clone 后注入 web AppState；泵未运行 = submit 诚实报错）。
#[derive(Clone)]
pub struct WebhookSubmitter {
    tx: tokio::sync::mpsc::Sender<WebhookMsg>,
}

impl WebhookSubmitter {
    /// 从泵 channel sender 构造（EventWatchManager::webhook_handle 用）。
    pub fn from_sender(tx: tokio::sync::mpsc::Sender<WebhookMsg>) -> Self {
        Self { tx }
    }

    /// 提交外部推送。返回命中的规则 id（空 = 未匹配）。2s 超时防泵卡死拖住
    /// HTTP 请求（响应性优先：超时 = 事件已入队与否不确定，调用方 503）。
    pub async fn submit(&self, token: &str, payload: &str) -> Result<Vec<String>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WebhookMsg {
                token: token.to_string(),
                payload: payload.to_string(),
                resp: tx,
            })
            .await
            .map_err(|_| "events pump not running".to_string())?;
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx).await {
            Ok(Ok(matched)) => Ok(matched),
            Ok(Err(_)) => Err("events pump dropped the request".to_string()),
            Err(_) => Err("events pump timeout".to_string()),
        }
    }
}

const PUMP_TICK_MS: u64 = 250;
const EVENT_CHANNEL_CAP: usize = 1024;
const WEBHOOK_CHANNEL_CAP: usize = 256;
/// webhook 单事件载荷截断（字节）——合并消息体封顶（buffer 最多列 8 条）。
const WEBHOOK_PAYLOAD_MAX: usize = 1024;
/// proc 命中行截断（字符口径按字节，超长行截断防消息膨胀）。
const PROC_LINE_MAX: usize = 512;
/// proc 输出采样周期（泵 tick 数：4 × 250ms = 1s）。
const PROC_SAMPLE_EVERY: u32 = 4;
/// sys 采样周期（8 × 250ms = 2s；sysinfo CPU% 需 ≥200ms 双刷新间隔，2s 足够）。
const SYS_SAMPLE_EVERY: u32 = 8;
/// 磁盘挂载表刷新周期（40 × 250ms = 10s；挂载点变化极低频）。
const DISK_REFRESH_EVERY: u32 = 40;
/// probe 到期检查周期（4 × 250ms = 1s 粒度；周期下限 5s 见 rules）。
const PROBE_CHECK_EVERY: u32 = 4;
/// probe 结果 channel 容量（规则数级别，128 充裕）。
const PROBE_CHANNEL_CAP: usize = 128;
/// fs content_pattern 首见基线窗（字节）：只读尾部这段做初始匹配，不扫全文件。
const CONTENT_TAIL_MAX: u64 = 64 * 1024;
/// fs content_pattern 单事件增量读上限（字节）——超长追加分段消费，残段
/// 坐标留在 offset，下次事件续读。
const CONTENT_READ_CAP: u64 = 1024 * 1024;
/// 全局每小时投递上限（跨规则合计；0=不限）。规则级上限在 deliver.max_per_hour。
pub const DEFAULT_GLOBAL_HOURLY_CAP: u32 = 60;

/// sys 采样快照（泵线程内构建；sample_sys 消费）。
pub struct SysSample {
    /// 全局 CPU 使用率%（sysinfo 双刷新增量口径；首轮 0.0 基态）。
    pub cpu: f32,
    /// 全局内存使用率%。
    pub mem_percent: f64,
    /// (挂载点, 可用字节, 总量字节)。
    pub disks: Vec<(PathBuf, u64, u64)>,
}

/// proc 规则的泵内采样状态（单线程所有权）。
struct ProcWatchState {
    regex: regex::Regex,
    /// 已消费字节坐标（background_registry produced 空间单调推进）。
    offset: u64,
    /// 行边界残段（chunk 劈行时留给下一轮拼接）。
    tail: String,
    seen_running: bool,
}

/// sys 规则的滞回状态机（normal →(≥above) firing →(<below) normal）。
struct SysWatchState {
    firing: bool,
}

/// probe 规则的状态机：`up: None` = 基线未定（首探只定基态不触发）；
/// `generation` = 规则代次（热重载改 target/probe_type 重置代次，在途回结果按
/// 代次丢弃，防旧目标的迟到结果打翻新目标的基态）。
#[derive(Clone)]
struct ProbeWatchState {
    up: Option<bool>,
    generation: u64,
    last_ms: u64,
}

/// fs content_pattern 规则的已编译正则 + 逐路径消费坐标。
struct ContentRuleState {
    regex: regex::Regex,
    /// path → 已消费字节坐标（行边界对齐；残段不消费，下次事件续读）。
    offsets: HashMap<PathBuf, u64>,
}

/// 排程给探测任务的请求（probe_checks 产出；run_probe 消费）。
#[derive(Debug)]
pub(crate) struct ProbeRequest {
    pub rule_id: String,
    pub generation: u64,
    pub probe_type: String,
    pub target: String,
    pub timeout_secs: u64,
}

/// 探测结果（run_probe 产出；apply_probe 消费）。`detail` = 判定依据
/// 短语（"HTTP 200" / "连接超时" / "退出码 0"…），进翻转 payload。
#[derive(Debug)]
pub(crate) struct ProbeOutcome {
    pub rule_id: String,
    pub generation: u64,
    pub up: bool,
    pub detail: String,
}

/// 字节上限截断（char boundary 安全）+ 截断注记（诚实不静默吞字）。注记
/// 报**实际保留**字节数 `cut` 而非上限 `max`——多字节字符跨界回退后两者
/// 可差 1-3 字节，报 max 就是谎报（BUG-20261121-034，同 BUG-20261118-165
/// 「注记数字必须为真」纪律）。
fn truncate_annotated(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…[已截断 {}B→{}B]", &s[..cut], s.len(), cut)
}

/// disk 使用率解析（已用 % 口径，与 cpu/mem 统一）。`watch_path` 给定 =
/// 最长前缀挂载点匹配；None = 取使用率最高盘（任一盘过阈即算）；
/// 找不到匹配挂载点 = None（本轮跳过，不瞎猜）。
fn resolve_disk_percent(sample: &SysSample, watch_path: Option<&Path>) -> Option<f64> {
    let used_pct = |avail: u64, total: u64| -> f64 {
        if total == 0 {
            return 0.0;
        }
        (total.saturating_sub(avail)) as f64 * 100.0 / total as f64
    };
    match watch_path {
        None => Some(
            sample
                .disks
                .iter()
                .map(|(_, a, t)| used_pct(*a, *t))
                .fold(f64::MIN, f64::max),
        ),
        Some(p) => {
            let mut best: Option<(usize, u64, u64)> = None;
            for (mount, avail, total) in &sample.disks {
                if p.starts_with(mount) {
                    let len = mount.as_os_str().len();
                    if best.map(|(l, _, _)| len > l).unwrap_or(true) {
                        best = Some((len, *avail, *total));
                    }
                }
            }
            best.map(|(_, a, t)| used_pct(a, t))
        }
    }
}

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
    /// webhook 收到的推送总数（含未匹配 token）。
    pub webhook_received: u64,
    /// webhook token 未命中任何规则数（诚实 404 面）。
    pub webhook_unmatched: u64,
    /// proc 输出行正则命中总数（合并前的原始命中行数）。
    pub proc_lines_matched: u64,
    /// proc 读失败计数（job 缺席/淘汰；同规则只计一次）。
    pub proc_read_errors: u64,
    /// sys 触发次数（进 firing 的次数，含滞回后再触发）。
    pub sys_triggered: u64,
    /// probe 状态翻转次数（up→down 与 down→up 合计；首探定基态不计）。
    pub probe_flips: u64,
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
    /// kind 专属摘要（webhook 不回显 token；proc=任务+正则；sys=阈值档）。
    #[serde(default)]
    pub detail: String,
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
    /// sys 采样当前值（EventsView 展示面）。懒闸语义：无 sys 规则期间不采样
    /// = None（页面诚实显示缺省，不为展示面空转采样器）。
    #[serde(default)]
    pub sys_current: Option<SysCurrentView>,
}

/// sys 当前值展示投影（单盘）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SysDiskView {
    pub mount: String,
    pub used_percent: f64,
}

/// sys 采样当前值（泵每次采样覆写 `last_sys`；`events.status` 透传）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SysCurrentView {
    pub cpu: f32,
    pub mem_percent: f64,
    pub disks: Vec<SysDiskView>,
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
    /// proc 规则采样状态（规则 id → offset/tail/regex）。
    proc_state: HashMap<String, ProcWatchState>,
    /// sys 规则滞回状态（规则 id → firing）。
    sys_state: HashMap<String, SysWatchState>,
    /// proc 读失败已告警集合（同规则只 warn 一次，防刷日志）。
    proc_warned: HashSet<String>,
    /// probe 规则状态机（规则 id → 基态/代次/上次排程）。
    probe_state: HashMap<String, ProbeWatchState>,
    /// probe 规则代次计数器（重载改 watch 段时自增，在途回结果按代次丢弃）。
    probe_gen: u64,
    /// fs content_pattern 规则状态（规则 id → 正则 + 逐路径 offset）。
    content_state: HashMap<String, ContentRuleState>,
    /// content 读失败已告警集合（同规则只 warn 一次）。
    content_warned: HashSet<String>,
    /// sys 采样当前值（泵每次采样覆写；`snapshot()` 透传展示面）。
    pub last_sys: Option<SysCurrentView>,
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
            proc_state: HashMap::new(),
            sys_state: HashMap::new(),
            proc_warned: HashSet::new(),
            probe_state: HashMap::new(),
            probe_gen: 0,
            content_state: HashMap::new(),
            content_warned: HashSet::new(),
            last_sys: None,
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
                // proc/sys/probe/content 采样状态对账：watch 段未变的规则保留
                // 采样进度（offset 不回退 → 不重扫历史输出不重复触发），变更/
                // 消失的丢弃（新 pattern 重新编译、新阈值滞回基态从 normal 重置、
                // 新 probe 目标重新定基态——代次自增防在途回结果串台）。
                let old_proc = std::mem::take(&mut self.proc_state);
                let old_sys = std::mem::take(&mut self.sys_state);
                let old_probe = std::mem::take(&mut self.probe_state);
                let old_content = std::mem::take(&mut self.content_state);
                self.proc_warned.clear();
                self.content_warned.clear();
                for r in &loaded.rules {
                    if r.watch.kind == "proc"
                        && let Some(old) = old_proc.get(&r.id)
                        && old.regex.as_str() == r.watch.pattern.as_deref().unwrap_or("")
                    {
                        self.proc_state.insert(
                            r.id.clone(),
                            ProcWatchState {
                                regex: old.regex.clone(),
                                offset: old.offset,
                                tail: old.tail.clone(),
                                seen_running: old.seen_running,
                            },
                        );
                    }
                    if r.watch.kind == "sys"
                        && let Some(old) = old_sys.get(&r.id)
                        // watch 段（metric/above/below）未变才保留滞回基态；
                        // 阈值变更 = 语义变更，从 normal 重置重新武装。
                        // 此时 self.rules 仍是旧规则集（赋值在块尾）。
                        && let Some(old_rule) = self.rules.iter().find(|o| o.id == r.id)
                        && old_rule.watch.metric == r.watch.metric
                        && old_rule.watch.above == r.watch.above
                        && old_rule.watch.below == r.watch.below
                    {
                        self.sys_state
                            .insert(r.id.clone(), SysWatchState { firing: old.firing });
                    }
                    if r.watch.kind == "probe"
                        && let Some(old) = old_probe.get(&r.id)
                        // probe_type/target 未变才保留基态与代次（目标变更 =
                        // 新语义，重新定基态；generation 随 probe_gen 自增，旧
                        // 目标的在途回结果按代次丢弃）。
                        && let Some(old_rule) = self.rules.iter().find(|o| o.id == r.id)
                        && old_rule.watch.probe_type == r.watch.probe_type
                        && old_rule.watch.target == r.watch.target
                    {
                        self.probe_state.insert(r.id.clone(), old.clone());
                    }
                }
                self.rules = loaded.rules;
                // content offset 对账：坐标是文件字节位置、与 pattern 无关——
                // 规则仍是带 content_pattern 的 fs 规则就保留 offset（pattern
                // 变更不重扫历史，只换滤网：正则按当前 pattern 重编译）；
                // 规则消失/去掉了 content_pattern / 编译失败 = 条目丢弃。
                let mut carried_content = HashMap::new();
                for (id, old) in old_content.into_iter() {
                    if let Some(r) = self.rules.iter().find(|r| r.id == id)
                        && r.watch.kind == "fs"
                        && let Some(pattern) = r.watch.content_pattern.as_deref()
                        && let Ok(regex) = regex::Regex::new(pattern)
                    {
                        carried_content.insert(
                            id,
                            ContentRuleState {
                                regex,
                                offsets: old.offsets,
                            },
                        );
                    }
                }
                self.content_state = carried_content;
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
        // BUG-20261117-063：失败也要钉游标——mtime 是「已消费到哪个版本」
        // 的游标，不是「解析成功」的证明。此前 Err 臂不钉，坏文件（半写/
        // 手改坏）期间每个泵 tick（250ms）都 stat+read+parse+warn 一轮：
        // 4Hz 无限重试，warn 洪水经 sse_log_layer 直刷 Dashboard 实时事件
        // 流。钉住后只在文件再次变化（修复）时重试；半写竞态下编辑器完成
        // 写入必产生新 mtime，自愈路径保留。
        self.rules_mtime = mtime;
        true
    }

    /// 当前全部监视根（rearm 差分基准）。空路径剔除——webhook/probe/sys
    /// 规则的 watch.path 恒空（非 fs 源无目录可盯），不过滤会对空串调
    /// notify watch → 必然 Err → 每次生命周期一条假故障 WARN。
    pub fn watch_roots(&self) -> BTreeSet<PathBuf> {
        self.rules
            .iter()
            .map(|r| r.watch.path.clone())
            .filter(|p| !p.as_os_str().is_empty())
            .collect()
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
                    detail: match r.watch.kind.as_str() {
                        // webhook 不回显 token（规则表是观测面，token 是凭据）。
                        "webhook" => "外部 POST 推送（token 认领；载荷≤1KB）".to_string(),
                        "proc" => format!(
                            "job #{} pattern={:?}{}",
                            r.watch.job_id.map(|j| j.to_string()).unwrap_or_default(),
                            r.watch.pattern.as_deref().unwrap_or(""),
                            if r.watch.notify_start.unwrap_or(false) {
                                " +启动通知"
                            } else {
                                ""
                            }
                        ),
                        "sys" => format!(
                            "{} ≥ {}%（回落 < {}%）",
                            r.watch.metric.as_deref().unwrap_or("?"),
                            r.watch
                                .above
                                .map(|v| format!("{v:.0}"))
                                .unwrap_or_else(|| "?".into()),
                            r.watch
                                .sys_below_or_default()
                                .map(|v| format!("{v:.0}"))
                                .unwrap_or_else(|| "?".into())
                        ),
                        "probe" => format!(
                            "{} {} 每 {}s（超时 {}s）",
                            r.watch.probe_type.as_deref().unwrap_or("?"),
                            r.watch.target.as_deref().unwrap_or("?"),
                            r.watch.probe_interval_or_default(),
                            r.watch.probe_timeout_or_default()
                        ),
                        _ => String::new(),
                    },
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
            sys_current: self.last_sys.clone(),
        }
    }

    /// 统一 pending 入队（多源共用）。`payload` 语义随源：fs=变更路径，
    /// webhook=外部载荷（截断后），proc=命中行/启动通知，sys=预渲染摘要。
    /// `debounce_ms` > 0 时同载荷去重折叠（fs 语义）；0 = 不折叠
    /// （webhook/proc/sys：相同载荷是不同事件，逐条计数）。
    fn push_pending(&mut self, rule_id: &str, payload: &str, now_ms: u64, debounce_ms: u64) {
        let entry = self
            .pending
            .entry(rule_id.to_string())
            .or_insert_with(|| MergeBuffer::new(now_ms));
        if debounce_ms > 0 && entry.duplicate(payload, now_ms, debounce_ms) {
            self.stats.deduped += 1;
            return;
        }
        entry.push(payload, now_ms);
    }

    /// 一条 fs 事件：规则匹配 + 可选 content_pattern 新增行过滤 + 同路径
    /// debounce 折叠 + 入 pending。
    pub fn ingest(&mut self, ev: RawFsEvent) {
        let path = PathBuf::from(&ev.path);
        let matched: Vec<String> = self
            .rules
            .iter()
            .filter(|r| crate::rules::rule_matches(r, &path))
            .map(|r| r.id.clone())
            .collect();
        for id in matched {
            let (content_pattern, debounce) = match self.rules.iter().find(|r| r.id == id) {
                Some(rule) => (
                    rule.watch.content_pattern.clone(),
                    rule.watch.debounce_ms_or_default(),
                ),
                None => continue,
            };
            // content_pattern 闸：新增段不命中 = 静默丢弃（路径变化本身不算）。
            if content_pattern.is_some()
                && !self.fs_content_passes(&id, content_pattern.as_deref().unwrap_or(""), &path)
            {
                continue;
            }
            self.push_pending(&id, &ev.path, ev.at_ms, debounce);
        }
    }

    /// webhook 推送消费（泵侧）。按 token 匹配规则（同 token 可挂多规则），
    /// 载荷截断后逐规则入 pending。返回命中的规则 id 列表（空 = token 无
    /// 规则认领，HTTP 面 404）。estop 冻结不影响匹配/应答——投递冻结在
    /// run_cycle 诚实丢弃（与 fs 事件同语义）。
    pub fn ingest_webhook(&mut self, token: &str, payload: &str, now_ms: u64) -> Vec<String> {
        self.stats.webhook_received += 1;
        let matched: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.watch.kind == "webhook" && r.watch.token.as_deref() == Some(token))
            .map(|r| r.id.clone())
            .collect();
        if matched.is_empty() {
            self.stats.webhook_unmatched += 1;
            return matched;
        }
        let payload = truncate_annotated(payload, WEBHOOK_PAYLOAD_MAX);
        for id in &matched {
            self.push_pending(id, &payload, now_ms, 0);
        }
        matched
    }

    /// fs content_pattern 闸（ingest 调用；调用前保证 kind=fs 且
    /// content_pattern 在场）。读变更文件的**新增段**（首见/截断 = 尾部基线窗
    /// [`CONTENT_TAIL_MAX`]），行边界对齐后逐行正则匹配；命中 = 放行。
    /// 语义锚点：
    /// - 坐标按文件字节推进，只消费到最后一个 `\n`——残段不消费留给下次
    ///   事件拼接（半行不漏判不重判）；
    /// - 基线窗内无行边界 = 坐标停在窗起点（残段累积，等下一个 \n 到齐
    ///   一起匹配——首见死循环不可能：offset 已记，下次走增量臂）；
    /// - 文件截断（len < offset，日志轮转）= 重新基线；
    /// - 新增段含 NUL = 按二进制跳过（坐标照常推进，不反复读）；
    /// - 读失败 = 一次性 warn + 本轮不触发（诚实跳过，不瞎放行）。
    fn fs_content_passes(&mut self, rule_id: &str, pattern: &str, path: &Path) -> bool {
        // 惰性建状态（新规则首见；正则 validate 加载期已编译过一次，此处
        // 再编译一次换免锁缓存——事件粒度 µs 级，不设独立编译缓存面）。
        if !self.content_state.contains_key(rule_id) {
            let Ok(regex) = regex::Regex::new(pattern) else {
                return false; // 防御分支（validate 拦过）
            };
            self.content_state.insert(
                rule_id.to_string(),
                ContentRuleState {
                    regex,
                    offsets: HashMap::new(),
                },
            );
        }
        let len = match std::fs::metadata(path) {
            Ok(m) => m.len(),
            Err(e) => {
                if self.content_warned.insert(rule_id.to_string()) {
                    tracing::warn!(
                        "[Events] fs rule {rule_id}: content 文件 {} 不可读（{e}），本轮不触发",
                        path.display()
                    );
                }
                return false;
            }
        };
        // 相一：起点决策（不可变借 state）。
        let (start, prev_offset, baseline) = {
            let st = self
                .content_state
                .get(rule_id)
                .expect("state just inserted");
            match st.offsets.get(path) {
                Some(&o) if len < o => (len.saturating_sub(CONTENT_TAIL_MAX), o, true), // 截断重基线
                Some(&o) => (o, o, false),
                None => (len.saturating_sub(CONTENT_TAIL_MAX), 0, true), // 首见基线窗
            }
        };
        if !baseline && len == prev_offset {
            return false; // 无新增字节（元数据变化类事件），不触发
        }
        let end = len.min(start + CONTENT_READ_CAP);
        // 相二：读新增段（不持 state 借用）。
        let bytes = (|| -> std::io::Result<Vec<u8>> {
            use std::io::{Read, Seek};
            let mut f = std::fs::File::open(path)?;
            f.seek(std::io::SeekFrom::Start(start))?;
            let mut buf = vec![0u8; (end - start) as usize];
            f.read_exact(&mut buf)?;
            Ok(buf)
        })();
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                if self.content_warned.insert(rule_id.to_string()) {
                    tracing::warn!(
                        "[Events] fs rule {rule_id}: content 文件 {} 读段失败（{e}），本轮不触发",
                        path.display()
                    );
                }
                return false;
            }
        };
        // 相三：行边界对齐 + 匹配 + 推进坐标（可变借 state）。
        let st = self
            .content_state
            .get_mut(rule_id)
            .expect("state just inserted");
        if bytes.contains(&0) {
            // 二进制：跳过但坐标推进到本段末尾（不反复读同一段）。
            st.offsets.insert(path.to_path_buf(), end);
            return false;
        }
        let Some(pos) = bytes.iter().rposition(|&b| b == b'\n') else {
            // 全段无行边界（半行）：基线 = 坐标停在窗起点等行到齐；增量 =
            // 坐标原地不动（下次续读拼接）。
            if baseline {
                st.offsets.insert(path.to_path_buf(), start);
            }
            return false;
        };
        let consume_end = pos + 1; // 消费到 \n（含）
        let matched = bytes[..consume_end]
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| {
                String::from_utf8_lossy(l)
                    .trim_end_matches('\r')
                    .to_string()
            })
            .any(|s| st.regex.is_match(&s));
        st.offsets
            .insert(path.to_path_buf(), start + consume_end as u64);
        matched
    }

    /// probe 到期排程（每 [`PROBE_CHECK_EVERY`] tick 调用）。返回本 tick 到期
    /// 的探测请求（调用方 spawn [`run_probe`]）。冻结期返回空（estop 暂停
    /// 排程，release 恢复；在途回结果照常应用——投递冻结归 run_cycle）。
    /// 惰性建状态：首条排程定代次（`last_ms=0` = 首探立即出发）。
    pub(crate) fn probe_checks(&mut self, now_ms: u64, frozen: bool) -> Vec<ProbeRequest> {
        if frozen {
            return Vec::new();
        }
        let ids: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.watch.kind == "probe" && r.deliver.enabled_or_default())
            .map(|r| r.id.clone())
            .collect();
        let mut reqs = Vec::new();
        for id in ids {
            let Some(rule) = self.rules.iter().find(|r| r.id == id) else {
                continue;
            };
            let interval_ms = rule.watch.probe_interval_or_default() * 1000;
            if !self.probe_state.contains_key(&id) {
                self.probe_gen += 1;
                self.probe_state.insert(
                    id.clone(),
                    ProbeWatchState {
                        up: None,
                        generation: self.probe_gen,
                        last_ms: 0,
                    },
                );
            }
            let generation = self.probe_state.get(&id).map(|s| s.generation).unwrap_or(0);
            let due = self
                .probe_state
                .get(&id)
                .is_some_and(|st| now_ms.saturating_sub(st.last_ms) >= interval_ms);
            if !due {
                continue;
            }
            if let Some(st) = self.probe_state.get_mut(&id) {
                st.last_ms = now_ms;
            }
            reqs.push(ProbeRequest {
                rule_id: id,
                generation,
                probe_type: rule.watch.probe_type.clone().unwrap_or_default(),
                target: rule.watch.target.clone().unwrap_or_default(),
                timeout_secs: rule.watch.probe_timeout_or_default(),
            });
        }
        reqs
    }

    /// 应用探测结果（泵 select 臂）。规则消失 / 代次过期（目标变更后旧目标
    /// 的在途结果）= 诚实丢弃。首探（基态 None）只定基态不触发；与基态不同
    /// = 翻转 → 预渲染摘要进 pending（`{payload}`）+ 统计。
    pub(crate) fn apply_probe(&mut self, out: ProbeOutcome, now_ms: u64) {
        let Some(st) = self.probe_state.get_mut(&out.rule_id) else {
            return;
        };
        if st.generation != out.generation {
            return; // 旧代次在途结果（目标已变更），丢弃
        }
        let Some(prev) = st.up else {
            st.up = Some(out.up); // 首探定基态
            return;
        };
        if prev == out.up {
            return; // 同态心跳
        }
        st.up = Some(out.up);
        self.stats.probe_flips += 1;
        let target = self
            .rules
            .iter()
            .find(|r| r.id == out.rule_id)
            .and_then(|r| r.watch.target.clone())
            .unwrap_or_default();
        let word = |u: bool| if u { "UP" } else { "DOWN" };
        let payload = format!(
            "{target} 探测翻转：{} → {}（{}）",
            word(prev),
            word(out.up),
            out.detail
        );
        self.push_pending(&out.rule_id, &payload, now_ms, 0);
    }

    /// proc 源采样（每 [`PROC_SAMPLE_EVERY`] tick）：逐规则经 [`ProcReaderFn`]
    /// 增量读后台任务输出（`output(id, offset)` 公开 API，零 registry 侵入）
    /// → 劈行 → 正则命中 → pending（debounce=0，每行独立事件）。
    /// `notify_start`：首次观测到任务运行中补一条启动通知（与命中行同 buffer
    /// 合并投递）。job 缺席/读失败 = 规则静默 idle + 一次性 warn + stats 计数。
    pub async fn sample_procs(&mut self, reader: Option<&ProcReaderFn>, now_ms: u64) {
        let Some(reader) = reader else {
            return;
        };
        let proc_ids: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.watch.kind == "proc" && r.watch.job_id.is_some())
            .map(|r| r.id.clone())
            .collect();
        for id in proc_ids {
            let Some(rule) = self.rules.iter().find(|r| r.id == id) else {
                continue;
            };
            let job_id = rule.watch.job_id.unwrap_or(0);
            let notify_start = rule.watch.notify_start.unwrap_or(false);
            // 惰性建采样状态（新规则 / 热重载后 pattern 变更 → offset 从 0
            // 重扫：新 pattern 下历史输出里已存在的命中也算数，诚实不漏）。
            if !self.proc_state.contains_key(&id) {
                let pattern = rule.watch.pattern.clone().unwrap_or_default();
                match regex::Regex::new(&pattern) {
                    Ok(regex) => {
                        self.proc_state.insert(
                            id.clone(),
                            ProcWatchState {
                                regex,
                                offset: 0,
                                tail: String::new(),
                                seen_running: false,
                            },
                        );
                    }
                    // validate 加载期已拦；防御分支不采样。
                    Err(_) => continue,
                }
            }
            // 读 + 解析（持有 state 借用一次完成），释放借用后再入队。
            let outcome: Result<(Vec<String>, u64, bool), String> = {
                let state = self.proc_state.get_mut(&id).expect("state just inserted");
                match (reader)(job_id, state.offset).await {
                    Err(e) => Err(e),
                    Ok(v) => {
                        let chunk = v.get("chunk").and_then(|x| x.as_str()).unwrap_or("");
                        let next_offset = v
                            .get("next_offset")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(state.offset);
                        let running = v.get("running").and_then(|x| x.as_bool()).unwrap_or(false);
                        // 劈行：running 任务末段可能是半行（后续 chunk 补全）
                        // → 留残段；任务已结束 → 末段按完整行消费。
                        let mut text = std::mem::take(&mut state.tail);
                        text.push_str(chunk);
                        let mut parts: Vec<String> = text
                            .split('\n')
                            .map(|s| s.trim_end_matches('\r').to_string())
                            .collect();
                        let tail = if running {
                            parts.pop().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let hits: Vec<String> = parts
                            .iter()
                            .filter(|l| !l.is_empty())
                            .filter(|l| state.regex.is_match(l))
                            .map(|l| truncate_annotated(l, PROC_LINE_MAX))
                            .collect();
                        let start_notify = notify_start && running && !state.seen_running;
                        if start_notify {
                            state.seen_running = true;
                        }
                        state.tail = tail;
                        Ok((hits, next_offset, start_notify))
                    }
                }
            };
            match outcome {
                Err(e) => {
                    self.stats.proc_read_errors += 1;
                    if self.proc_warned.insert(id.clone()) {
                        tracing::warn!(
                            "[Events] proc rule {id}: 后台任务 #{job_id} 读失败（不存在或已淘汰?），规则静默 idle：{e}"
                        );
                    }
                }
                Ok((hits, next_offset, start_notify)) => {
                    self.proc_warned.remove(&id);
                    if let Some(st) = self.proc_state.get_mut(&id) {
                        st.offset = next_offset;
                    }
                    if start_notify {
                        self.push_pending(&id, &format!("[后台任务 #{job_id} 已启动]"), now_ms, 0);
                    }
                    if !hits.is_empty() {
                        self.stats.proc_lines_matched += hits.len() as u64;
                        for line in hits {
                            self.push_pending(&id, &line, now_ms, 0);
                        }
                    }
                }
            }
        }
    }

    /// sys 源采样（每 [`SYS_SAMPLE_EVERY`] tick）：滞回状态机——normal 且
    /// value ≥ above → firing + 推一次触发摘要；firing 期 value < below →
    /// 回 normal（静默，不推）；下次再过 above 才再触发（防抖）。
    /// disk 挂载点解析：`watch.path` 给定 = 最长前缀匹配挂载点；缺省 =
    /// 取使用率最高盘（任一盘过阈即算）。找不到挂载点 = 本轮跳过。
    pub fn sample_sys(&mut self, sample: &SysSample, now_ms: u64) {
        let specs: Vec<(String, String, f64, f64, Option<PathBuf>)> = self
            .rules
            .iter()
            .filter(|r| r.watch.kind == "sys")
            .filter_map(|r| {
                let above = r.watch.above?;
                let below = r.watch.sys_below_or_default().unwrap_or(above - 10.0);
                Some((
                    r.id.clone(),
                    r.watch.metric.clone().unwrap_or_default(),
                    above,
                    below,
                    // disk 且显式给了定位路径才走挂载点匹配；空 path = None
                    // （任一盘过阈语义），不能把 Some(空路径) 传下去——空路径
                    // 匹配不到任何挂载点会被误判成「跳过」。
                    if r.watch.metric.as_deref() == Some("disk")
                        && !r.watch.path.as_os_str().is_empty()
                    {
                        Some(r.watch.path.clone())
                    } else {
                        None
                    },
                ))
            })
            .collect();
        for (id, metric, above, below, disk_path) in specs {
            let value = match metric.as_str() {
                "cpu" => Some(sample.cpu as f64),
                "mem" => Some(sample.mem_percent),
                "disk" => resolve_disk_percent(sample, disk_path.as_deref()),
                _ => None,
            };
            let Some(value) = value else {
                continue;
            };
            let was = self.sys_state.get(&id).map(|s| s.firing).unwrap_or(false);
            let now_firing = if was { value >= below } else { value >= above };
            if now_firing && !was {
                self.sys_state
                    .insert(id.clone(), SysWatchState { firing: true });
                self.stats.sys_triggered += 1;
                let payload = format!("{metric} 使用率 {value:.1}%（触发阈值 ≥ {above:.0}%）");
                self.push_pending(&id, &payload, now_ms, 0);
            } else if !now_firing && was {
                self.sys_state
                    .insert(id.clone(), SysWatchState { firing: false });
                tracing::debug!(
                    "[Events] sys rule {id}: {metric} 回落 {value:.1}%（< {below:.0}%），回 normal 可再触发"
                );
            } else if !self.sys_state.contains_key(&id) {
                // 首轮基态落账（未触发也记 normal，滞回从这一刻起算）。
                self.sys_state
                    .insert(id.clone(), SysWatchState { firing: now_firing });
            }
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
            // 渲染按 kind 分派：fs 走原路径（字节不变，prompt cache/既有
            // 测试锚定）；proc 带 {line}/{job_id}；webhook/sys 的 buffer
            // 载荷即 {payload}（外部原文 / 预渲染摘要）。
            let rendered = match rule.watch.kind.as_str() {
                "fs" => crate::deliver::render_prompt(&rule.deliver.prompt, count, &paths),
                "proc" => crate::deliver::render_prompt_vars(
                    &rule.deliver.prompt,
                    count,
                    &[
                        ("line", paths.join("\n")),
                        (
                            "job_id",
                            rule.watch.job_id.map(|j| j.to_string()).unwrap_or_default(),
                        ),
                    ],
                ),
                _ => crate::deliver::render_prompt_vars(
                    &rule.deliver.prompt,
                    count,
                    &[("payload", paths.join("\n"))],
                ),
            };
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
/// 可在 start 之前先把它注入 web 面（P2 EventsView）。webhook 提交句柄
/// 同理（`webhook_handle()`，clone 共享同一泵 channel）。
pub struct EventWatchManager {
    rules_path: PathBuf,
    injector: InjectorFn,
    is_frozen: FrozenFn,
    global_hourly_cap: u32,
    snapshot: Arc<parking_lot::RwLock<EventSnapshot>>,
    webhook_tx: tokio::sync::mpsc::Sender<WebhookMsg>,
    webhook_rx: Option<tokio::sync::mpsc::Receiver<WebhookMsg>>,
    probe_tx: tokio::sync::mpsc::Sender<ProbeOutcome>,
    probe_rx: Option<tokio::sync::mpsc::Receiver<ProbeOutcome>>,
    proc_reader: Option<ProcReaderFn>,
    /// 进程终态令牌（BUG-20261115-023 家族纪律；BUG-20261119-208 同款 DI
    /// 形态）。None（缺省/测试装配）= never-cancelled 默认令牌，泵 select
    /// 令牌臂恒 pending，行为与旧版逐字节一致。
    terminal: Option<tokio_util::sync::CancellationToken>,
    /// BUG-20261123-037：command 探测子进程 cwd 锚点（gateway 装配注入
    /// workspace 根）。None = 继承进程 cwd（旧行为）——gateway 部署形态
    /// cwd 不可预测，相对路径探测命令随部署形态漂移（false-alert 风险）。
    working_dir: Option<PathBuf>,
}

impl EventWatchManager {
    pub fn new(rules_path: PathBuf, injector: InjectorFn, is_frozen: FrozenFn) -> Self {
        let (webhook_tx, webhook_rx) = tokio::sync::mpsc::channel(WEBHOOK_CHANNEL_CAP);
        let (probe_tx, probe_rx) = tokio::sync::mpsc::channel(PROBE_CHANNEL_CAP);
        Self {
            rules_path,
            injector,
            is_frozen,
            global_hourly_cap: DEFAULT_GLOBAL_HOURLY_CAP,
            snapshot: Arc::new(parking_lot::RwLock::new(EventSnapshot::default())),
            webhook_tx,
            webhook_rx: Some(webhook_rx),
            probe_tx,
            probe_rx: Some(probe_rx),
            proc_reader: None,
            terminal: None,
            working_dir: None,
        }
    }

    /// 全局每小时上限覆盖（测试用；生产走默认 60）。
    pub fn with_global_hourly_cap(mut self, cap: u32) -> Self {
        self.global_hourly_cap = cap;
        self
    }

    /// proc 源增量读闭包（gateway 装配：闭包持 background_registry Arc）。
    /// 缺省 = proc 规则采样跳过（一次性 warn，headless/测试常态）。
    pub fn with_proc_reader(mut self, reader: ProcReaderFn) -> Self {
        self.proc_reader = Some(reader);
        self
    }

    /// 进程终态令牌（gateway 装配传 `terminal_cancel_token().clone()`）：
    /// 泵 select 令牌臂优先于 tick——cancel 后不再 re-park timer，排空窗内
    /// 不采样不注入（家族纪律 BUG-20261115-023；DI 形态同 nemesis-web
    /// uploads/snooze sweeper，BUG-20261119-208）。缺省 = never-cancelled，
    /// 行为与旧版一致。
    pub fn with_terminal_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.terminal = Some(token);
        self
    }

    /// command 探测子进程 cwd 锚点（BUG-20261123-037；gateway 装配传
    /// workspace 根）。None = 继承进程 cwd（旧行为）。
    pub fn with_working_dir(mut self, working_dir: Option<PathBuf>) -> Self {
        self.working_dir = working_dir;
        self
    }

    /// webhook 提交句柄（gateway 注入 web AppState；多 clone 共享泵 channel）。
    pub fn webhook_handle(&self) -> WebhookSubmitter {
        WebhookSubmitter::from_sender(self.webhook_tx.clone())
    }

    /// 观测快照共享句柄（P2：gateway 注入 web AppState；泵每周期覆写）。
    pub fn snapshot_handle(&self) -> Arc<parking_lot::RwLock<EventSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    /// 规则文件路径（日志/UI 展示）。
    pub fn rules_path(&self) -> &Path {
        &self.rules_path
    }

    pub fn start(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<RawFsEvent>(EVENT_CHANNEL_CAP);
            let mut watcher = match FsWatcher::new(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("[Events] manager not started: {e}");
                    return;
                }
            };
            let mut webhook_rx = self.webhook_rx.take().unwrap_or_else(|| {
                // new() 必建 channel；防御分支（start 不可达路径）。
                let (t, r) = tokio::sync::mpsc::channel(1);
                drop(t);
                r
            });
            let mut probe_rx = self.probe_rx.take().unwrap_or_else(|| {
                // 同上防御分支。
                let (t, r) = tokio::sync::mpsc::channel(1);
                drop(t);
                r
            });
            // 探测结果回程 channel 的发送端（spawn 出去的探测任务各持克隆）。
            let probe_tx = self.probe_tx.clone();
            let mut webhook_alive = true;
            let mut probe_alive = true;
            // probe HTTP 共享客户端（连接池复用；per-request 超时在探测任务
            // 内 tokio::time::timeout 包裹）。
            let probe_client = reqwest::Client::new();
            if self.proc_reader.is_none() {
                tracing::warn!(
                    "[Events] 无 ProcReader 装配：proc 规则采样跳过（headless/测试装配常态）"
                );
            }
            let mut st = ManagerState::new(self.global_hourly_cap);
            st.load_rules(&self.rules_path);
            watcher.rearm(&st.watch_roots());
            // sys 采样器（泵线程独占）：CPU% 增量口径靠连续两次 refresh 的
            // 间隔（采样周期 2s，足够）；磁盘挂载表 10s 刷新。
            let mut sys = sysinfo::System::new();
            // 带初始快照构造（Disks::new() 是空列表构造器——空表期间 disk 规则
            // 解析 None 静默跳过，首条 sys 规则装载后最长 10s 不触发）。
            let mut disks = sysinfo::Disks::new_with_refreshed_list();
            tracing::info!(
                "[Events] manager started (rules: {}, global cap {}/h)",
                self.rules_path.display(),
                self.global_hourly_cap
            );
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(PUMP_TICK_MS));
            // 家族纪律等待点（BUG-20261115-023）：None 装配（测试常态）回落
            // never-cancelled 默认令牌——令牌臂恒 pending，select 形态与旧版
            // 一致；gateway 装配传真令牌，cancel 后排空窗内不再采样不注入。
            let terminal = self.terminal.take().unwrap_or_default();
            let mut proc_tick: u32 = 0;
            let mut sys_tick: u32 = 0;
            let mut disk_tick: u32 = 0;
            let mut probe_tick: u32 = 0;
            loop {
                // 前置 is_cancelled：预取消零工作，确定性退出不与停机竞速。
                if terminal.is_cancelled() {
                    break;
                }
                tokio::select! {
                    _ = tick.tick() => {}
                    maybe = rx.recv() => match maybe {
                        Some(ev) => st.ingest(ev),
                        // watcher 被 drop（停机路径）：channel 关闭即退。
                        None => break,
                    },
                    maybe_w = webhook_rx.recv(), if webhook_alive => match maybe_w {
                        Some(msg) => {
                            let matched = st.ingest_webhook(&msg.token, &msg.payload, now_ms());
                            let _ = msg.resp.send(matched);
                        }
                        None => {
                            // 提交端全部 drop（gateway 停机/测试收尾）：webhook
                            // 臂停选，fs/proc/sys/probe 泵继续（投递面不受影响）。
                            webhook_alive = false;
                        }
                    },
                    // 令牌臂优先于 tick：cancel 即退出走下方 watcher.shutdown
                    // 收尾（与 rx 臂 None 同一优雅路径）。
                    _ = terminal.cancelled() => break,
                    maybe_p = probe_rx.recv(), if probe_alive => match maybe_p {
                        Some(out) => st.apply_probe(out, now_ms()),
                        None => {
                            // 全部探测任务结束且 sender drop（停机/测试收尾）：
                            // probe 臂停选，其余泵继续。
                            probe_alive = false;
                        }
                    },
                }
                if st.check_reload(&self.rules_path) {
                    watcher.rearm(&st.watch_roots());
                }
                proc_tick += 1;
                if proc_tick >= PROC_SAMPLE_EVERY {
                    proc_tick = 0;
                    st.sample_procs(self.proc_reader.as_ref(), now_ms()).await;
                }
                sys_tick += 1;
                if sys_tick >= SYS_SAMPLE_EVERY {
                    sys_tick = 0;
                    // 懒闸：无 sys 规则不采样（不空转刷计数器）。首条 sys 规则
                    // 热载入后的首个采样值是距上次刷新的间隔均值——滞回语义下
                    // 至多一次过冲触发，随后回落回 normal，诚实自校正。
                    if st.rules.iter().any(|r| r.watch.kind == "sys") {
                        sys.refresh_cpu_usage();
                        let cpu = sys.global_cpu_usage();
                        sys.refresh_memory();
                        let total = sys.total_memory().max(1);
                        let mem_percent = sys.used_memory() as f64 * 100.0 / total as f64;
                        disk_tick += 1;
                        if disk_tick >= DISK_REFRESH_EVERY {
                            disk_tick = 0;
                            // sysinfo 0.33：refresh(true) = 重列挂载点 + 刷新数值，
                            // 已消失挂载点移除。
                            disks.refresh(true);
                        }
                        let disk_list: Vec<(PathBuf, u64, u64)> = disks
                            .list()
                            .iter()
                            .map(|d| {
                                (
                                    d.mount_point().to_path_buf(),
                                    d.available_space(),
                                    d.total_space(),
                                )
                            })
                            .collect();
                        let sample = SysSample {
                            cpu,
                            mem_percent,
                            disks: disk_list,
                        };
                        // 展示面投影（events.status sys_current；used% 口径与
                        // 阈值判定同源）。
                        st.last_sys = Some(SysCurrentView {
                            cpu: sample.cpu,
                            mem_percent: sample.mem_percent,
                            disks: sample
                                .disks
                                .iter()
                                .map(|(m, a, t)| SysDiskView {
                                    mount: m.display().to_string(),
                                    used_percent: if *t == 0 {
                                        0.0
                                    } else {
                                        t.saturating_sub(*a) as f64 * 100.0 / *t as f64
                                    },
                                })
                                .collect(),
                        });
                        st.sample_sys(&sample, now_ms());
                    }
                }
                // probe 到期检查（PROBE_CHECK_EVERY=4 tick ≈ 1s 粒度）：调度侧
                // 只算 due 并 spawn 探测任务——网络 IO 全部在独立任务里，泵线程
                // 永不阻塞；结果经 channel 回到 probe_rx 臂（apply_probe）。
                // frozen 只暂停调度（在途结果照常入账，投递面 run_cycle 冻结），
                // 与 webhook estop 语义同源。
                probe_tick += 1;
                if probe_tick >= PROBE_CHECK_EVERY {
                    probe_tick = 0;
                    let frozen_now = (self.is_frozen)();
                    for req in st.probe_checks(now_ms(), frozen_now) {
                        let tx = probe_tx.clone();
                        let client = probe_client.clone();
                        let cwd = self.working_dir.clone();
                        tokio::spawn(run_probe(req, client, tx, cwd));
                    }
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

/// probe 探测执行体（独立 tokio 任务——网络 IO / 子进程等待不占泵线程）。
/// 结果经 channel 交回泵（probe_rx 臂 → apply_probe 消费）；sender 全部
/// drop = probe 臂停选（停机路径）。未知 probe_type 防御性静默（加载期
/// 校验已拦，运行期不可达）。
pub(crate) async fn run_probe(
    req: ProbeRequest,
    client: reqwest::Client,
    tx: tokio::sync::mpsc::Sender<ProbeOutcome>,
    cwd: Option<PathBuf>,
) {
    let (up, detail) = match req.probe_type.as_str() {
        "http" => http_probe(&client, &req.target, req.timeout_secs).await,
        "tcp" => tcp_probe(&req.target, req.timeout_secs).await,
        "command" => command_probe(&req.target, req.timeout_secs, cwd.as_deref()).await,
        _ => return,
    };
    let _ = tx
        .send(ProbeOutcome {
            rule_id: req.rule_id,
            generation: req.generation,
            up,
            detail,
        })
        .await;
}

/// http 探测：up = 2xx。超时 / 连接失败 = down，detail 带根因（翻转载荷
/// 里可见）。
async fn http_probe(client: &reqwest::Client, target: &str, timeout_secs: u64) -> (bool, String) {
    let fut = client.get(target).send();
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Err(_) => (false, format!("超时（{timeout_secs}s）")),
        Ok(Err(e)) => (false, format!("请求失败：{e}")),
        Ok(Ok(resp)) => (
            resp.status().is_success(),
            format!("HTTP {}", resp.status()),
        ),
    }
}

/// tcp 探测：up = 连接成功（端口可达即存活，无数据交换）。
async fn tcp_probe(target: &str, timeout_secs: u64) -> (bool, String) {
    let fut = tokio::net::TcpStream::connect(target);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Err(_) => (false, format!("超时（{timeout_secs}s）")),
        Ok(Err(e)) => (false, format!("连接失败：{e}")),
        Ok(Ok(_)) => (true, "连接成功".to_string()),
    }
}

/// command 探测：up = 退出码 0。target 按空白切 program+args（不经 shell；
/// 带空格路径的引号形态不支持，管理员配置面文档声明）。kill_on_drop：探测
/// 任务被丢弃（停机）时子进程跟着死，不留孤儿进程。stdio 全 null——探测
/// 只关心退出码，不给目标进程管道背压机会。`cwd`（BUG-20261123-037）=
/// 装配注入的 workspace 根锚点：相对路径探测命令按它解析，不随 gateway
/// 部署形态漂移；None = 继承进程 cwd（旧行为）。
async fn command_probe(
    target: &str,
    timeout_secs: u64,
    cwd: Option<&std::path::Path>,
) -> (bool, String) {
    let mut parts = target.split_whitespace();
    let Some(program) = parts.next() else {
        return (false, "空命令".to_string());
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(parts);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.kill_on_drop(true);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    // env 窄版闸（BUG-20261117-048，家族闸纪律对齐 cli_delegation）：探测
    // 命令是管理员配置面 spawn 点，不继承 bot 内部命名空间变量；窄剥保留
    // 第三方自身 auth env。
    let _stripped = nemesis_utils::env_sanitize::sanitize_tokio_command_narrow(&mut cmd);
    // 黑窗根修（BUG-20261117-048，对齐 formatter.rs 同因注释）：gateway 无
    // console 运行（release windows 子系统）时 spawn console 探测程序会
    // 分配可见新窗口——探测 stdio 全 null，压窗零副作用。
    //（tokio Command 在 Windows 原生自带 creation_flags，无需 std CommandExt）
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let fut = cmd.output();
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fut).await {
        Err(_) => (false, format!("超时（{timeout_secs}s）")),
        Ok(Err(e)) => (false, format!("启动失败：{e}")),
        Ok(Ok(out)) => match out.status.code() {
            Some(0) => (true, "退出码 0".to_string()),
            Some(code) => (false, format!("退出码 {code}")),
            None => (false, "被信号终止".to_string()),
        },
    }
}

#[cfg(test)]
mod tests;
