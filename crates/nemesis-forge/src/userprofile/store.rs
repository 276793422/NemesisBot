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

use nemesis_types::safety::violates_security_boundary;
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
    /// 稳定规则身份证（话术 AI 化批一 §4.1）：挖掘兜底产物带来源挖掘器
    /// 的规则 id（如 `stat.language` / `opener.task`）；反思/观察/回抛/
    /// legacy 条目为空串。归并路由见 [`upsert_habit`]（带 id 按 id 匹配，
    /// 模板措辞微调不产生重复条目）；legacy 回填在加载侧按静态模板表补齐
    /// （[`super::mining::static_template_rule_id`]）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rule_id: String,
    /// **持久标记**（serde default 落盘——豁免理由必须与翻桶结果同寿，
    /// 见下）：legacy 静态回填在加载侧翻桶的条目（[backfill_legacy_static_rules]）。
    /// G8 auto 配额截断豁免（[`prune_and_cap_with_max`]）——被翻条目是
    /// 存量长寿数据不是可再生挖掘产物，进 auto 桶参与 [`AUTO_MAX_ENTRIES`]
    /// 截断会在 sweep 被静默删除（2026-11-26 BUG，回归锁 store/tests.rs）。
    ///
    /// **为什么必须落盘**：翻桶 `auto=true` 随保存持久化，豁免若只存内存，
    /// 下个 sweep 重新加载时标记丢失 → 条目被当真产物 → 照删
    /// （首版修复实测副本 sweep #2 复发，同删 2 条）。衰减/安全边界/过期
    /// 照常管辖（非免死金牌）。有机归并翻桶（[`upsert_habit`]
    /// `allow_auto_flip`）不置此标记——信号确认过的条目按 G8 语义照常
    /// 竞争名额。旧二进制读新文件忽略未知键，零迁移。
    #[serde(default)]
    pub legacy_backfilled: bool,
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
            rule_id: String::new(),
            legacy_backfilled: false,
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

/// 画像作用域（双 scope 拆分，2026-11-25 用户拍板）：对话会话与项目会话
/// 各自独立画像文件 + 独立水线 + 独立挖掘/反思管线。文件级拆分（非条目
/// 级 scope 字段）——水线（last_sweep_ms/last_reflect_ms）在 UserProfile
/// 根上，文件级隔离让两 scope 的增量窗/反思触发天然互不干扰。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileScope {
    /// 对话组会话（主 loop / 集群 / headless；现有 `user_profile.json`
    /// 零迁移——存量条目全归对话画像）。
    Dialogue,
    /// 项目组会话（L6++ 项目 loop；资产仍存主 workspace
    /// `config/user_profile_project.json`，项目目录零污染）。
    Project,
}

impl ProfileScope {
    /// 画像文件名（`profile_path_scope` 单一拼接点消费）。
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Dialogue => "user_profile.json",
            Self::Project => "user_profile_project.json",
        }
    }

    /// 对侧 scope（批次二搬移的双文件定位 / 批次三跨文件路由共用）。
    pub fn other(self) -> Self {
        match self {
            Self::Dialogue => Self::Project,
            Self::Project => Self::Dialogue,
        }
    }

    /// wire 名（WSAPI/CLI 参数与响应投影用；web 侧 `scope_from` 解析的逆）。
    pub fn slug(self) -> &'static str {
        match self {
            Self::Dialogue => "dialogue",
            Self::Project => "project",
        }
    }

    /// wire 名解析（[`slug`](Self::slug) 的逆；严格词表，未知 = None 由
    /// 调用方决定兜底或 loud 报错）。观察提名条目自带 scope 的消费点。
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "dialogue" => Some(Self::Dialogue),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

/// 手动条目来源标记（批「注入内容页」D8）：TAB1 手动习惯编辑器落库形态。
/// 用户意志锚点级保护（D9）：decay 跳过、AI 反思改写排除、兜底 upsert
/// 撞文丢弃、prune 不占名额；治理唯一入口 = 编辑器整体替换（[`manual_save`]）。
pub const SOURCE_MANUAL: &str = "manual";

/// 画像文件路径（唯一拼接点）。`= profile_path_scope(workspace, Dialogue)`
/// ——兼容薄转发（存量调用点语义不变）。
pub fn profile_path(workspace: &Path) -> PathBuf {
    profile_path_scope(workspace, ProfileScope::Dialogue)
}

/// 画像文件路径（scope 化真相源）：对话 = `config/user_profile.json`，
/// 项目 = `config/user_profile_project.json`（同目录，主 workspace 锚）。
pub fn profile_path_scope(workspace: &Path, scope: ProfileScope) -> PathBuf {
    workspace.join("config").join(scope.file_name())
}

