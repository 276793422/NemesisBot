//! 洞察台账（Insights Ledger）——自学习铺路批次一（2026-11-24）。
//!
//! 「LLM 当脑子、Rust 当路」的下匝道载体：学习对话（批次三）产出的
//! 行为准则/流程约束条目化落盘，active 条目每轮常驻注入 agent 的 merged
//! snapshot。本模块只做**确定性**部分：台账 CRUD / 生命周期 / 激活闸 /
//! 安全词表双侧 / 原子写 / 字节稳定渲染；学什么、学成什么形态、内容怎么
//! 写归对话引擎（批次三，不在本模块）。
//!
//! 真相源：`docs/PLAN/2026-11-24_自学习铺路计划.md` §4.1。
//!
//! 设计要点：
//! - 文件形态 `<workspace>/forge/insights.json`，每 cycle / 每 build 全量
//!   重读（无缓存一致性负担）；store 实例只是**路径视图**——WSAPI/CLI
//!   按调用现构造是合法形态，RMW 由模块级全局写锁串行（跨实例不丢更新）。
//! - 原子写（temp+rename）；损坏 fail-closed（全不加载 + WARN + 坏文件
//!   挪 `.corrupt` 留档）；BOM 容忍（config.json 同款先例）。
//! - 激活闸（防自激，确定性护栏）：dialogue 来源恒 candidate；同指纹复现
//!   ≥ [`InsightStore::activation_recurrence`] → active；manual 来源直接
//!   active（用户是权威）；veto → retired(vetoed) 且同指纹再提案**不复活**
//!   （驻留压制，B 线画像 vetoed 语义同款——删了会重生成，驻留才压得住）。
//! - 注入渲染 [`InsightStore::render_section`]：仅 active；cap 5；排序
//!   稳定（confidence desc, id asc）；渲染侧词表复扫（第二道闸——盘面
//!   手编/历史残留防御）；空台账/损坏 = None（段缺席零字节）。
//! - **无 `last_injected` 字段**——注入是 per-build 无状态渲染，落字段会
//!   造成每轮写盘放大。
//! - `stats` 字段批次四补（纠正率 delta 实效统计）；本版 schema 预留
//!   `version`，entries 未知字段由 serde 默认容忍。
//!
//! 测试：[`tests`]（独立文件，仓库内联测试禁令合规）。

use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::{Path, PathBuf};

/// statement 上限（字符）。schema 常量刻意不 config 化——同 forge 其他
/// 格式约束先例：合法性不能随部署漂移。超限**拒绝不截断**（压缩措辞是
/// 对话引擎的活，不是 Rust 的活）。
pub const STATEMENT_MAX_CHARS: usize = 120;
/// evidence 上限（字符），超限截断（宁存摘要不拒收——证据是辅助字段）。
pub const EVIDENCE_MAX_CHARS: usize = 300;
/// 注入条目上限（active 条数 cap）。
pub const INJECTION_CAP: usize = 5;
/// 同指纹复现转 active 的缺省阈值。config `learn.activation_recurrence`
/// 批次三接线；构造点 [`InsightStore::with_recurrence`] 已可调。
pub const DEFAULT_ACTIVATION_RECURRENCE: u32 = 3;

/// 条目状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsightStatus {
    /// 候选（dialogue 来源落库初态；同指纹复现达标或人工晋升转 active）。
    Candidate,
    /// 常驻注入。
    Active,
    /// 退役（不注入；retire_reason 记原因）。
    Retired,
}

/// 条目来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsightSource {
    /// 学习对话产出（激活闸管辖）。
    Dialogue,
    /// 人工录入（用户是权威，直接 active）。
    Manual,
}

/// 退役原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetireReason {
    /// 被实践/用户证明不成立。
    Corrected,
    /// 被更新的条目取代。
    Superseded,
    /// 长期无复现自然过期。
    Expired,
    /// 人工否决（同指纹再提案永久压制）。
    Vetoed,
}

