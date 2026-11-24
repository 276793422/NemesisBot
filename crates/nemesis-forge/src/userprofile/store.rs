//! 用户习惯画像存储（自 nemesis-agent/user_profile.rs 迁入 + 结构化升级，
//! 2026-11-24 Forge 化重构批一/批二）。
//!
//! 存储：`<workspace>/config/user_profile.json`（**独立于 persona 文件**——
//! persona 激活会整文件覆盖 USER.md，习惯绝不能放 USER.md）。
//!
//! 纪律（v3.1 计划裁决，原样保留）：
//! - 生成**免确认**自动生效（置信度 ≥0.7）；Dashboard 展示 + veto/forget/pin 后门；
//! - **硬边界**：习惯只调整风格与流程姿态，绝不触碰权限与安全语义——
//!   渲染层过滤含「自动批准/放行/降档」语义的条目（防御性双保险，生成侧
//!   同样拒绝），审批放行只走 approval pattern 机制；
//! - **仅向前不回溯**：记录 `enabled_at`，只分析其后的会话数据；
//! - 默认关：`user_profile.enabled`（config）；衰减：30 天无重现 decaying、
//!   再 30 天 expired；条目上限 10（按置信度截断）。
//!
//! 结构化升级（批二，G4/G8）：
//! - 条目增 `steps`（仪式步骤数组）/`scope`（global/project）/`evidence`
//!   （溯源）/`auto`（固定规则产物标记）——serde default 向后兼容，老
//!   条目缺字段 = 旧形态单句条目原样可读；
//! - `auto=true` 条目（统计规则/确定性挖掘器固定产物）单独配额
//!   [`AUTO_MAX_ENTRIES`]，不再与反思通道细粒度习惯挤同一批名额（G8）；
//! - 渲染升级：带 steps 条目 = 步骤清单块（步骤同样过安全边界 + 单行折叠）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 习惯条目状态。
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DECAYING: &str = "decaying";
pub const STATUS_EXPIRED: &str = "expired";
pub const STATUS_VETOED: &str = "vetoed";

/// 置信度过线阈值（config 可调，缺省 0.7）。
pub const CONFIDENCE_THRESHOLD: f64 = 0.7;
/// 注入条目上限（非 auto 桶）。
pub const MAX_ENTRIES: usize = 10;
/// auto 条目（统计规则/挖掘器固定产物）单独配额（G8 名额挤压根修）。
pub const AUTO_MAX_ENTRIES: usize = 3;
/// 衰减天数（active→decaying；decaying→expired 同值再段）。
pub const DECAY_DAYS: i64 = 30;
/// 单条目 steps 上限（仪式步骤数组）。
pub const MAX_STEPS: usize = 12;
/// 单个 step 字符上限。
pub const MAX_STEP_CHARS: usize = 120;
/// evidence 溯源条目上限。
pub const MAX_EVIDENCE: usize = 8;
/// 条目 content 字符上限（单行纪律）。
pub const MAX_CONTENT_CHARS: usize = 200;

/// 条目主语分型（批 A 双向接口）：`habit` = 用户习惯（主语=用户，注入面
/// 唯一合法值）；`lesson` = 助手/LLM 教训（主语=agent——**永不入画像**，
/// 反思解析层分流到 [`super::bridge`]，upsert 入口硬拒双保险）。
pub const KIND_HABIT: &str = "habit";
pub const KIND_LESSON: &str = "lesson";

/// 习惯条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HabitEntry {
    /// 类别：workflow | correction | tool_preference | style | environment | quality_bar
    pub category: String,
    /// 习惯陈述（注入 system prompt 的原文）。
    pub content: String,
    /// 置信度 0.0-1.0。
    pub confidence: f64,
    /// 信号命中次数。
    #[serde(default)]
    pub hits: u32,
    #[serde(default)]
    pub first_seen: String,
    #[serde(default)]
    pub last_seen: String,
    #[serde(default = "default_status")]
    pub status: String,
    /// pin = 用户钉住：衰减 sweep 跳过，渲染置顶。
    #[serde(default)]
    pub pinned: bool,
    /// 仪式步骤数组（结构化升级批二；缺字段 = 旧形态单句条目）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    /// 作用域（空串 = 全局；预留 `project:<id>` 形态）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// 溯源（支撑会话 / todo 文件 / 遥测日期）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// 固定产物标记：true = 统计规则/确定性挖掘器每轮稳定再生成的条目
    /// （单独配额 [`AUTO_MAX_ENTRIES`]，不与反思条目挤名额——G8）。
    #[serde(default)]
    pub auto: bool,
    /// 主语分型（批 A）：`"habit"`（缺省，serde default——老条目/旧反思
    /// 响应零迁移）/ `"lesson"`（助手教训——upsert 硬拒，见
    /// [`upsert_habit`]）。
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 来源标记（批 A 接口二）：空 = 自产（本系统挖掘/反思）；非空 =
    /// 外部抛入（Forge 经验，经 [`ingest_external_entry`] 强制标记）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