/// 读取画像（**写路径禁用**）：缺失 = 空画像；损坏 = Err 带路径。
///
/// 只读展示类调用方可用 [`load_profile`] 的容错形态；**任何读取后回写
/// 的路径（sweep/veto/forget/pin/enable）必须用本函数**——损坏静默当空
/// 再 save 会把全量习惯清空（四路评审 B- 根修）。
pub fn try_load_profile(workspace: &Path) -> Result<UserProfile, String> {
    try_load_profile_scope(workspace, ProfileScope::Dialogue)
}

/// [`try_load_profile`](fn@try_load_profile) 的 scope 化真相源。
pub fn try_load_profile_scope(
    workspace: &Path,
    scope: ProfileScope,
) -> Result<UserProfile, String> {
    let path = profile_path_scope(workspace, scope);
    // read_config_string 剥 BOM（裸读时记事本 BOM → parse 失败 → 写路径
    // 全拒绝 / 读路径静默空画像，BUG-20261103-334 同族）。
    let content = match nemesis_utils::fsutil::read_config_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(UserProfile::default()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut profile: UserProfile =
        serde_json::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))?;
    backfill_legacy_static_rules(&mut profile);
    Ok(profile)
}

/// legacy 静态回填（话术 AI 化批一 §4.4 / §八 #7#13，**内存侧不落盘**）：
/// `rule_id` 为空且 content 与静态模板表逐字（归一）相等的条目，补
/// `rule_id` + `auto=true`——修复挖掘器产物条目在 rule_id 字段引入前的
/// 徽标假阴性。幂等（已有 id / 无匹配原样跳过）；**参数化家族不做骨架
/// 匹配**（route.pair/shell.default/terminal.family 含运行期填充值，逐字
/// 比对永远不中），靠归并时 rule_id 采纳有机回填。读路径不写盘，下次
/// 任意保存（sweep/veto/…）自然持久化。
fn backfill_legacy_static_rules(profile: &mut UserProfile) {
    for h in &mut profile.habits {
        if h.rule_id.is_empty()
            && let Some(rule) = super::mining::static_template_rule_id(&h.content)
        {
            h.rule_id = rule.to_string();
            h.auto = true;
            // 持久标记（落盘）：徽标翻桶但**豁免 G8 auto 配额截断**（2026-11-26
            // BUG 根修）——被翻条目是存量长寿数据，参与 AUTO_MAX_ENTRIES
            // 竞争会在首个 sweep 被静默删除。衰减/安全边界照常管辖。
            h.legacy_backfilled = true;
        }
    }
}

/// 读取画像（缺失/损坏 = 空画像；不 panic 不误伤）。
///
/// ⚠ 仅限纯展示/统计读取；读后回写的调用方必须走 [`try_load_profile`]。
pub fn load_profile(workspace: &Path) -> UserProfile {
    load_profile_scope(workspace, ProfileScope::Dialogue)
}

/// [`load_profile`](fn@load_profile) 的 scope 化真相源。
pub fn load_profile_scope(workspace: &Path, scope: ProfileScope) -> UserProfile {
    try_load_profile_scope(workspace, scope).unwrap_or_default()
}

/// 写画像（config 家族写串行锁；原子落盘）。
pub fn save_profile(workspace: &Path, profile: &UserProfile) -> Result<(), String> {
    save_profile_scope(workspace, ProfileScope::Dialogue, profile)
}

/// [`save_profile`](fn@save_profile) 的 scope 化真相源。
pub fn save_profile_scope(
    workspace: &Path,
    scope: ProfileScope,
    profile: &UserProfile,
) -> Result<(), String> {
    let _serial = nemesis_config::config_write_lock();
    save_profile_locked_scope(workspace, scope, profile)
}

/// [`save_profile`](fn@save_profile) 的锁内版：调用方**已持**
/// [`nemesis_config::config_write_lock`] 时使用（锁不可重入——锁内函数
/// 再走 `save_profile` 会自死锁；四路评审 B- 根修配套）。
pub fn save_profile_locked(workspace: &Path, profile: &UserProfile) -> Result<(), String> {
    save_profile_locked_scope(workspace, ProfileScope::Dialogue, profile)
}