/// 洞察条目。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InsightEntry {
    /// `ins-<yyyymmdd>-<序号>`（序号按同日最大值 +1，全量扫描）。
    pub id: String,
    /// statement 规范化 SHA256 前 16 hex（去重/复现计数键）。
    pub fingerprint: String,
    /// ≤120 字陈述（注入原文）。
    pub statement: String,
    /// 自由串（LLM 定）；空串落库时归一 "general"。
    pub category: String,
    /// ≤300 字证据摘要（超限截断）。
    pub evidence: String,
    pub source: InsightSource,
    /// 0.0–1.0（越界钳制）。
    pub confidence: f64,
    /// 同指纹累计次数（含首次）。
    pub occurrences: u32,
    pub first_seen: String,
    pub last_seen: String,
    pub status: InsightStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire_reason: Option<RetireReason>,
    /// 首次转 active 的时刻（RFC3339 local）。manual 来源落库即 active =
    /// first_seen 同刻；dialogue 来源 = 复现达标或人工 promote 时刻。
    /// 实效统计（[`InsightStore::refresh_stats`]）以最早激活时刻做
    /// 前后窗口切分；旧台账无此键（serde default）零迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    /// 场景归属（洞察页与场景拆分 2026-11-24）：None = 对话洞察（主 loop
    /// 注入面）/ Some(项目 id) = 项目洞察（该项目 loop 注入面）。只在
    /// [`InsightStore::add_candidate`] 新建时落值，复现合并不覆盖（身份
    /// 字段与 first_seen 同级）；旧台账无此键（serde default）零迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// 实效统计（自学习铺路批次四）：纠正率 delta 前后窗口对照。
///
/// 窗口切分点 = 最早激活时刻（`activated_at` 最小的 active 条目）：
/// 之前捕获的纠正候选算「前窗」，之后的算「后窗」。实效叙事是
/// **后窗纠正率应低于前窗**（洞察生效 → 同类纠正变少）；比率只是
/// 只读推导数据，判断归人/LLM，本模块不替下结论。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InsightStats {
    /// 候选台账总行数（candidates.jsonl 逐行计数，含已消费）。
    #[serde(default)]
    pub candidates_total: u64,
    /// 对话消费游标读数（已被学习对话消化的行数）。
    #[serde(default)]
    pub candidates_consumed: u64,
    /// 台账条目按状态计数。
    #[serde(default)]
    pub entries_candidate: u64,
    #[serde(default)]
    pub entries_active: u64,
    #[serde(default)]
    pub entries_retired: u64,
    /// 持有激活时刻的条目数（窗口对照的样本基数）。
    #[serde(default)]
    pub activated_count: u64,
    /// 窗口切分点（最早 activated_at，RFC3339；无激活条目 = 空串）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub activation_pivot: String,
    /// 激活前捕获的纠正候选数。
    #[serde(default)]
    pub corrections_before: u64,
    /// 激活后捕获的纠正候选数。
    #[serde(default)]
    pub corrections_after: u64,
    /// 前窗长度（最早候选 ts → 切分点，天；小数）。
    #[serde(default)]
    pub window_days_before: f64,
    /// 后窗长度（切分点 → 统计时刻，天；小数）。
    #[serde(default)]
    pub window_days_after: f64,
    /// 前窗每天纠正率（= corrections_before / window_days_before；窗口
    /// 长度为 0 时 = 0.0，不编造比率）。
    #[serde(default)]
    pub correction_rate_before: f64,
    /// 后窗每天纠正率。
    #[serde(default)]
    pub correction_rate_after: f64,
    /// 统计生成时刻（RFC3339 local）。
    #[serde(default)]
    pub generated_at: String,
}

impl InsightStats {
    /// 窗口对照 delta：后窗每天率 − 前窗每天率（负 = 后窗更少纠正，
    /// 洞察生效方向；任一窗口为 0 时 None——不编造可比性）。
    pub fn rate_delta(&self) -> Option<f64> {
        if self.window_days_before <= 0.0 || self.window_days_after <= 0.0 {
            return None;
        }
        Some(self.correction_rate_after - self.correction_rate_before)
    }
}