fn default_status() -> String {
    STATUS_ACTIVE.to_string()
}

fn default_kind() -> String {
    KIND_HABIT.to_string()
}

impl HabitEntry {
    /// 挖掘器/规则通道便捷构造（auto=true，status=active，无 steps）。
    pub fn auto_entry(
        category: &str,
        content: &str,
        confidence: f64,
        hits: u32,
        now: &str,
    ) -> Self {
        Self {
            category: category.to_string(),
            content: content.to_string(),
            confidence,
            hits,
            first_seen: now.to_string(),
            last_seen: now.to_string(),
            status: STATUS_ACTIVE.into(),
            pinned: false,
            steps: Vec::new(),
            scope: String::new(),
            evidence: Vec::new(),
            auto: true,
            kind: KIND_HABIT.into(),
            source: String::new(),
        }
    }

    /// 单行清洗（content 与每个 step 通用）：换行折叠为空格 + 截断。
    pub fn fold_one_line(s: &str, max_chars: usize) -> String {
        s.replace(['\r', '\n'], " ")
            .chars()
            .take(max_chars)
            .collect()
    }

    /// 步骤数组清洗：单行折叠 + 截断 + 条数上限。
    pub fn sanitize_steps(steps: &[String]) -> Vec<String> {
        steps
            .iter()
            .map(|s| Self::fold_one_line(s, MAX_STEP_CHARS))
            .filter(|s| !s.trim().is_empty())
            .take(MAX_STEPS)
            .collect()
    }
}

/// 用户画像（user_profile.json 根）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserProfile {
    /// 开关打开时刻（RFC3339；仅向前锚点。None = 从未开启）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_at: Option<String>,
    /// 上次 sweep 的采集水线（epoch 毫秒 = 本轮 collect 开始时刻）。增量
    /// 采集语义：下轮只处理 timestamp >= 水线的消息——缺失时全量重扫会把
    /// 同一批老消息每轮重新 upsert（hits 虚增 + last_seen 永刷新 → 衰减
    /// 永不触发）。None = 从未 sweep（回落 enabled_at 锚点）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sweep_ms: Option<i64>,
    #[serde(default)]
    pub habits: Vec<HabitEntry>,
    /// 上次 LLM 反思的水线（epoch 毫秒；G1 重构——反思触发改为
    /// 「窗内新增用户消息 ≥ reflect_min_new_messages」或 sweep 计数到顶，
    /// 本水线界定「新增」的起点；None = 从未反思，回落 sweep 窗口）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reflect_ms: Option<i64>,
    /// 距上次反思的 sweep 计数（G1 根修：持久化进画像文件——重启不再
    /// 归零，频繁重启的设备反思通道也能到达触发阈值）。
    #[serde(default)]
    pub sweeps_since_reflect: u32,
}

/// 统计通道证据窗起点（epoch 毫秒；滚动 24h，二轮复查定稿）：
///
/// `window_start = max(min(上次 sweep 水线, now-24h), enabled_at 锚点)`
///
/// - **首 sweep**：`min(锚点, now-24h)` 通常落锚点之前，`max` 夹回锚点
///   （仅向前不回溯）；启用已超 24h 的存量用户首 sweep 也只扫近 24h
///   （不回溯挖陈年历史）；
/// - **稳态**：滚动 24h——统计规则（/ 的 n≥10 闸）需要足够大的样本
///   窗，小时级增量批对稀疏用户永远凑不齐；
/// - **停机补漏**：水线停在几天前 → 窗口下探到水线（补齐停机期证据，
///   锚点兜底防越界）；
/// - **时钟偏移**：水线在未来 → `min` 取滚动界，不扫未来。
///
/// 锚点缺失/损坏 = None（调用方跳过本轮采集）——`last_sweep_ms` 继续由
/// sweep 推进，角色 = 停机补漏下探边界。
pub const SWEEP_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