/// [`save_profile_locked`](fn@save_profile_locked) 的 scope 化真相源。
pub fn save_profile_locked_scope(
    workspace: &Path,
    scope: ProfileScope,
    profile: &UserProfile,
) -> Result<(), String> {
    let path = profile_path_scope(workspace, scope);
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
///
/// 双锚点（双 scope 拆分 D3）：开关全局一根弦，但两份画像文件各有独立
/// 水线——**项目画像缺 enabled_at 锚点 = sweep_window_start_ms 恒 None =
/// 项目画像永不采集**。enable 时两份同步写锚点（幂等仅向前；项目文件
/// 缺失 = 创建空画像 + 锚点）。disable 只动 config（sweep no-op），画像
/// 文件与锚点原样保留。
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
    // 项目画像锚点同步（D3）：同锁内第二份文件。失败不回滚对话画像
    // （锚点写入幂等；显式 enable 与项目 sweep 的锚点自动回填
    // [`backfill_project_anchor_locked`]（批次〇）都会补齐）——但诚实上抛。
    let mut project = try_load_profile_scope(workspace, ProfileScope::Project)?;
    if project.enabled_at.is_none() {
        project.enabled_at = profile.enabled_at.clone();
        save_profile_locked_scope(workspace, ProfileScope::Project, &project)?;
    }
    Ok(profile)
}

/// 项目画像锚点自动回填（批次〇，2026-11-26 存量迁移缺口）：双 scope 代码
/// 晚于现场 enable 落仓库——存量部署只锚了对话文件（单 scope 时代
/// [`enable_profile`] 只写 `user_profile.json`），项目文件不存在 → 项目
/// sweep 在 `sweep_window_start_ms` 恒 None = 永久 no-op，项目习惯一直为
/// 空。本函数在项目 sweep 窗口 None 时被编排层调用：
///
/// - 对话画像已锚（用户开启过画像）→ 补锚：项目文件缺失 = 创建空画像 +
///   `enabled_at` 取对话锚点值（锚点仅向前，语义对齐 [`enable_profile`]
///   双锚；幂等——项目已锚不动）；
/// - 对话也未锚 = 用户从未开启，不补（尊重，`Ok(false)`）；
/// - 任一画像文件损坏（JSON parse 失败）= Err 上抛（不静默不回写——写
///   路径纪律，编排层按 scope 隔离失败处理）。
///
/// 调用方**必须已持** [`nemesis_config::config_write_lock`]（RMW 全程锁内
/// 纪律；`_locked` 后缀 = 锁内版约定）。返回是否补了锚。
pub fn backfill_project_anchor_locked(workspace: &Path) -> Result<bool, String> {
    let dialogue = try_load_profile_scope(workspace, ProfileScope::Dialogue)?;
    let Some(anchor) = dialogue.enabled_at.clone() else {
        return Ok(false); // 对话也未锚 = 用户从未开启，不补
    };
    let mut project = try_load_profile_scope(workspace, ProfileScope::Project)?;
    if project.enabled_at.is_some() {
        return Ok(false); // 已锚 = 幂等不动
    }
    project.enabled_at = Some(anchor);
    save_profile_locked_scope(workspace, ProfileScope::Project, &project)?;
    Ok(true)
}

// 渲染层过滤 `violates_security_boundary`：2026-11-24 单一真相源上收
// `nemesis_types::safety`（自学习铺路批次一——洞察台账共用同一份禁词表，
// 原私有 const 本模块副本退役；画像迁入 forge 后同源引用不变）。

/// 上榜前清洗：剔除 expired + 安全违规条目；**vetoed 条目保留驻留**（二轮
/// 复查根修：负样本压制语义——vetoed 一旦被 prune 删除，规则通道的固定
/// 字符串候选会在下一轮 sweep 原样重新生成，否决失效；驻留时 upsert 命中
/// 同内容条目保持 vetoed 态，天然抑制重生成）；条数上限分桶计数（G8）：
/// 非 auto 条目按置信度截 [`MAX_ENTRIES`]（config 可调），auto 条目
/// （固定规则/挖掘器产物）单独截 [`AUTO_MAX_ENTRIES`]，vetoed 不占任何
/// 名额。pinned 条目优先保留（用户意志不被截断）。
///
/// **legacy 回填豁免**（2026-11-26 BUG 根修）：加载侧静态回填翻桶的条目
/// （持久标记 [`HabitEntry::legacy_backfilled`]）随 auto 桶但**不占**
/// [`AUTO_MAX_ENTRIES`] 名额——它们是存量长寿数据不是可再生产物。
///
/// 手动条目三分桶（双 scope 拆分 D9）：`source=manual`（TAB1 编辑器
/// 用户手写）独立成桶**不截断**（用户意志不限量——写多少注入多少，上限
/// 由用户自己把握）；截断只作用于机器产物两桶。
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
    // 三分桶：手动（用户手写，不截）/ 非-auto（反思/观察/外部/AI）/
    // auto（挖掘器固定产物）。
    let (mut handwritten, kept): (Vec<_>, Vec<_>) =
        kept.into_iter().partition(|h| h.source == SOURCE_MANUAL);
    let (mut non_auto, mut auto): (Vec<_>, Vec<_>) = kept.into_iter().partition(|h| !h.auto);
    // 各桶内 pinned 优先（用户意志不被截断），其次置信度。
    let sort_key = |a: &HabitEntry, b: &HabitEntry| {
        b.pinned.cmp(&a.pinned).then_with(|| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };
    handwritten.sort_by(|a, b| sort_key(a, b));
    non_auto.sort_by(|a, b| sort_key(a, b));
    auto.sort_by(|a, b| sort_key(a, b));
    non_auto.truncate(max_entries);
    // G8 配额只作用于**可再生真产物**：legacy 静态回填翻桶的条目（持久
    // 标记，2026-11-26 BUG 根修）是存量长寿数据，随行不占名额——否则
    // 翻桶即落败（置信度通常低于新鲜产物），首个 sweep 静默删除存量。
    // 有机归并翻桶（信号确认）不带标记，照常竞争。两子桶各自保序。
    let (auto_legacy, mut auto_genuine): (Vec<_>, Vec<_>) =
        auto.into_iter().partition(|h| h.legacy_backfilled);
    auto_genuine.truncate(AUTO_MAX_ENTRIES);
    profile.habits = handwritten;
    profile.habits.extend(non_auto);
    profile.habits.extend(auto_genuine);
    profile.habits.extend(auto_legacy);
    profile.habits.extend(vetoed);
}