/// 盘上台账（`<workspace>/forge/insights.json`）。`stats` 批次四起由
/// [`InsightStore::refresh_stats`] 写入；反序列化容忍缺键（`#[serde(default)]`），
/// None 时正向写盘也不写空 stats——不造假字段。
#[derive(Serialize, Deserialize)]
struct InsightLedger {
    version: u32,
    entries: Vec<InsightEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stats: Option<InsightStats>,
}

/// 洞察台账 store。实例只是路径视图（见模块注释），可任意克隆/重建。
pub struct InsightStore {
    path: PathBuf,
    activation_recurrence: u32,
}

/// 模块级全局 RMW 写锁：跨 store 实例串行化「读-改-写」全程。
/// WSAPI/CLI 每调用现构造 store 与注入闭包持有的 store 是不同实例，
/// 实例级锁锁不住彼此——与 `nemesis_config::config_write_lock` 同形。
static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

impl InsightStore {
    pub fn new(workspace: &Path) -> Self {
        Self::with_recurrence(workspace, DEFAULT_ACTIVATION_RECURRENCE)
    }

    /// 指定激活阈值构造（config `learn.activation_recurrence` 接线点）。
    /// <1 视为 1（首现即 active——无人值守设备的合法极端配置）。
    pub fn with_recurrence(workspace: &Path, recurrence: u32) -> Self {
        Self {
            path: workspace.join("forge").join("insights.json"),
            activation_recurrence: recurrence.max(1),
        }
    }