pub fn sweep_window_start_ms(profile: &UserProfile, now_ms: i64) -> Option<i64> {
    let anchor = profile
        .enabled_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())?;
    let rolling = now_ms.saturating_sub(SWEEP_WINDOW_MS);
    let lower = match profile.last_sweep_ms {
        Some(w) => w.min(rolling),
        None => rolling,
    };
    Some(lower.max(anchor))
}

/// 画像文件路径（唯一拼接点）。
pub fn profile_path(workspace: &Path) -> PathBuf {
    workspace.join("config").join("user_profile.json")
}

/// 读取画像（**写路径禁用**）：缺失 = 空画像；损坏 = Err 带路径。
///
/// 只读展示类调用方可用 [`load_profile`] 的容错形态；**任何读取后回写
/// 的路径（sweep/veto/forget/pin/enable）必须用本函数**——损坏静默当空
/// 再 save 会把全量习惯清空（四路评审 B- 根修）。
pub fn try_load_profile(workspace: &Path) -> Result<UserProfile, String> {
    let path = profile_path(workspace);
    // read_config_string 剥 BOM（裸读时记事本 BOM → parse 失败 → 写路径
    // 全拒绝 / 读路径静默空画像，BUG-20261103-334 同族）。
    let content = match nemesis_utils::fsutil::read_config_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(UserProfile::default()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    serde_json::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// 读取画像（缺失/损坏 = 空画像；不 panic 不误伤）。
///
/// ⚠ 仅限纯展示/统计读取；读后回写的调用方必须走 [`try_load_profile`]。
pub fn load_profile(workspace: &Path) -> UserProfile {
    try_load_profile(workspace).unwrap_or_default()
}

/// 写画像（config 家族写串行锁；原子落盘）。
pub fn save_profile(workspace: &Path, profile: &UserProfile) -> Result<(), String> {
    let _serial = nemesis_config::config_write_lock();
    save_profile_locked(workspace, profile)
}

/// [`save_profile`] 的锁内版：调用方**已持** [`nemesis_config::config_write_lock`]
/// 时使用（锁不可重入——锁内函数再走 `save_profile` 会自死锁；四路评审
/// B- 根修配套）。
pub fn save_profile_locked(workspace: &Path, profile: &UserProfile) -> Result<(), String> {
    let path = profile_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    let content =
        serde_json::to_string_pretty(profile).map_err(|e| format!("serialize profile: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| format!("write profile: {e}"))
}

/// 开启画像（记录 enabled_at——仅向前锚点；幂等：已开启不改动）。
///
/// `max_entries` = 注入条目上限，**由调用方从 config 现读传入**
/// （`UserProfileConfig::max_entries_or(MAX_ENTRIES)`）——sweep 侧按同一
/// config 值截断（此前本函数内嵌硬编码 [`prune_and_cap`]
/// (=10)，`max_entries` 配 >10 的用户 disable→enable 循环后习惯被静默
/// 截回 10，与配置矛盾）。
pub fn enable_profile(workspace: &Path, max_entries: usize) -> Result<UserProfile, String> {
    // RMW 全程持锁（锁外读改写会丢并发写入；save_profile 锁不可重入，
    // 内层走 save_profile_locked）。
    let _serial = nemesis_config::config_write_lock();
    let mut profile = try_load_profile(workspace)?;
    if profile.enabled_at.is_none() {
        profile.enabled_at = Some(chrono::Local::now().to_rfc3339());
    }
    prune_and_cap_with_max(&mut profile, max_entries.max(1));
    save_profile_locked(workspace, &profile)?;
    Ok(profile)
}

/// 渲染层过滤：拒绝任何含权限/安全语义的习惯内容（硬边界防御性双保险）。
pub(crate) fn violates_security_boundary(content: &str) -> bool {
    let lower = content.to_lowercase();
    const FORBIDDEN: &[&str] = &[
        "自动批准",
        "自动放行",
        "免审批",
        "跳过审批",
        "自动 approve",
        "auto approve",
        "auto-approve",
        "降档",
        "降低安全",
        "绕过审批",
        "skip approval",
        "bypass approval",
    ];
    FORBIDDEN.iter().any(|k| lower.contains(k))
}

/// 上榜前清洗：剔除 expired + 安全违规条目；**vetoed 条目保留驻留**（二轮
/// 复查根修：负样本压制语义——vetoed 一旦被 prune 删除，规则通道的固定
/// 字符串候选会在下一轮 sweep 原样重新生成，否决失效；驻留时 upsert 命中
/// 同内容条目保持 vetoed 态，天然抑制重生成）；条数上限分桶计数（G8）：
/// 非 auto 条目按置信度截 [`MAX_ENTRIES`]（config 可调），auto 条目
/// （固定规则/挖掘器产物）单独截 [`AUTO_MAX_ENTRIES`]，vetoed 不占任何
/// 名额。pinned 条目优先保留（用户意志不被截断）。
pub fn prune_and_cap(profile: &mut UserProfile) {
    prune_and_cap_with_max(profile, MAX_ENTRIES);
}

/// [`prune_and_cap`](fn@prune_and_cap) 的可调上限版（config 消费点：
/// `user_profile.max_entries`，经 `UserProfileConfig::max_entries_or` 回落
/// [`MAX_ENTRIES`]）。
pub fn prune_and_cap_with_max(profile: &mut UserProfile, max_entries: usize) {
    profile
        .habits
        .retain(|h| h.status != STATUS_EXPIRED && !violates_security_boundary(&h.content));
    let (kept, vetoed): (Vec<_>, Vec<_>) = profile
        .habits
        .drain(..)
        .partition(|h| h.status != STATUS_VETOED);
    let (mut manual, mut auto): (Vec<_>, Vec<_>) = kept.into_iter().partition(|h| !h.auto);
    // 各桶内 pinned 优先（用户意志不被截断），其次置信度。
    let sort_key = |a: &HabitEntry, b: &HabitEntry| {
        b.pinned.cmp(&a.pinned).then_with(|| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };
    manual.sort_by(|a, b| sort_key(a, b));
    auto.sort_by(|a, b| sort_key(a, b));
    manual.truncate(max_entries);
    auto.truncate(AUTO_MAX_ENTRIES);
    profile.habits = manual;
    profile.habits.extend(auto);
    profile.habits.extend(vetoed);
}

/// upsert：语义相同（同 category + 内容归一相等）取高置信度、hits+1、
/// 刷新 last_seen；不同则插入。vetoed 条目命中时**保持 vetoed**（仅
/// decaying→active 转换）——与 `prune_and_cap` 的驻留语义配套构成否决
/// 抑制链：驻留 + 不复活 = 同内容候选永不重生成。
///
/// 结构化字段合并（批二）：steps 取并集（去重保序、清洗截断）——反思
/// 通道对同一仪式的步骤细化能累积进既有条目；evidence 同并集（溯源
/// 累积）；auto 取 OR（一旦被挖掘器命中过，配额归 auto 桶）。
pub fn upsert_habit(profile: &mut UserProfile, mut entry: HabitEntry) {
    let norm = |s: &str| s.trim().to_lowercase();
    // 双保险（批 A）：非 habit 主语分型一律拒入——lesson 即便被反思解析层
    // 误路由（应分流到 bridge 队列），也进不了用户画像注入面。未知分型
    // 同拒（fail-closed：注入面语义只有用户习惯一种）。
    if entry.kind != KIND_HABIT {
        return;
    }
    if violates_security_boundary(&entry.content) {
        return; // 硬边界：安全语义候选直接丢弃
    }
    for existing in profile.habits.iter_mut() {
        if existing.category == entry.category && norm(&existing.content) == norm(&entry.content) {
            // max 后钳制：LLM 反思通道候选可能带 >1.0 的越界置信度（四路
            // 评审 B- 根修）。
            existing.confidence = existing.confidence.max(entry.confidence).clamp(0.0, 1.0);
            existing.hits += 1;
            if !entry.last_seen.is_empty() {
                existing.last_seen = entry.last_seen;
            }
            if existing.status == STATUS_DECAYING {
                existing.status = STATUS_ACTIVE.to_string();
            }
            // 结构化合并：steps 并集（保序去重 + 清洗）。
            for s in HabitEntry::sanitize_steps(&entry.steps) {
                if !existing.steps.iter().any(|e| e == &s) {
                    existing.steps.push(s);
                }
            }
            existing.steps = HabitEntry::sanitize_steps(&existing.steps);
            // evidence 并集（溯源累积，超限保尾部）：新证据比旧证据信息量
            // 高（近期行为更能代表当前习惯），头部淘汰而非尾部截断——尾部
            // 截断会把刚并入的新溯源当溢出项丢掉。
            for ev in &entry.evidence {
                if !existing.evidence.iter().any(|e| e == ev) {
                    existing.evidence.push(ev.clone());
                }
            }
            if existing.evidence.len() > MAX_EVIDENCE {
                let start = existing.evidence.len() - MAX_EVIDENCE;
                existing.evidence.drain(..start);
            }
            existing.auto = existing.auto || entry.auto;
            return;
        }
    }
    entry.confidence = entry.confidence.clamp(0.0, 1.0);
    entry.steps = HabitEntry::sanitize_steps(&entry.steps);
    entry.evidence.truncate(MAX_EVIDENCE);
    profile.habits.push(entry);
}

/// 外部经验抛入（批 A 接口二：Forge → userprofile）。`source` **强制**
/// 来源标记（空 = Err——外部知识必须可溯源到抛入方；自产条目 source 为
/// 空，不走本函数）。
///
/// 纪律：`kind` 强制归一 [`KIND_HABIT`]（外部抛入的是「关于用户的经验」；
/// lesson 语义走 [`super::bridge`] 队列，方向相反）；`auto` 强制 false
/// （进反思配额桶，不占挖掘器固定配额）；合并语义与自产完全一致（同
/// content 并 evidence、置信度取高、保尾溯源——内部走 [`upsert_habit`]，
/// 安全边界过滤与 lesson 双保险同源生效）。合并命中已有自产条目时保留
/// 原条目 source（本地证据链优先，外部来源不覆盖自产标记）。
pub fn ingest_external_entry(
    workspace: &Path,
    mut entry: HabitEntry,
    source: &str,
) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("ingest 来源标记为空（外部经验必须可溯源到抛入方），拒绝执行".into());
    }
    entry.kind = KIND_HABIT.into();
    entry.auto = false;
    entry.source = source.to_string(); // 强制：参数是权威，不透传调用方残留值
    // RMW 全程持锁（veto/forget 同族纪律）；损坏画像 = Err 不回写。
    let _serial = nemesis_config::config_write_lock();
    let mut profile = try_load_profile(workspace)?;
    upsert_habit(&mut profile, entry);
    save_profile_locked(workspace, &profile)
}

/// 衰减 sweep：last_seen 距今 >decay_days 天 → decaying；>2×decay_days →
/// expired。`now` 由调用方传入（RFC3339），便于单测注入时钟；`decay_days`
/// 缺省 [`DECAY_DAYS`]（config `user_profile.decay_days` 可调）。
pub fn decay_sweep(profile: &mut UserProfile, now: chrono::DateTime<chrono::Local>) {
    decay_sweep_with_days(profile, now, DECAY_DAYS);
}

/// [`decay_sweep`](fn@decay_sweep) 的可调天数版（config 消费点）。
pub fn decay_sweep_with_days(
    profile: &mut UserProfile,
    now: chrono::DateTime<chrono::Local>,
    decay_days: i64,
) {
    for h in profile.habits.iter_mut() {
        if h.status == STATUS_VETOED || h.status == STATUS_EXPIRED || h.pinned {
            continue;
        }
        let Ok(seen) = chrono::DateTime::parse_from_rfc3339(&h.last_seen) else {
            continue;
        };
        let days = (now - seen.with_timezone(&chrono::Local)).num_days();
        // 受检乘法（BUG-20261107-025；同族先例：BUG-20261107-017 turn-wal
        // TTL、spill.rs retention saturating_mul）——decay_days 是 config
        // 来源 i64（`decay_days_or` 只挡 0/负，无上界），`*2` 溢出在 debug
        // panic / release 环绕为负 → 全部条目误判 expired 被 prune 清空
        // （静默毁数据）。饱和语义 = 「衰减门槛荒谬大 → 永不衰减」（诚实
        // 极端值行为）。
        let two_windows = decay_days.saturating_mul(2);
        if days > two_windows {
            h.status = STATUS_EXPIRED.to_string();
        } else if days > decay_days {
            h.status = STATUS_DECAYING.to_string();
        }
    }
}

/// 注入段渲染（agent loop::messages merged snapshot 的输入；空 = 未开启
/// 或无 active 条目，零字节注入）。content 换行折叠为空格（单行纪律）。
///
/// 结构化升级（批二）：带 steps 条目渲染为步骤清单块（步骤单行折叠 +
/// 安全边界过滤）；无 steps 条目保持旧单行形态（字节兼容既有注入段，
/// prompt cache 前缀在内容不变期间稳定）。
pub fn render_section(profile: &UserProfile) -> String {
    let mut lines = Vec::new();
    let mut entries: Vec<&HabitEntry> = profile
        .habits
        .iter()
        .filter(|h| h.status == STATUS_ACTIVE || h.status == STATUS_DECAYING)
        .filter(|h| !violates_security_boundary(&h.content))
        .collect();
    entries.sort_by_key(|h| std::cmp::Reverse(h.pinned));
    for h in entries {
        let pin_mark = if h.pinned { " [pinned]" } else { "" };
        // 换行折叠：content 若夹带 \r\n 会伪造注入行结构（四路评审
        // B- 根修——反思维护内容统一压成单行）。
        let one_line = h.content.replace(['\r', '\n'], " ");
        lines.push(format!("- [{}] {}{pin_mark}", h.category, one_line));
        // 步骤清单块（结构化条目）：步骤同样单行折叠 + 安全边界过滤。
        for (i, step) in h
            .steps
            .iter()
            .filter(|s| !violates_security_boundary(s))
            .enumerate()
        {
            let step_line = step.replace(['\r', '\n'], " ");
            lines.push(format!("  {}. {}", i + 1, step_line));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n")
}

/// veto：按内容子串匹配置 vetoed（负样本：驻留抑制——prune 保留 vetoed
/// 条目，upsert 命中同内容保持 vetoed，重生成被压制；详见
/// [`prune_and_cap`]）。返回是否命中。空子串 = Err（`contains("")` 恒真
/// 会全量命中，CLI/WSAPI 双入口共用此防线）。匹配大小写不敏感。
pub fn veto_habit(workspace: &Path, content_sub: &str) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("veto 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    // RMW 全程持锁（四路评审 B- 根修）；损坏画像 = Err 不回写。
    let _serial = nemesis_config::config_write_lock();
    let mut p = try_load_profile(workspace)?;
    let needle = content_sub.to_lowercase();
    let mut hit = false;
    for h in p.habits.iter_mut() {
        if h.content.to_lowercase().contains(&needle) {
            h.status = STATUS_VETOED.to_string();
            hit = true;
        }
    }
    if hit {
        save_profile_locked(workspace, &p)?;
    }
    Ok(hit)
}

/// forget：按内容子串整条删除（与 veto 不同：删除后同语义候选可重新生成）。
/// 空子串 = Err（同 [`veto_habit`] 防线）。匹配大小写不敏感。
pub fn forget_habit(workspace: &Path, content_sub: &str) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("forget 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let _serial = nemesis_config::config_write_lock();
    let mut p = try_load_profile(workspace)?;
    let needle = content_sub.to_lowercase();
    let before = p.habits.len();
    p.habits
        .retain(|h| !h.content.to_lowercase().contains(&needle));
    let removed = p.habits.len() < before;
    if removed {
        save_profile_locked(workspace, &p)?;
    }
    Ok(removed)
}

/// pin / unpin：按内容子串切换钉住（钉住 = 不衰减、渲染置顶）。
/// 空子串 = Err（同 [`veto_habit`] 防线）。匹配大小写不敏感。
pub fn set_pinned(workspace: &Path, content_sub: &str, pinned: bool) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("pin 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let _serial = nemesis_config::config_write_lock();
    let mut p = try_load_profile(workspace)?;
    let needle = content_sub.to_lowercase();
    let mut hit = false;
    for h in p.habits.iter_mut() {
        if h.content.to_lowercase().contains(&needle) {
            h.pinned = pinned;
            hit = true;
        }
    }
    if hit {
        save_profile_locked(workspace, &p)?;
    }
    Ok(hit)
}

#[cfg(test)]
mod tests;