/// upsert 归并统计刷新：置信度取高、hits+1、刷新 last_seen；steps/evidence
/// 取并集（保序去重 + 清洗截断）。
///
/// `allow_auto_flip` = auto 翻桶**单向性**（话术 AI 化 §4.4 / §八 #10）：
/// 仅 `existing.rule_id` 为空（未被任何规则认领的 legacy/反思条目）允许
/// 兜底产物把 auto 翻 true；已被规则/AI 认领的条目（rule_id 非空 + 批二
/// 起 auto=false 的 AI 改写条目）归并只刷新统计，不翻桶不改话术——AI
/// 话术被兜底信号命中后若翻回 auto=true，会挂着「模板兜底」徽标撒谎。
fn merge_habit_stats(existing: &mut HabitEntry, entry: HabitEntry, allow_auto_flip: bool) {
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
    if allow_auto_flip {
        existing.auto = existing.auto || entry.auto;
    }
}

/// upsert：语义相同取高置信度、hits+1、刷新 last_seen；不同则插入。
/// vetoed 条目命中时**保持 vetoed**（仅 decaying→active 转换）——与
/// `prune_and_cap` 的驻留语义配套构成否决抑制链：驻留 + 不复活 = 同内容
/// 候选永不重生成。
///
/// 归并路由（话术 AI 化批一 §4.4 upsert 三分）：
/// - **挖掘兜底产物**（`rule_id` 非空）：先按 `rule_id` 匹配归并（模板
///   措辞后续微调不产生重复条目）；同 id 未命中回落 content 匹配
///   （legacy 条目 `rule_id` 为空 → 采纳新 id + 允许 auto 翻桶 = 有机回填）。
/// - **反思/观察/回抛产物**（`rule_id` 空）：按 content 归一匹配（现状
///   不变）；命中 `rule_id` 为空的条目时 auto 翻桶语义保持（auto OR）。
///
/// 结构化字段合并：steps 取并集（去重保序、清洗截断）——反思通道对同一
/// 仪式的步骤细化能累积进既有条目；evidence 同并集（溯源累积）；auto
/// 翻桶单向性见 [`merge_habit_stats`]。
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
    // 手动条目撞文（双 scope 拆分 D9）：用户已手写同义条目 → 机器产物
    // 直接丢弃（不归并不重复插）。手动话术是用户原文，机器不归并（不会
    // 刷 stats/翻桶），同义信号已由手写条目表达——再插一条 = 重复注入。
    if profile
        .habits
        .iter()
        .any(|h| h.source == SOURCE_MANUAL && norm(&h.content) == norm(&entry.content))
    {
        return;
    }
    // ① 带规则身份证的产物：先按 rule_id 匹配（同 id 归并：existing 已被
    //    规则认领 → 不采纳不翻桶，AI 改写条目的措辞/桶位原样保留）。
    if !entry.rule_id.is_empty()
        && let Some(existing) = profile
            .habits
            .iter_mut()
            .find(|h| h.rule_id == entry.rule_id)
    {
        merge_habit_stats(existing, entry, false);
        return;
    }
    // ② content 归一匹配（反思产物主路径 + 带 id 产物同 id 未命中的回落）。
    if let Some(pos) = profile
        .habits
        .iter()
        .position(|h| h.category == entry.category && norm(&h.content) == norm(&entry.content))
    {
        let existing = &mut profile.habits[pos];
        // rule_id 采纳 + auto 翻桶单向性：仅 existing 未被规则认领
        // （rule_id 空）时采纳新 id 并允许 auto 翻 true。
        let adoptable = existing.rule_id.is_empty();
        if adoptable && !entry.rule_id.is_empty() {
            existing.rule_id = entry.rule_id.clone();
        }
        merge_habit_stats(existing, entry, adoptable);
        return;
    }
    entry.confidence = entry.confidence.clamp(0.0, 1.0);
    entry.steps = HabitEntry::sanitize_steps(&entry.steps);
    entry.evidence.truncate(MAX_EVIDENCE);
    profile.habits.push(entry);
}