    /// 台账文件路径（WSAPI/CLI 展示用）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn empty_ledger() -> InsightLedger {
        InsightLedger {
            version: 1,
            entries: Vec::new(),
            stats: None,
        }
    }

    /// 读台账。文件缺席 = 空台账（合法初态）；损坏 = fail-closed 全不
    /// 加载（WARN + 坏文件挪 `.corrupt` 留档，已有 `.corrupt` 则带毫秒
    /// 后缀防覆盖）；BOM 容忍。
    fn load_ledger(&self) -> InsightLedger {
        let raw = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(_) => return Self::empty_ledger(),
        };
        let bytes: &[u8] = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &raw[3..]
        } else {
            &raw[..]
        };
        let text = String::from_utf8_lossy(bytes);
        match serde_json::from_str::<InsightLedger>(&text) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %e,
                    "[insights] 台账损坏，fail-closed 全不加载（坏文件留档 .corrupt）"
                );
                let corrupt = PathBuf::from(format!("{}.corrupt", self.path.display()));
                let target = if corrupt.exists() {
                    let millis = chrono::Utc::now().timestamp_millis();
                    PathBuf::from(format!("{}.{millis}.corrupt", self.path.display()))
                } else {
                    corrupt
                };
                let _ = std::fs::rename(&self.path, &target);
                Self::empty_ledger()
            }
        }
    }

    /// 原子写（temp+rename，nemesis-utils 韧性包装）。
    fn save_ledger(&self, ledger: &InsightLedger) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create forge dir: {e}"))?;
        }
        let json =
            serde_json::to_string_pretty(ledger).map_err(|e| format!("serialize insights: {e}"))?;
        nemesis_utils::write_file_atomic(&self.path.to_string_lossy(), json.as_bytes(), 0o600)
            .map_err(|e| format!("write insights: {e}"))
    }

    /// 入库简形态：不需要关心自动激活与否的调用方用这个（测试与旧调用
    /// 点保持签名不变）；完整形态见 [`Self::add_candidate_checked`]。
    pub fn add_candidate(
        &self,
        statement: &str,
        category: &str,
        evidence: &str,
        source: InsightSource,
        confidence: f64,
        scope: Option<&str>,
    ) -> Result<InsightEntry, String> {
        self.add_candidate_checked(statement, category, evidence, source, confidence, scope)
            .map(|(entry, _)| entry)
    }

    /// 入库完整形态：新指纹建条目（dialogue→candidate / manual→active）；
    /// 同指纹命中 = 复现（occurrences+1、刷新 last_seen、达标转 active），
    /// **不建新条目**——retired 态（尤其 vetoed）天然驻留压制，同指纹再
    /// 提案不复活。返回（命中/新落的条目，本轮是否触发自动激活）。
    ///
    /// `scope`（洞察页与场景拆分 2026-11-24）：场景归属——None = 对话洞察
    /// （主 loop 注入）/ Some(项目 id) = 项目洞察（该项目 loop 注入）。
    /// **只在新条目创建时落值**；同指纹复现命中 = 既有条目身份不变（scope
    /// 与 first_seen 同级，创建后不可变——同陈述跨场景复现不迁移归属）。
    ///
    /// 闸顺序（先硬边界后格式）：①安全词表（statement+evidence 双查，
    /// 命中拒绝落库）→ ②statement 非空 + ≤120 字（超限拒绝）→ ③category
    /// 空归 "general"、evidence 超 300 截断、confidence 钳制。
    ///
    /// 自动激活（复现达标 candidate→active = 进入注入面）时 loud warn——
    /// 注入面成员变化必须可观测（BUG-4；人工可随时 veto）。
    pub fn add_candidate_checked(
        &self,
        statement: &str,
        category: &str,
        evidence: &str,
        source: InsightSource,
        confidence: f64,
        scope: Option<&str>,
    ) -> Result<(InsightEntry, bool), String> {
        if nemesis_types::safety::violates_security_boundary(statement)
            || nemesis_types::safety::violates_security_boundary(evidence)
        {
            return Err(
                "洞察内容命中安全词表，拒绝落库（注入面绝不承载权限/安全语义）".to_string()
                    + &format!("：{}", truncate_for_error(statement)),
            );
        }
        let statement = statement.trim();
        if statement.is_empty() {
            return Err("statement 为空，拒绝落库".to_string());
        }
        if statement.chars().count() > STATEMENT_MAX_CHARS {
            return Err(format!(
                "statement 超 {} 字上限（{} 字），拒绝落库——请压缩措辞后再提交",
                STATEMENT_MAX_CHARS,
                statement.chars().count()
            ));
        }
        let category = {
            let c = category.trim();
            if c.is_empty() {
                "general".to_string()
            } else {
                c.to_string()
            }
        };
        let evidence: String = if evidence.chars().count() > EVIDENCE_MAX_CHARS {
            evidence.chars().take(EVIDENCE_MAX_CHARS).collect()
        } else {
            evidence.to_string()
        };
        let confidence = confidence.clamp(0.0, 1.0);
        let fp = fingerprint(statement);

        let _serial = WRITE_LOCK.lock();
        let mut ledger = self.load_ledger();
        let now = chrono::Local::now();
        let now_s = now.to_rfc3339();
        if let Some(existing) = ledger.entries.iter_mut().find(|e| e.fingerprint == fp) {
            existing.occurrences += 1;
            existing.last_seen = now_s.clone();
            let mut activated = false;
            if existing.status == InsightStatus::Candidate
                && existing.occurrences >= self.activation_recurrence
            {
                existing.status = InsightStatus::Active;
                existing.activated_at = Some(now_s.clone());
                activated = true;
                tracing::warn!(
                    id = %existing.id,
                    occurrences = existing.occurrences,
                    statement = %existing.statement,
                    "[insights] 洞察复现达标，自动激活进入注入面（人工可 veto）"
                );
            }
            let out = existing.clone();
            self.save_ledger(&ledger)?;
            return Ok((out, activated));
        }
        let entry = InsightEntry {
            id: next_id(&ledger.entries, now),
            fingerprint: fp,
            statement: statement.to_string(),
            category,
            evidence,
            source,
            confidence,
            occurrences: 1,
            first_seen: now_s.clone(),
            last_seen: now_s.clone(),
            status: match source {
                InsightSource::Manual => InsightStatus::Active,
                InsightSource::Dialogue => InsightStatus::Candidate,
            },
            retire_reason: None,
            // manual 落库即 active——激活时刻 = first_seen 同刻。
            activated_at: matches!(source, InsightSource::Manual).then(|| now_s.clone()),
            scope: scope
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string()),
        };
        ledger.entries.push(entry.clone());
        self.save_ledger(&ledger)?;
        Ok((entry, false))
    }

    /// 人工晋升 candidate → active（用户权威面；active 幂等；retired
    /// 拒绝——退役判定不因晋升请求翻转）。激活时刻记录首次晋升时刻
    /// （幂等重放不覆盖——窗口统计要的是「第一次生效」）。
    pub fn promote(&self, id: &str) -> Result<InsightEntry, String> {
        self.transition(id, |e| {
            if e.status == InsightStatus::Retired {
                return Err(format!(
                    "{} 已退役（{:?}），不可晋升",
                    e.id,
                    e.retire_reason.unwrap_or(RetireReason::Expired)
                ));
            }
            e.status = InsightStatus::Active;
            if e.activated_at.is_none() {
                e.activated_at = Some(chrono::Local::now().to_rfc3339());
            }
            Ok(())
        })
    }

    /// 人工否决 → retired(vetoed)。同指纹再提案从此驻留压制（永不复活）。
    pub fn veto(&self, id: &str) -> Result<InsightEntry, String> {
        self.retire(id, RetireReason::Vetoed)
    }

    /// 退役（记原因；幂等——已退役刷新原因与 last_seen 由调用方语义决定，
    /// v1 直接拒绝重复退役，保持动作面诚实）。
    pub fn retire(&self, id: &str, reason: RetireReason) -> Result<InsightEntry, String> {
        self.transition(id, |e| {
            if e.status == InsightStatus::Retired {
                return Err(format!("{} 已退役，不可重复退役", e.id));
            }
            e.status = InsightStatus::Retired;
            e.retire_reason = Some(reason);
            Ok(())
        })
    }

    fn transition(
        &self,
        id: &str,
        apply: impl FnOnce(&mut InsightEntry) -> Result<(), String>,
    ) -> Result<InsightEntry, String> {
        let _serial = WRITE_LOCK.lock();
        let mut ledger = self.load_ledger();
        let entry = ledger
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("未找到条目 {id}（forge insights list 查看现有 id）"))?;
        apply(entry)?;
        let out = entry.clone();
        self.save_ledger(&ledger)?;
        Ok(out)
    }

    /// 全量条目（可选 status 过滤；盘面只读视图）。
    pub fn list(&self, status: Option<InsightStatus>) -> Result<Vec<InsightEntry>, String> {
        let ledger = self.load_ledger();
        Ok(ledger
            .entries
            .into_iter()
            .filter(|e| status.is_none_or(|s| e.status == s))
            .collect())
    }

    /// 注入段渲染：active 条目排序（confidence desc, id asc）截 cap 5，
    /// 渲染侧词表复扫；空 = None（段缺席零字节）。每 build 调用，从盘
    /// 全量重读（merged snapshot 字节稳定语义：文件不变 ⇒ 输出字节不变）。
    /// 对话洞察视图（scope = None）——主 loop（对话组会话）消费入口。
    pub fn render_section(&self) -> Option<String> {
        self.render_section_for(None)
    }

    /// 场景过滤渲染（洞察页与场景拆分 2026-11-24）：`scope = None` 只出
    /// 对话洞察（entry.scope 缺席）；`Some(pid)` 只出该项目的条目——项目
    /// 之间不串台，项目洞察也不泄入对话会话。其余语义与 [`Self::render_section`]
    /// 逐字一致（排序/cap/词表/字节稳定）。
    pub fn render_section_for(&self, scope: Option<&str>) -> Option<String> {
        let ledger = self.load_ledger();
        let mut actives: Vec<&InsightEntry> = ledger
            .entries
            .iter()
            .filter(|e| e.status == InsightStatus::Active)
            .filter(|e| e.scope.as_deref() == scope)
            .filter(|e| !nemesis_types::safety::violates_security_boundary(&e.statement))
            .collect();
        if actives.is_empty() {
            return None;
        }
        actives.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        actives.truncate(INJECTION_CAP);
        let mut out = String::from("# Active Insights\n");
        for e in actives {
            out.push_str(&format!("- [{}] {}\n", e.category, e.statement));
        }
        out.push_str("\n(以上是本机自学习积累的行为准则，持续遵循；与新指令冲突时向用户说明。)");
        Some(out)
    }

    /// 实效统计刷新（自学习铺路批次四）：纠正率 delta 前后窗口对照，
    /// 重算落台账 `stats` 字段。全量重算 + 原子写（与 ledger RMW 同锁
    /// 串行）；统计时刻的候选面从 `<forge>/evidence/candidates.jsonl`
    /// 读（与 session_evidence 同源，不复制台账）。
    pub fn refresh_stats(&self) -> Result<InsightStats, String> {
        let _serial = WRITE_LOCK.lock();
        let mut ledger = self.load_ledger();
        let stats = self.compute_stats(&ledger);
        ledger.stats = Some(stats.clone());
        self.save_ledger(&ledger)?;
        Ok(stats)
    }

    /// 读最近一次落盘的实效统计（`refresh_stats` 写入；从未刷新 = None）。
    pub fn stats(&self) -> Option<InsightStats> {
        self.load_ledger().stats
    }

    /// 统计推导（纯读：candidates.jsonl + 台账条目 → InsightStats）。
    fn compute_stats(&self, ledger: &InsightLedger) -> InsightStats {
        let mut stats = InsightStats {
            entries_candidate: ledger
                .entries
                .iter()
                .filter(|e| e.status == InsightStatus::Candidate)
                .count() as u64,
            entries_active: ledger
                .entries
                .iter()
                .filter(|e| e.status == InsightStatus::Active)
                .count() as u64,
            entries_retired: ledger
                .entries
                .iter()
                .filter(|e| e.status == InsightStatus::Retired)
                .count() as u64,
            activated_count: ledger
                .entries
                .iter()
                .filter(|e| e.activated_at.is_some())
                .count() as u64,
            generated_at: chrono::Local::now().to_rfc3339(),
            ..Default::default()
        };

        // 窗口切分点 = 最早激活时刻。旧台账手编/历史条目无 activated_at
        // 不参与切分（诚实：没有激活时刻就没有窗口语义）。
        let pivot = ledger
            .entries
            .iter()
            .filter_map(|e| e.activated_at.as_deref())
            .filter_map(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .min();

        // 候选面：总行数 + 消费游标 + 可解析 ts 的前后窗计数（一次读全
        // 台账；撕裂行 read_all 已跳过并 WARN——窗口计数与总数同源）。
        let mut first_candidate_ts: Option<chrono::DateTime<chrono::FixedOffset>> = None;
        if let Some(forge_dir) = self.path.parent() {
            let store = crate::session_evidence::CandidateStore::from_forge_dir(forge_dir);
            let rows = store.read_all();
            stats.candidates_total = rows.len() as u64;
            stats.candidates_consumed =
                store.consumed_count(crate::session_evidence::CONSUMER_DIALOGUE);
            for c in rows {
                let Ok(t) = chrono::DateTime::parse_from_rfc3339(&c.draft.ts) else {
                    continue; // ts 不可解析 = 不进窗口（诚实不硬猜时区）
                };
                first_candidate_ts = Some(first_candidate_ts.map_or(t, |m| m.min(t)));
                if let Some(p) = pivot
                    && t < p
                {
                    stats.corrections_before += 1;
                } else if pivot.is_some() {
                    stats.corrections_after += 1;
                }
            }
        }

        if let (Some(p), Some(first)) = (pivot, first_candidate_ts) {
            stats.activation_pivot = p.to_rfc3339();
            let before_days =
                p.signed_duration_since(first).num_milliseconds() as f64 / 86_400_000.0;
            let now = chrono::Local::now().fixed_offset();
            let after_days = now.signed_duration_since(p).num_milliseconds() as f64 / 86_400_000.0;
            stats.window_days_before = before_days.max(0.0);
            stats.window_days_after = after_days.max(0.0);
            if stats.window_days_before > 0.0 {
                stats.correction_rate_before =
                    stats.corrections_before as f64 / stats.window_days_before;
            }
            if stats.window_days_after > 0.0 {
                stats.correction_rate_after =
                    stats.corrections_after as f64 / stats.window_days_after;
            }
        }
        stats
    }
}

