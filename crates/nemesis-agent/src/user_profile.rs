//! 用户习惯画像（自进化升级 B 线，2026-10-14）。
//!
//! 存储：`<workspace>/config/user_profile.json`（**独立于 persona 文件**——
//! persona 激活会整文件覆盖 USER.md，习惯绝不能放 USER.md）。
//!
//! 纪律（v3.1 计划裁决）：
//! - 生成**免确认**自动生效（置信度 ≥0.7）；Dashboard 展示 + veto/forget/pin 后门；
//! - **硬边界**：习惯只调整风格与流程姿态，绝不触碰权限与安全语义——
//!   渲染层过滤含「自动批准/放行/降档」语义的条目（防御性双保险，生成侧
//!   同样拒绝），审批放行只走 approval pattern 机制；
//! - **仅向前不回溯**：记录 `enabled_at`，只分析其后的会话数据；
//! - 默认关：`user_profile.enabled`（config）；衰减：30 天无重现 decaying、
//!   再 30 天 expired；条目上限 10（按置信度截断）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 习惯条目状态。
pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_DECAYING: &str = "decaying";
pub const STATUS_EXPIRED: &str = "expired";
pub const STATUS_VETOED: &str = "vetoed";

/// 置信度过线阈值（config 可调，缺省 0.7）。
pub const CONFIDENCE_THRESHOLD: f64 = 0.7;
/// 注入条目上限。
pub const MAX_ENTRIES: usize = 10;
/// 衰减天数（active→decaying；decaying→expired 同值再段）。
pub const DECAY_DAYS: i64 = 30;

/// 习惯条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HabitEntry {
    /// 类别：workflow | correction | tool_preference | style
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
}

fn default_status() -> String {
    STATUS_ACTIVE.to_string()
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
}

/// 统计通道证据窗起点（epoch 毫秒；滚动 24h，二轮复查定稿）：
///
/// `window_start = max(min(上次 sweep 水线, now-24h), enabled_at 锚点)`
///
/// - **首 sweep**：`min(锚点, now-24h)` 通常落锚点之前，`max` 夹回锚点
///   （仅向前不回溯）；启用已超 24h 的存量用户首 sweep 也只扫近 24h
///   （不回溯挖陈年历史）；
/// - **稳态**：滚动 24h——统计规则（R1/R2 的 n≥10 闸）需要足够大的样本
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

/// 读取画像（缺失/损坏 = 空画像；不 panic 不误伤）。
pub fn load_profile(workspace: &Path) -> UserProfile {
    let path = profile_path(workspace);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 写画像（config 家族写串行锁；原子落盘）。
pub fn save_profile(workspace: &Path, profile: &UserProfile) -> Result<(), String> {
    let _serial = nemesis_config::config_write_lock();
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
pub fn enable_profile(workspace: &Path) -> Result<UserProfile, String> {
    let mut profile = load_profile(workspace);
    if profile.enabled_at.is_none() {
        profile.enabled_at = Some(chrono::Local::now().to_rfc3339());
    }
    prune_and_cap(&mut profile);
    save_profile(workspace, &profile)?;
    Ok(profile)
}

/// 渲染层过滤：拒绝任何含权限/安全语义的习惯内容（硬边界防御性双保险）。
fn violates_security_boundary(content: &str) -> bool {
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
/// 同内容条目保持 vetoed 态，天然抑制重生成）；条数上限只数非 vetoed
/// 条目（按置信度截断），vetoed 不占活跃名额。
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
    let (active, vetoed): (Vec<_>, Vec<_>) = profile
        .habits
        .drain(..)
        .partition(|h| h.status != STATUS_VETOED);
    let mut active = active;
    active.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    active.truncate(max_entries);
    profile.habits = active;
    profile.habits.extend(vetoed);
}

/// upsert：语义相同（同 category + 内容归一相等）取高置信度、hits+1、
/// 刷新 last_seen；不同则插入。vetoed 条目命中时**保持 vetoed**（仅
/// decaying→active 转换）——与 `prune_and_cap` 的驻留语义配套构成否决
/// 抑制链：驻留 + 不复活 = 同内容候选永不重生成。
pub fn upsert_habit(profile: &mut UserProfile, mut entry: HabitEntry) {
    let norm = |s: &str| s.trim().to_lowercase();
    if violates_security_boundary(&entry.content) {
        return; // 硬边界：安全语义候选直接丢弃
    }
    for existing in profile.habits.iter_mut() {
        if existing.category == entry.category && norm(&existing.content) == norm(&entry.content) {
            existing.confidence = existing.confidence.max(entry.confidence);
            existing.hits += 1;
            if !entry.last_seen.is_empty() {
                existing.last_seen = entry.last_seen;
            }
            if existing.status == STATUS_DECAYING {
                existing.status = STATUS_ACTIVE.to_string();
            }
            return;
        }
    }
    entry.confidence = entry.confidence.clamp(0.0, 1.0);
    profile.habits.push(entry);
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
        if days > decay_days * 2 {
            h.status = STATUS_EXPIRED.to_string();
        } else if days > decay_days {
            h.status = STATUS_DECAYING.to_string();
        }
    }
}

/// 注入段渲染（ContextBuilder::set_user_profile_section 的输入）。
/// 空 = 未开启或无 active 条目（零字节注入）。
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
        lines.push(format!("- [{}] {}{pin_mark}", h.category, h.content));
    }
    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n")
}