/// 反思改写结局（话术 AI 化批二 §4.3）。
#[derive(Debug, Clone, PartialEq)]
pub enum RewriteOutcome {
    /// 至少一条被原地改写（含被吞合并）：返回全部点名 id（调用方在 claimed
    /// 集合登记，同拍后续结论不再二次消费同 id——首条生效余者按纯 AI）。
    Rewrote(Vec<String>),
    /// 点名 id 在画像中无可改写目标（vetoed/pinned 持有者不算）→ 调用方
    /// 按 [`upsert_habit`] 插入 AI 条目（`rule_id` = 首个点名 id）并认领
    /// 全部 id；若 vetoed/pinned 条目已持有该 id，upsert ① 的 rule_id
    /// 匹配会把统计并进持有者——措辞/桶位不动，不产生同 id 重复条目。
    NoHit,
    /// 改写内容触安全硬边界（或非 habit 主语）：**不删不改不插**——被吞
    /// 条目原样保留（宁可不改写，不可借改写删内容）。
    Rejected,
}

/// 反思结论原地改写（话术 AI 化批二 §4.3）：AI 点名 rule_ids → 画像中
/// 按规则 id 找既有条目原地改写措辞。多 id 合并覆盖时，被吞条目按 id 删除、
/// evidence/steps 并入目标。
///
/// 语义细节：
/// - **全量位置扫描**：每个 id 扫全部匹配位（不只首个）——首个可改写位
///   （非 vetoed 且非 pinned）= 改写目标，其余可改写位 = 被吞；vetoed /
///   pinned 持有者整位跳过（vetoed = 否决抑制链保护；pinned = 用户钉住
///   的措辞不许 AI 静默改写）。
/// - **改写即 AI 桶**：`auto=false`（挂「AI 总结」徽标；兜底信号后续命中
///   经 [`merge_habit_stats`] 的 allow_auto_flip=false 不翻回模板桶——
///   §4.4 翻桶单向性）。
/// - **hits 保留**：改写不是新信号命中，目标 hits 计数原样（与 upsert 的
///   hits+1 语义区分）；evidence/steps 累积保留 + 被吞并入（尾部超限淘汰
///   头部，同源 [`merge_habit_stats`] 纪律）。
/// - **同源过闸**：`violates_security_boundary`（拒绝=Rejected 全不动）+
///   fold/sanitize/clamp（与解析层清洗同源，双保险）。
pub fn apply_reflected_rewrite(
    profile: &mut UserProfile,
    entry: HabitEntry,
    rule_ids: &[String],
    now: &str,
) -> RewriteOutcome {
    // 双保险与 upsert_habit 同源：非 habit 主语 / 安全硬边界 = 全不动。
    if entry.kind != KIND_HABIT || violates_security_boundary(&entry.content) {
        return RewriteOutcome::Rejected;
    }
    let ids: Vec<String> = rule_ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return RewriteOutcome::NoHit; // 无 id 无从改写（调用方不该走到这）
    }
    let eligible = |h: &HabitEntry| {
        h.status != STATUS_VETOED
            && !h.pinned
            && h.source != SOURCE_MANUAL
            && ids.iter().any(|id| id == &h.rule_id)
    };
    // 全量位置扫描：首个可改写位 = 目标，其余 = 被吞；vetoed/pinned 不进
    // 两个集合（原样保留）。
    let positions: Vec<usize> = (0..profile.habits.len())
        .filter(|i| eligible(&profile.habits[*i]))
        .collect();
    let Some(target_pos) = positions.first().copied() else {
        return RewriteOutcome::NoHit;
    };
    let swallow_pos = &positions[1..];

    // 先摘被吞（降序 remove 保持其余索引稳定），evidence/steps 并入素材
    // 先收集——目标位在被吞位之前时，摘除不移动更小索引，目标位仍有效。
    let mut merged_evidence = entry.evidence.clone();
    let mut merged_steps = entry.steps.clone();
    for pos in swallow_pos.iter().rev() {
        let swallowed = profile.habits.remove(*pos);
        merged_evidence.extend(swallowed.evidence);
        merged_steps.extend(swallowed.steps);
    }

    let t = &mut profile.habits[target_pos];
    // 措辞与类别替换（fold 同源双保险——解析层已清洗）。
    t.content = HabitEntry::fold_one_line(&entry.content, MAX_CONTENT_CHARS);
    t.category = entry.category.clone();
    // 统计累积（无 hits+1——改写不是信号命中）；decaying 复活；auto 强制
    // false（AI 总结桶；翻桶单向性 §4.4）。
    t.confidence = t.confidence.max(entry.confidence).clamp(0.0, 1.0);
    if !entry.last_seen.is_empty() {
        t.last_seen = entry.last_seen.to_string();
    } else if !now.is_empty() {
        t.last_seen = now.to_string();
    }
    if t.status == STATUS_DECAYING {
        t.status = STATUS_ACTIVE.to_string();
    }
    t.auto = false;
    for s in HabitEntry::sanitize_steps(&merged_steps) {
        if !t.steps.iter().any(|e| e == &s) {
            t.steps.push(s);
        }
    }
    t.steps = HabitEntry::sanitize_steps(&t.steps);
    for ev in &merged_evidence {
        if !ev.is_empty() && !t.evidence.iter().any(|e| e == ev) {
            t.evidence.push(ev.clone());
        }
    }
    // 超限保尾部（新证据信息量高，头部淘汰——merge_habit_stats 同纪律）。
    if t.evidence.len() > MAX_EVIDENCE {
        let start = t.evidence.len() - MAX_EVIDENCE;
        t.evidence.drain(..start);
    }
    RewriteOutcome::Rewrote(ids)
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
    scope: ProfileScope,
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
    let mut profile = try_load_profile_scope(workspace, scope)?;
    upsert_habit(&mut profile, entry);
    save_profile_locked_scope(workspace, scope, &profile)
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
        // 手动条目不衰减（D9：用户意志锚点级——手写的永不过期）。
        if h.source == SOURCE_MANUAL {
            continue;
        }
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
pub fn veto_habit(
    workspace: &Path,
    scope: ProfileScope,
    content_sub: &str,
) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("veto 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    // RMW 全程持锁（四路评审 B- 根修）；损坏画像 = Err 不回写。
    let _serial = nemesis_config::config_write_lock();
    let mut p = try_load_profile_scope(workspace, scope)?;
    let needle = content_sub.to_lowercase();
    let mut hit = false;
    for h in p.habits.iter_mut() {
        if h.content.to_lowercase().contains(&needle) {
            h.status = STATUS_VETOED.to_string();
            hit = true;
        }
    }
    if hit {
        save_profile_locked_scope(workspace, scope, &p)?;
    }
    Ok(hit)
}