/// statement 指纹：规范化（trim + 折叠连续空白为单空格 + 小写）后
/// SHA256 前 16 hex。规范化只折**形态**差异，不折措辞差异——语义级
/// 去重归对话引擎对账阶段（批次三），指纹层宁勿滥不误合。
pub fn fingerprint(statement: &str) -> String {
    let mut norm = String::with_capacity(statement.len());
    let mut last_ws = true; // trim 语义：首部空白不进
    for ch in statement.trim().chars() {
        if ch.is_whitespace() {
            if !last_ws {
                norm.push(' ');
            }
            last_ws = true;
        } else {
            norm.push(ch);
            last_ws = false;
        }
    }
    if norm.ends_with(' ') {
        norm.pop();
    }
    let digest = sha2::Sha256::digest(norm.to_lowercase().as_bytes());
    let mut hex = String::with_capacity(16);
    for b in digest.iter().take(8) {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// 生成下一条目 id：`ins-<yyyymmdd>-<seq:03>`，seq = 同日最大序号 +1
/// （全量扫描；删除不留洞不影响——序号只求唯一不求连续）。
fn next_id(entries: &[InsightEntry], now: chrono::DateTime<chrono::Local>) -> String {
    let prefix = format!("ins-{}-", now.format("%Y%m%d"));
    let mut max_seq = 0u32;
    for e in entries {
        if let Some(seq) =
            e.id.strip_prefix(&prefix)
                .and_then(|s| s.parse::<u32>().ok())
        {
            max_seq = max_seq.max(seq);
        }
    }
    format!("{prefix}{:03}", max_seq + 1)
}

/// 报错文案里的 statement 截断（只用于人读错误，120 字上限之外再截 60）。
fn truncate_for_error(s: &str) -> String {
    if s.chars().count() <= 60 {
        s.to_string()
    } else {
        s.chars().take(60).collect::<String>() + "…"
    }
}

/// status 过滤参数解析（WSAPI/CLI 共用；`""`/`"all"`/缺省 = 不过滤，
/// 未知值 loud 拒绝不静默吞）。
pub fn parse_status(s: &str) -> Result<Option<InsightStatus>, String> {
    match s.trim().to_lowercase().as_str() {
        "" | "all" => Ok(None),
        "candidate" => Ok(Some(InsightStatus::Candidate)),
        "active" => Ok(Some(InsightStatus::Active)),
        "retired" => Ok(Some(InsightStatus::Retired)),
        other => Err(format!(
            "未知 status 过滤值 `{other}`（支持 all|candidate|active|retired）"
        )),
    }
}

/// retire 原因解析（WSAPI `insights.update` reason 参数；veto 动作不经过
/// 此函数——vetoed 是动作语义自带）。缺省 superseded。
pub fn parse_retire_reason(s: &str) -> Result<RetireReason, String> {
    match s.trim().to_lowercase().as_str() {
        "corrected" => Ok(RetireReason::Corrected),
        "superseded" | "" => Ok(RetireReason::Superseded),
        "expired" => Ok(RetireReason::Expired),
        other => Err(format!(
            "未知 retire reason `{other}`（支持 corrected|superseded|expired）"
        )),
    }
}

#[cfg(test)]
mod tests;