/// veto：按内容子串匹配置 vetoed（负样本：驻留抑制——prune 保留 vetoed
/// 条目，upsert 命中同内容保持 vetoed，重生成被压制；详见
/// [`prune_and_cap`]）。返回是否命中。空子串 = Err（`contains("")` 恒真
/// 会全量命中，CLI/WSAPI 双入口共用此防线）。
pub fn veto_habit(workspace: &Path, content_sub: &str) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("veto 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let mut p = load_profile(workspace);
    let mut hit = false;
    for h in p.habits.iter_mut() {
        if h.content.contains(content_sub) {
            h.status = STATUS_VETOED.to_string();
            hit = true;
        }
    }
    if hit {
        save_profile(workspace, &p)?;
    }
    Ok(hit)
}

/// forget：按内容子串整条删除（与 veto 不同：删除后同语义候选可重新生成）。
/// 空子串 = Err（同 [`veto_habit`] 防线）。
pub fn forget_habit(workspace: &Path, content_sub: &str) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("forget 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let mut p = load_profile(workspace);
    let before = p.habits.len();
    p.habits.retain(|h| !h.content.contains(content_sub));
    let removed = p.habits.len() < before;
    if removed {
        save_profile(workspace, &p)?;
    }
    Ok(removed)
}

/// pin / unpin：按内容子串切换钉住（钉住 = 不衰减、渲染置顶）。
/// 空子串 = Err（同 [`veto_habit`] 防线）。
pub fn set_pinned(workspace: &Path, content_sub: &str, pinned: bool) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("pin 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let mut p = load_profile(workspace);
    let mut hit = false;
    for h in p.habits.iter_mut() {
        if h.content.contains(content_sub) {
            h.pinned = pinned;
            hit = true;
        }
    }
    if hit {
        save_profile(workspace, &p)?;
    }
    Ok(hit)
}

/// 统计通道候选（信号 → HabitEntry）。
///
/// 输入 = 近期会话的用户消息列表（仅 enabled_at 之后的数据由调用方过滤）。
/// 规则（v1 确定性集，纯函数可单测）：
/// - R1 语言：消息 ≥10 条且中文字符占比 >0.7 → 中文交流习惯；
/// - R2 简洁：平均用户长度 <40 字 → 简洁交互习惯；
/// - R3 纠正：纠正前缀（不对/不，/我意思是/别/不要）频次 ≥3 →
///   「先确认理解再动手」习惯。
pub fn stat_candidates(user_messages: &[String], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let n = user_messages.len();
    if n == 0 {
        return out;
    }
    let now = now.to_string();

    // R1 语言。
    let total_chars: usize = user_messages.iter().map(|m| m.chars().count()).sum();
    if total_chars > 0 {
        let cjk: usize = user_messages
            .iter()
            .flat_map(|m| m.chars())
            .filter(|c| {
                let c = *c as u32;
                (0x4E00..=0x9FFF).contains(&c)
            })
            .count();
        let ratio = cjk as f64 / total_chars as f64;
        if n >= 10 && ratio > 0.7 {
            out.push(HabitEntry {
                category: "style".into(),
                content: "用户主要使用中文交流，回复使用中文".into(),
                confidence: (0.5 + ratio * 0.4).min(0.95),
                hits: n as u32,
                first_seen: now.clone(),
                last_seen: now.clone(),
                status: STATUS_ACTIVE.into(),
                pinned: false,
            });
        }
    }

    // R2 简洁。
    let avg_len = total_chars as f64 / n as f64;
    if n >= 10 && avg_len < 40.0 {
        out.push(HabitEntry {
            category: "style".into(),
            content: "用户消息简短直接，回复宜简洁、结论先行".into(),
            confidence: 0.7,
            hits: n as u32,
            first_seen: now.clone(),
            last_seen: now.clone(),
            status: STATUS_ACTIVE.into(),
            pinned: false,
        });
    }

    // R3 纠正前缀。
    const PREFIXES: &[&str] = &["不对", "不，", "不是的", "我意思是", "别", "不要"];
    let corrections = user_messages
        .iter()
        .filter(|m| {
            let t = m.trim_start();
            PREFIXES.iter().any(|p| t.starts_with(p))
        })
        .count();
    if corrections >= 3 {
        out.push(HabitEntry {
            category: "correction".into(),
            content: "用户常用纠正性反馈——动手前优先复述理解、小步确认方向".into(),
            confidence: (0.5 + corrections as f64 * 0.1).min(0.9),
            hits: corrections as u32,
            first_seen: now.clone(),
            last_seen: now.clone(),
            status: STATUS_ACTIVE.into(),
            pinned: false,
        });
    }

    out
}

#[cfg(test)]
mod tests;