/// forget：按内容子串整条删除（与 veto 不同：删除后同语义候选可重新生成）。
/// 空子串 = Err（同 [`veto_habit`] 防线）。匹配大小写不敏感。
pub fn forget_habit(
    workspace: &Path,
    scope: ProfileScope,
    content_sub: &str,
) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("forget 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let _serial = nemesis_config::config_write_lock();
    let mut p = try_load_profile_scope(workspace, scope)?;
    let needle = content_sub.to_lowercase();
    let before = p.habits.len();
    p.habits
        .retain(|h| !h.content.to_lowercase().contains(&needle));
    let removed = p.habits.len() < before;
    if removed {
        save_profile_locked_scope(workspace, scope, &p)?;
    }
    Ok(removed)
}

/// pin / unpin：按内容子串切换钉住（钉住 = 不衰减、渲染置顶）。
/// 空子串 = Err（同 [`veto_habit`] 防线）。匹配大小写不敏感。
pub fn set_pinned(
    workspace: &Path,
    scope: ProfileScope,
    content_sub: &str,
    pinned: bool,
) -> Result<bool, String> {
    if content_sub.trim().is_empty() {
        return Err("pin 匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let _serial = nemesis_config::config_write_lock();
    let mut p = try_load_profile_scope(workspace, scope)?;
    let needle = content_sub.to_lowercase();
    let mut hit = false;
    for h in p.habits.iter_mut() {
        if h.content.to_lowercase().contains(&needle) {
            h.pinned = pinned;
            hit = true;
        }
    }
    if hit {
        save_profile_locked_scope(workspace, scope, &p)?;
    }
    Ok(hit)
}

/// [`move_habit_scope`] 结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MoveScopeOutcome {
    /// true = 真搬了；false = 幂等（条目已在目标画像）。
    pub moved: bool,
    /// 来源画像 wire 名（"dialogue" / "project"）。
    pub from: &'static str,
    /// 目标画像 wire 名。
    pub to: &'static str,
    /// 被搬条目全文（定位用子串可能短于全文，回显全文防误认）。
    pub content: String,
}

/// 条目跨 scope 搬移（批次二，2026-11-26）：把单条习惯在对话/项目画像
/// 文件间搬移（用户意志兜底——位置分型判错时的修正通道）。
///
/// 语义：
/// - 单 [`nemesis_config::config_write_lock`] 内两份 `try_load_profile_scope`
///   （任一损坏 = Err 不回写——写路径纪律）；
/// - 定位：精确归一匹配（`trim().to_lowercase()` 相等）优先，无精确命中
///   再退子串匹配；**两份都命中 = Err**（治理重复，不猜搬哪条）；
///   目标已命中 = 幂等 `Ok(moved=false)`；来源命中多条 = Err（搬移是
///   单条操作）；双份全无 = Err「未找到」；
/// - 摘除 + 全字段原样搬运（content/kind/steps/evidence/pinned/status/
///   rule_id/source/confidence/hits/first_seen/last_seen——source=manual
///   的五面保护随字段自然保留）；`entry.scope` 注记字段置空（归属由
///   文件承载，单一真相源 = 文件；全仓 grep 确认零读者，置空安全）；
/// - 目标锚点（`enabled_at`）缺失照写不拒（注入按文件读；采集锚点是
///   另一维度，批次〇回填链会补齐）——决策记录在案；
/// - 落盘顺序：**先目标后来源**——中途失败的最坏窗口 = 两份各有一条
///   （重复），绝不会丢条目；veto/forget/pin 同款「大小写不敏感子串」
///   定位口径。
pub fn move_habit_scope(
    workspace: &Path,
    content_sub: &str,
    to_scope: ProfileScope,
) -> Result<MoveScopeOutcome, String> {
    if content_sub.trim().is_empty() {
        return Err("搬移匹配子串为空（会命中全部条目），拒绝执行".into());
    }
    let _serial = nemesis_config::config_write_lock();
    let mut source = try_load_profile_scope(workspace, to_scope.other())?;
    let mut target = try_load_profile_scope(workspace, to_scope)?;
    let needle = content_sub.trim().to_lowercase();
    // 定位闭包：精确归一命中优先，退子串；返回 (命中数, 精确命中下标)。
    let locate = |habits: &[HabitEntry]| -> (usize, Option<usize>) {
        let mut exact: Vec<usize> = Vec::new();
        let mut partial: Vec<usize> = Vec::new();
        for (i, h) in habits.iter().enumerate() {
            let c = h.content.trim().to_lowercase();
            if c == needle {
                exact.push(i);
            } else if c.contains(&needle) {
                partial.push(i);
            }
        }
        if !exact.is_empty() {
            (exact.len(), Some(exact[0]))
        } else {
            (partial.len(), None)
        }
    };
    let (src_hits, src_exact) = locate(&source.habits);
    let (tgt_hits, _tgt_exact) = locate(&target.habits);
    if src_hits > 0 && tgt_hits > 0 {
        return Err("两份画像都命中该子串（治理重复）：先经 veto/forget 清理一侧再搬移".into());
    }
    if tgt_hits > 0 {
        let content = target.habits[_tgt_exact.unwrap_or(0)].content.clone();
        return Ok(MoveScopeOutcome {
            moved: false,
            from: to_scope.slug(),
            to: to_scope.slug(),
            content,
        });
    }
    if src_hits > 1 {
        return Err(format!(
            "子串命中来源画像 {src_hits} 条条目，搬移是单条操作：请用更长的子串唯一定位"
        ));
    }
    if src_hits == 0 {
        return Err(format!("两份画像均未找到匹配「{content_sub}」的条目"));
    }
    let idx = src_exact.unwrap_or_else(|| {
        // 无精确命中：子串唯一命中——找出那一条的下标。
        source
            .habits
            .iter()
            .position(|h| h.content.trim().to_lowercase().contains(&needle))
            .expect("src_hits==1 已保证存在")
    });
    let mut entry = source.habits.remove(idx);
    entry.scope = String::new();
    let content = entry.content.clone();
    target.habits.push(entry);
    // 先目标后来源：中途失败最坏 = 重复，不丢数据。
    save_profile_locked_scope(workspace, to_scope, &target)?;
    save_profile_locked_scope(workspace, to_scope.other(), &source)?;
    Ok(MoveScopeOutcome {
        moved: true,
        from: to_scope.other().slug(),
        to: to_scope.slug(),
        content,
    })
}

/// 手动习惯保存（TAB1 手动编辑器整体替换语义；双 scope 拆分 D8/D9）。
///
/// 按行 diff：解析 `lines` 每行（`[类别] 内容`，类别可选缺省
/// [`DEFAULT_MANUAL_CATEGORY`]；空行跳过）→ 新行 upsert（`source=manual`、
/// `confidence=1.0`、`kind=habit`、恒 active）；消失行整条删除（只删
/// `source=manual`——机器产物不经本通道动）。已存在的手动条目（按内容
/// 归一匹配）原样保留（first_seen 不刷新）。
///
/// 防线：违规行（安全边界）= Err 拒整批（报行号，fail-loud——手写安全
/// 语义内容不允许静默跳过进注入面）；与机器条目撞文容忍并存（调用方
/// 经返回的 `conflicts` 诚实提示）。
pub fn manual_save(
    workspace: &Path,
    scope: ProfileScope,
    lines: &[&str],
    now: &str,
) -> Result<ManualSaveOutcome, String> {
    // 先全量解析校验（fail-loud 在任何写动作之前）。
    let mut parsed: Vec<(String, String)> = Vec::new(); // (category, content)
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (category, content) =
            parse_manual_line(line).map_err(|e| format!("第 {} 行: {e}", i + 1))?;
        if violates_security_boundary(&content) {
            return Err(format!(
                "第 {} 行触安全硬边界，整批拒绝保存（手写内容不允许进注入面）",
                i + 1
            ));
        }
        let content = HabitEntry::fold_one_line(&content, MAX_CONTENT_CHARS);
        parsed.push((category, content));
    }
    let norm = |s: &str| s.trim().to_lowercase();
    let _serial = nemesis_config::config_write_lock();
    let mut profile = try_load_profile_scope(workspace, scope)?;
    // diff 删除：现有手动条目按 norm(content) 不在新行集 → 删。
    let wanted: std::collections::HashSet<String> = parsed.iter().map(|(_, c)| norm(c)).collect();
    let before = profile.habits.len();
    let removed_existing: Vec<String> = profile
        .habits
        .iter()
        .filter(|h| h.source == SOURCE_MANUAL && !wanted.contains(&norm(&h.content)))
        .map(|h| h.content.clone())
        .collect();
    profile
        .habits
        .retain(|h| h.source != SOURCE_MANUAL || wanted.contains(&norm(&h.content)));
    let removed = before - profile.habits.len();
    // diff 新增：新行集不在现有（任意来源同文 → 视为已表达，不重复建
    // 手动条目；与机器条目撞文 = conflicts 诚实回报）。
    let mut added = 0usize;
    let mut conflicts: Vec<String> = Vec::new();
    for (category, content) in &parsed {
        let existing_any = profile
            .habits
            .iter()
            .any(|h| norm(&h.content) == norm(content));
        if existing_any {
            let is_manual = profile
                .habits
                .iter()
                .any(|h| h.source == SOURCE_MANUAL && norm(&h.content) == norm(content));
            if !is_manual {
                conflicts.push(content.clone());
            }
            continue; // 已表达（手动保留原条目 / 机器条目撞文不重复建）
        }
        let mut entry = HabitEntry::auto_entry(category, content, 1.0, 0, now);
        entry.auto = false;
        entry.source = SOURCE_MANUAL.into();
        profile.habits.push(entry);
        added += 1;
    }
    save_profile_locked_scope(workspace, scope, &profile)?;
    Ok(ManualSaveOutcome {
        added,
        removed,
        removed_contents: removed_existing,
        conflicts,
    })
}

/// [`manual_save`] 结果（诊断/前端诚实提示）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManualSaveOutcome {
    /// 新建手动条目数。
    pub added: usize,
    /// 删除的消失行数。
    pub removed: usize,
    /// 被删除条目的原文（前端可回显撤销线索）。
    pub removed_contents: Vec<String>,
    /// 与既有机器条目撞文的手动行（未重复建，诚实提示）。
    pub conflicts: Vec<String>,
}

/// 手动行默认类别（无 `[类别]` 前缀时）。
pub const DEFAULT_MANUAL_CATEGORY: &str = "manual";

/// 手动行语法解析：`[类别] 内容`（类别可选）。返回 (类别, 内容)。
/// Err = 空内容 / 形式错误。
fn parse_manual_line(line: &str) -> Result<(String, String), String> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let category = rest[..end].trim();
        let content = rest[end + 1..].trim();
        if category.is_empty() {
            return Err("类别段为空（[ ] 内需要类别名）".into());
        }
        if content.is_empty() {
            return Err("内容为空".into());
        }
        let category = HabitEntry::fold_one_line(category, 24);
        return Ok((category, content.to_string()));
    }
    let content = line.trim();
    if content.is_empty() {
        return Err("内容为空".into());
    }
    Ok((DEFAULT_MANUAL_CATEGORY.into(), content.to_string()))
}

/// 手动条目 → 编辑器行文本（TAB1 上半编辑器回显形态：`[类别] 内容`）。
pub fn manual_lines(profile: &UserProfile) -> Vec<String> {
    profile
        .habits
        .iter()
        .filter(|h| h.source == SOURCE_MANUAL)
        .map(|h| format!("[{}] {}", h.category, h.content))
        .collect()
}

#[cfg(test)]
mod tests;
