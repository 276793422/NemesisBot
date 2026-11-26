//! 学习对话引擎——自学习铺路批次三「路面」（2026-11-24）。
//!
//! 「LLM 当脑子、Rust 当路」的路面本体：四阶段多轮对话（提议→定形→
//! 对账→定稿），学什么、学成什么形态、内容怎么写全部由对话里的 LLM
//! 判断；Rust 只摆证据（[`crate::session_evidence::EvidencePack`]）、运
//! 对话（把前几轮记录串进下一阶段 user prompt）、落载体（洞察过闸进
//! [`InsightStore`]，skill 草稿随 outcome 交付）与跑护栏（预算/estop/
//! 互斥/留痕）。协议 prompt 资产在
//! [`nemesis_prompts::forge`]（阶段标记 `# 学习对话阶段 N/4` 是引擎×
//! 测试模型对键契约，勿漂移）。
//!
//! 护栏纪律（§7 失败路径）：
//! - 预算：`budget_tokens` 作为每阶段 max_tokens 下传；证据包体积由组
//!   装器封顶（超限截证据包不截轮次），`truncated` 诚实透传进 outcome；
//! - 非法 JSON：每阶段 lenient 解析 + 附加提示重试一次，第二次仍非法 =
//!   该阶段条目诚实丢弃（transcript 留现场，零落库污染）；
//! - estop：入口 + 每阶段之间检查，**且阶段内 LLM 调用进行中按 300ms
//!   轮询中止**（`call_phase` 内 `select!`，在飞调用即弃——estop 不等
//!   600s 超时）；中断时 transcript-so-far 照常留痕、游标不推进；
//! - 互斥：进程级 `DIALOGUE_BUSY` CAS（cycle 撞手动）+ 跨进程 advisory
//!   锁文件 `learn.lock`（CLI 与 gateway 是两个进程，进程级 CAS 盖不住
//!   跨进程并发双写 insights.json；陈旧锁 >2h 诚实接管）——任一层
//!   占用后者都诚实报「已在进行」；
//! - 落库：洞察走 [`InsightStore`] 全闸（安全词表/长度/钳制），被拒条
//!   目记入 `dropped` 不炸对话；对账判 supersede 且目标为 active 洞察
//!   时自动退役（`RetireReason::Superseded`）——对话已裁定旧条目错误，
//!   留着 = 注入面自相矛盾；复现达标自动激活的条目 id 记入
//!   `LearnDialogueOutcome::activated`（激活进注入面必须可观测）。
//!
//! 真相源：`docs/PLAN/2026-11-24_自学习铺路计划.md` §3 / §6 / §7。
//!
//! 测试：[`tests`]（独立文件，仓库内联测试禁令合规）。

use crate::insights::{InsightSource, InsightStatus, InsightStore, RetireReason};
use crate::reflector_llm::LLMCaller;
use crate::session_evidence::EvidencePack;
use crate::userprofile::reflection::REFLECT_CATEGORIES;
use crate::userprofile::store::{CONFIDENCE_THRESHOLD, HabitEntry};
use nemesis_prompts::forge::{
    LEARN_DIALOGUE_SYSTEM_PROMPT, learn_phase1_prompt, learn_phase2_prompt, learn_phase3_prompt,
    learn_phase4_prompt,
};
use nemesis_types::safety::violates_security_boundary;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 全局对话互斥（cycle × 手动触发）：CAS false→true 拿锁，
/// [`BusyGuard`] Drop 释放。与 `insights::WRITE_LOCK` 同形（进程级）。
static DIALOGUE_BUSY: AtomicBool = AtomicBool::new(false);

/// 跨进程锁文件陈旧阈值（秒）。对话链理论上界 ≈ 4 阶段 × 600s 调用超时、
/// 加上重试与闸（远小于 2h）；锁文件在场且超龄 = 上次进程死亡残留（进程
/// 不可能还在跑），诚实接管。
const LOCK_STALE_SECS: u64 = 2 * 3600;

/// estop 阶段内轮询步进（毫秒）。`call_phase` 的 `select!` 臂步进粒度
/// ——estop 触发最迟 ~300ms 后打断在飞 LLM 调用。
const ESTOP_POLL_MS: u64 = 300;

/// 接口二抛入来源标记（双缝互通 2026-11-25 缝二）：学习对话产出的用户
/// 侧结论经 `store::ingest_external_entry` 入画像时强制携带（外部经验
/// 必须可溯源到抛入方——bridge 纪律）。
pub const LEARN_DIALOGUE_SOURCE: &str = "learn_dialogue";

/// 测试专用进程级串行闸：`DIALOGUE_BUSY` 是进程全局态，cargo 并行测试
/// 线程互踩会把「已在进行」误判成用例失败（background_registry TEST_LOCK
/// 同款纪律）。所有触达 `run`/`DIALOGUE_BUSY` 的用例首行持锁。
#[cfg(test)]
pub(crate) static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        DIALOGUE_BUSY.store(false, Ordering::Release);
    }
}

/// 跨进程 advisory 锁：`DIALOGUE_BUSY` 只盖本进程（cycle 撞手动），CLI
/// `forge learn run` 与 gateway 是两个进程——跨进程并发 = insights.json
/// 双写（原子写保证不损坏，后写覆盖前写）。锁文件 `create_new` 原子占
/// 位（内容 pid+时间戳供诊断），Drop 释放。陈旧锁（> [`LOCK_STALE_SECS`]
/// ）接管一次：移除后重抢；接管竞态由 `create_new` 兜底（抢失败的一
/// 方诚实报忙）。
struct FileLockGuard {
    path: PathBuf,
    owned: bool,
}

impl FileLockGuard {
    fn acquire(path: &Path) -> Result<Self, String> {
        for attempt in 0..2u8 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut f) => {
                    use std::io::Write as _;
                    let _ = writeln!(
                        f,
                        "pid={} started={}",
                        std::process::id(),
                        chrono::Local::now().to_rfc3339()
                    );
                    return Ok(Self {
                        path: path.to_path_buf(),
                        owned: true,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .map(|d| d.as_secs());
                    if age.is_some_and(|a| a > LOCK_STALE_SECS) && attempt == 0 {
                        tracing::warn!(
                            path = %path.display(),
                            age_secs = age,
                            "[learn] learn.lock 陈旧（持锁进程已死亡的残留），接管"
                        );
                        // 韧性删除（fsutil 删除纪律）：AV/索引器瞬时句柄正扫
                        // lock 文件时单发必败 → attempt 已耗尽误报「对话进行
                        // 中」；包装内退避重试吸收瞬时窗口，耗尽 warn 留痕。
                        let _ = nemesis_utils::fsutil::remove_file_resilient(path);
                        continue;
                    }
                    return Err(format!(
                        "学习对话已在进行中（learn.lock 在场，{}；本次触发被拒绝）",
                        age.map_or_else(|| "持锁时长未知".to_string(), |a| format!("已持锁 {a}s"))
                    ));
                }
                Err(e) => return Err(format!("learn.lock 创建失败：{e}")),
            }
        }
        // 陈旧接管重抢仍撞 AlreadyExists = 别的进程正在接管（或再次落
        // 锁）——诚实报忙。循环最多两轮，此处只在第二轮 AlreadyExists
        // 分支未 return 时到达不了：上面的 AlreadyExists 分支 attempt==1
        // 时直接 return Err。为编译器收敛而设的兜底。
        Err("学习对话已在进行中（learn.lock 在场；本次触发被拒绝）".to_string())
    }
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        if self.owned {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    "[learn] learn.lock 释放失败（陈锁接管面兜底，不影响结果）"
                ),
            }
        }
    }
}

/// 已落库洞察（store 发号后的回执形态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedInsightOut {
    /// `ins-<yyyymmdd>-<seq>`（[`InsightStore`] 发号）。
    pub id: String,
    pub statement: String,
}

/// skill 草稿（批次三只随 outcome 交付，**不落 workspace**——装前
/// tests 实跑过闸落盘归批次四）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSkillDraft {
    pub slug: String,
    /// SKILL.md 全文（含 frontmatter 与可选 tests 段）。
    pub skill_md: String,
    /// frontmatter tests 段可被 [`crate::skill_tests`] 解析（批次四实跑
    /// 闸的预探测；false = 无 tests 段或段非法，批次四会拒绝装载）。
    pub has_tests: bool,
}

/// 用户侧结论回抛记录（双缝互通 2026-11-25 缝二）。`id` = 提案 id（观察
/// 提名落盘时 ObservationEntry 的真实 id 由 bridge 内部发号，不在调用方
/// 可见范围——outcome 记提案 id 供回查 transcript）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFindingOut {
    /// 处置：`ingested`（过闸直入画像 profile.json）| `observation`
    /// （低置信/非法词表/入画像被拒 → 观察提名队列人工裁决）。
    pub disposition: String,
    /// 提案 id（回查 transcript 用）。
    pub id: String,
    /// 结论陈述。
    pub statement: String,
    /// 类别（画像词表原值；非法词表原样记录供裁决参考）。
    pub category: String,
    /// 置信度。
    pub confidence: f64,
}

/// 对话产物 + 过程诚实记录（落 `outcome.json` 留痕）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearnDialogueOutcome {
    /// 已落库洞察条目。
    pub insights: Vec<LearnedInsightOut>,
    /// skill 草稿（对话原始产出；落盘与否见 [`Self::gate`] 逐稿记录）。
    pub skills: Vec<LearnedSkillDraft>,
    /// 装前 tests 闸逐稿记录（自学习铺路批次四）：实跑过闸才落
    /// `workspace/skills/{slug}-forge/`；拒绝/弃稿原因诚实留痕。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate: Vec<crate::skill_gate::DraftGateRecord>,
    /// 用户侧结论回抛记录（缝二：subject=user 条目的分流结果；serde
    /// default 空 = 旧 outcome 兼容）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_findings: Vec<UserFindingOut>,
    /// 「没有值得学的」一等答案（proposals 空 / 全部条目中途丢弃时给出）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_reason: Option<String>,
    /// 阶段 1 提议总数（过程可观测）。
    pub proposals_total: usize,
    /// 逐条丢弃记录（非法 JSON / 未知 id / 落库被拒 / none……）。
    pub dropped: Vec<String>,
    /// supersede 执行记录（被退役的旧条目 id）。
    pub retired: Vec<String>,
    /// 本轮触发自动激活（复现达标 candidate→active）的洞察 id 列表
    /// （激活 = 进入注入面，成员变化必须可观测——outcome/WSAPI/前端
    /// 三面透传）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activated: Vec<String>,
    /// 证据包是否超预算截断（组装器标记透传）。
    pub truncated: bool,
    /// 完整走完的阶段数（1–4；诚实反映中止点）。
    pub phases_completed: u8,
}

/// 阶段出口：传输失败（对话中止）vs 两次解析失败（该阶段条目诚实丢弃，
/// 携带全部原始回复）。
enum PhaseExit {
    Transport(String),
    Unparseable(Vec<String>),
}

/// estop 轮询等待（[`ESTOP_POLL_MS`] 步进；只在 stop 触发时完成）。
async fn wait_stopped(is_stopped: &(dyn Fn() -> bool + Send + Sync)) {
    loop {
        if is_stopped() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(ESTOP_POLL_MS)).await;
    }
}

/// 单阶段调用 + lenient 解析 + 单次重试。第二次仍非法 =
/// [`PhaseExit::Unparseable`]（带**全部**原始回复供 transcript 留现场——
/// 只留最后一次会把第一次的现场销毁）。
///
/// estop 阶段内中止：在飞 LLM 调用与急停轮询 `select!` 竞速——estop
/// 先触发即丢弃在飞 future，返回 [`PhaseExit::Transport`]（诚实错误，
/// 游标不推进、下轮诚实重放）。`biased` + stop 臂在前：首次轮询即检查
/// `is_stopped()`，已停状态连请求都不发出。
async fn call_phase<T: DeserializeOwned>(
    caller: &dyn LLMCaller,
    user_prompt: String,
    budget_tokens: u32,
    is_stopped: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(T, String), PhaseExit> {
    let mut prompt = user_prompt;
    let mut raws: Vec<String> = Vec::new();
    for attempt in 0..2u8 {
        if attempt == 1 {
            prompt.push_str(
                "\n\n（上一次回复不是合法 JSON 对象。请只回复一个合法 JSON 对象，不要输出任何其他文本。）",
            );
        }
        let chat = caller.chat(
            LEARN_DIALOGUE_SYSTEM_PROMPT,
            &prompt,
            Some(budget_tokens as i64),
        );
        tokio::pin!(chat);
        let raw = tokio::select! {
            biased;
            _ = wait_stopped(is_stopped) => {
                return Err(PhaseExit::Transport("急停（estop）中止".into()));
            }
            res = &mut chat => res.map_err(PhaseExit::Transport)?,
        };
        if let Ok(v) = parse_json::<T>(&raw) {
            return Ok((v, raw));
        }
        raws.push(raw);
    }
    Err(PhaseExit::Unparseable(raws))
}

fn parse_json<T: DeserializeOwned>(raw: &str) -> Result<T, ()> {
    let v = crate::reflector_llm::extract_json(raw).ok_or(())?;
    serde_json::from_value(v).map_err(|_| ())
}

// ── 各阶段响应 serde 形态（私有；契约见 nemesis-prompts::forge）───────

#[derive(Deserialize)]
struct Phase1Resp {
    #[serde(default)]
    proposals: Vec<Proposal>,
    #[serde(default)]
    empty_reason: Option<String>,
}

#[derive(Deserialize)]
struct Proposal {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    observation: String,
    #[serde(default)]
    lesson: String,
}

#[derive(Deserialize)]
struct Phase2Resp {
    #[serde(default)]
    items: Vec<FormDecision>,
}

#[derive(Deserialize)]
struct FormDecision {
    id: String,
    form: String,
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
struct Phase3Resp {
    #[serde(default)]
    items: Vec<VerdictDecision>,
}

#[derive(Deserialize)]
struct VerdictDecision {
    id: String,
    verdict: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    note: String,
}

#[derive(Deserialize)]
struct Phase4Resp {
    #[serde(default)]
    insights: Vec<FinalInsight>,
    #[serde(default)]
    skills: Vec<FinalSkill>,
}

#[derive(Deserialize)]
struct FinalInsight {
    id: String,
    /// 主语分型（双缝互通缝二）：`assistant`（缺省 serde default——旧输出
    /// 零迁移）| `user`（对用户本人习惯/偏好的结论，走画像回抛路由）。
    #[serde(default)]
    subject: String,
    statement: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    confidence: f64,
}

#[derive(Deserialize)]
struct FinalSkill {
    id: String,
    /// 主语分型（防御性）：恒 `assistant`——prompt 已约束 user 主语只产
    /// 出洞察；模型仍产出 user 主语 skill 时 Rust 侧诚实丢弃（用户习惯
    /// 不写成 agent 技能，不猜）。
    #[serde(default)]
    subject: String,
    slug: String,
    #[serde(default)]
    skill_md: String,
}

/// 定形裁决（非法值已在对账映射时剔除）。
#[derive(Clone, Copy, PartialEq)]
enum Form {
    Insight,
    Skill,
    Both,
}

impl Form {
    fn wants_insight(self) -> bool {
        matches!(self, Form::Insight | Form::Both)
    }
    fn wants_skill(self) -> bool {
        matches!(self, Form::Skill | Form::Both)
    }
    fn label(self) -> &'static str {
        match self {
            Form::Insight => "insight",
            Form::Skill => "skill",
            Form::Both => "both",
        }
    }
}

/// 对账保留项（supersede 带被取代目标）。
struct Kept {
    id: String,
    supersede_target: Option<String>,
}

/// 运行一次学习对话（调用方无需持锁——入口 CAS + 锁文件双层互斥，撞
/// 车方诚实报「已在进行」）。`trace_dir` = `<workspace>/forge/learn/<ts>_<trigger>`，
/// 所有出口（成功/中止/丢弃）都落 `transcript.md` + `outcome.json`；
/// `lock_path` = `<workspace>/forge/learn.lock`（跨进程 advisory 锁）。
/// `profile_ws` = 画像工作区根（缝二主语复核臂的回抛目标；None =
/// headless/无工作区形态——subject=user 条目全部诚实丢弃不回抛）。
#[allow(clippy::too_many_arguments)]
pub async fn run(
    caller: &dyn LLMCaller,
    pack: &EvidencePack,
    insights: &InsightStore,
    skills_digest: &str,
    is_stopped: &(dyn Fn() -> bool + Send + Sync),
    trace_dir: &Path,
    lock_path: &Path,
    budget_tokens: u32,
    scope: Option<&str>,
    profile_ws: Option<&Path>,
) -> Result<LearnDialogueOutcome, String> {
    if DIALOGUE_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err("学习对话已在进行中，本次触发被拒绝".to_string());
    }
    let _guard = BusyGuard;
    // 跨进程锁：CAS 过了再抢锁文件（锁文件失败 = 别的进程在跑）。声明
    // 在 BusyGuard 之后——Drop 逆序，锁文件先释放、CAS 后释放，两层
    // 对称收口。
    let _file_lock = FileLockGuard::acquire(lock_path)?;

    let mut outcome = LearnDialogueOutcome {
        truncated: pack.truncated,
        ..Default::default()
    };
    // transcript 以证据包开卷；后续每阶段追加完整往返（### user 提问 +
    // ### assistant 模型原始回复）——无状态单调用 API 下，transcript 就是
    // 多轮对话载体，后一阶段必须看得见前一阶段的问与答。
    let mut transcript = format!("# 学习对话留痕\n\n## 证据包\n\n{}\n", pack.text);
    let persist = |t: &str, o: &LearnDialogueOutcome| persist_trace(trace_dir, t, o);

    if is_stopped() {
        let err = "学习对话被拒绝：急停（estop）冻结中".to_string();
        persist(&transcript, &outcome);
        return Err(err);
    }

    // ── 阶段 1：提议 ─────────────────────────────────────────────────
    let p1_prompt = learn_phase1_prompt(&pack.text);
    transcript.push_str("\n## 阶段 1 提议\n\n### user\n\n");
    transcript.push_str(&p1_prompt);
    transcript.push('\n');
    let (p1, raw) =
        match call_phase::<Phase1Resp>(caller, p1_prompt, budget_tokens, is_stopped).await {
            Ok(v) => v,
            Err(PhaseExit::Transport(e)) => {
                let err = format!("学习对话阶段1（提议）LLM 调用失败：{e}");
                persist(&transcript, &outcome);
                return Err(err);
            }
            Err(PhaseExit::Unparseable(raws)) => {
                transcript.push_str("\n### assistant（两次非法 JSON）\n\n");
                for r in &raws {
                    transcript.push_str(r);
                    transcript.push_str("\n\n---\n\n");
                }
                outcome.phases_completed = 1;
                outcome.empty_reason = Some("提议阶段两次输出均非合法 JSON，本批诚实放弃".into());
                persist(&transcript, &outcome);
                return Ok(outcome);
            }
        };
    transcript.push_str("\n### assistant\n\n");
    transcript.push_str(&raw);
    transcript.push('\n');
    outcome.proposals_total = p1.proposals.len();
    if p1.proposals.is_empty() {
        outcome.phases_completed = 1;
        outcome.empty_reason = Some(
            p1.empty_reason
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| "模型未说明理由".into()),
        );
        persist(&transcript, &outcome);
        return Ok(outcome);
    }

    // ── 阶段 2：定形（注入经济学）───────────────────────────────────
    if is_stopped() {
        let err = "学习对话被急停中断（阶段 2 前）".to_string();
        persist(&transcript, &outcome);
        return Err(err);
    }
    let p2_prompt = learn_phase2_prompt(&transcript);
    transcript.push_str("\n## 阶段 2 定形\n\n### user\n\n");
    transcript.push_str(&p2_prompt);
    transcript.push('\n');
    let (p2, raw) =
        match call_phase::<Phase2Resp>(caller, p2_prompt, budget_tokens, is_stopped).await {
            Ok(v) => v,
            Err(PhaseExit::Transport(e)) => {
                let err = format!("学习对话阶段2（定形）LLM 调用失败：{e}");
                persist(&transcript, &outcome);
                return Err(err);
            }
            Err(PhaseExit::Unparseable(raws)) => {
                transcript.push_str("\n### assistant（两次非法 JSON）\n\n");
                for r in &raws {
                    transcript.push_str(r);
                    transcript.push_str("\n\n---\n\n");
                }
                outcome.phases_completed = 2;
                outcome
                    .dropped
                    .push("阶段 2（定形）两次输出均非合法 JSON，全部条目放弃".into());
                outcome.empty_reason = Some("定形阶段输出非法，全部条目放弃".into());
                persist(&transcript, &outcome);
                return Ok(outcome);
            }
        };
    transcript.push_str("\n### assistant\n\n");
    transcript.push_str(&raw);
    transcript.push('\n');

    let valid_ids: HashSet<&str> = p1.proposals.iter().map(|p| p.id.as_str()).collect();
    let mut formed: Vec<(String, Form)> = Vec::new();
    let mut decided = HashSet::new();
    for item in p2.items {
        if !valid_ids.contains(item.id.as_str()) {
            outcome
                .dropped
                .push(format!("定形：未知条目 id «{}»，忽略", item.id));
            continue;
        }
        decided.insert(item.id.clone());
        match item.form.trim().to_lowercase().as_str() {
            "insight" => formed.push((item.id, Form::Insight)),
            "skill" => formed.push((item.id, Form::Skill)),
            "both" => formed.push((item.id, Form::Both)),
            "none" => outcome.dropped.push(format!(
                "{}：定形为 none（{}）——不沉淀",
                item.id,
                if item.reason.trim().is_empty() {
                    "未说明"
                } else {
                    &item.reason
                }
            )),
            other => outcome
                .dropped
                .push(format!("{}：定形值非法 «{other}»，该项丢弃", item.id)),
        }
    }
    for p in &p1.proposals {
        if !decided.contains(&p.id) {
            outcome
                .dropped
                .push(format!("{}：定形阶段未裁决，丢弃", p.id));
        }
    }
    if formed.is_empty() {
        outcome.phases_completed = 2;
        outcome.empty_reason = Some("定形阶段全部条目判 none / 丢弃".into());
        persist(&transcript, &outcome);
        return Ok(outcome);
    }

    // ── 阶段 3：对账（对照台账 + 技能库去重）────────────────────────
    if is_stopped() {
        let err = "学习对话被急停中断（阶段 3 前）".to_string();
        persist(&transcript, &outcome);
        return Err(err);
    }
    // 台账对账原料 = 注入段同源渲染（模型在对话里看到的与运行时看到的
    // 一致，「是否重复」才有同一参照系）。
    let ledger_digest = insights.render_section().unwrap_or_default();
    let p3_prompt = learn_phase3_prompt(&transcript, &ledger_digest, skills_digest);
    transcript.push_str("\n## 阶段 3 对账\n\n### user\n\n");
    transcript.push_str(&p3_prompt);
    transcript.push('\n');
    let (p3, raw) =
        match call_phase::<Phase3Resp>(caller, p3_prompt, budget_tokens, is_stopped).await {
            Ok(v) => v,
            Err(PhaseExit::Transport(e)) => {
                let err = format!("学习对话阶段3（对账）LLM 调用失败：{e}");
                persist(&transcript, &outcome);
                return Err(err);
            }
            Err(PhaseExit::Unparseable(raws)) => {
                transcript.push_str("\n### assistant（两次非法 JSON）\n\n");
                for r in &raws {
                    transcript.push_str(r);
                    transcript.push_str("\n\n---\n\n");
                }
                outcome.phases_completed = 3;
                outcome
                    .dropped
                    .push("阶段 3（对账）两次输出均非合法 JSON，全部条目放弃".into());
                outcome.empty_reason = Some("对账阶段输出非法，全部条目放弃".into());
                persist(&transcript, &outcome);
                return Ok(outcome);
            }
        };
    transcript.push_str("\n### assistant\n\n");
    transcript.push_str(&raw);
    transcript.push('\n');

    let mut kept: Vec<Kept> = Vec::new();
    let mut adjudicated = HashSet::new();
    for item in p3.items {
        if !valid_ids.contains(item.id.as_str()) {
            outcome
                .dropped
                .push(format!("对账：未知条目 id «{}»，忽略", item.id));
            continue;
        }
        adjudicated.insert(item.id.clone());
        match item.verdict.trim().to_lowercase().as_str() {
            "keep" => kept.push(Kept {
                id: item.id,
                supersede_target: None,
            }),
            "supersede" => kept.push(Kept {
                id: item.id,
                supersede_target: Some(item.target.trim().to_string()),
            }),
            "duplicate" => outcome.dropped.push(format!(
                "{}：对账 duplicate（与 {} 重复）——不沉淀{}",
                item.id,
                if item.target.trim().is_empty() {
                    "现有条目"
                } else {
                    item.target.trim()
                },
                note_suffix(&item.note)
            )),
            "conflict_drop" => outcome.dropped.push(format!(
                "{}：对账 conflict_drop（证据不足推翻 {}）——不沉淀{}",
                item.id,
                if item.target.trim().is_empty() {
                    "现有条目"
                } else {
                    item.target.trim()
                },
                note_suffix(&item.note)
            )),
            other => outcome
                .dropped
                .push(format!("{}：对账值非法 «{other}»，该项丢弃", item.id)),
        }
    }
    for (id, form) in &formed {
        if !adjudicated.contains(id) {
            outcome.dropped.push(format!(
                "{id}：对账阶段未裁决（定形 {}），丢弃",
                form.label()
            ));
        }
    }
    if kept.is_empty() {
        outcome.phases_completed = 3;
        outcome.empty_reason = Some("对账阶段全部条目判重复 / 冲突 / 丢弃".into());
        persist(&transcript, &outcome);
        return Ok(outcome);
    }

    // ── 阶段 4：定稿 ────────────────────────────────────────────────
    if is_stopped() {
        let err = "学习对话被急停中断（阶段 4 前）".to_string();
        persist(&transcript, &outcome);
        return Err(err);
    }
    let kept_render = render_kept_items(&p1.proposals, &formed, &kept);
    let p4_prompt = learn_phase4_prompt(&transcript, &kept_render);
    transcript.push_str("\n## 阶段 4 定稿\n\n### user\n\n");
    transcript.push_str(&p4_prompt);
    transcript.push('\n');
    let (p4, raw) =
        match call_phase::<Phase4Resp>(caller, p4_prompt, budget_tokens, is_stopped).await {
            Ok(v) => v,
            Err(PhaseExit::Transport(e)) => {
                let err = format!("学习对话阶段4（定稿）LLM 调用失败：{e}");
                persist(&transcript, &outcome);
                return Err(err);
            }
            Err(PhaseExit::Unparseable(raws)) => {
                transcript.push_str("\n### assistant（两次非法 JSON）\n\n");
                for r in &raws {
                    transcript.push_str(r);
                    transcript.push_str("\n\n---\n\n");
                }
                outcome.phases_completed = 3;
                outcome
                    .dropped
                    .push("阶段 4（定稿）两次输出均非合法 JSON，全部条目放弃".into());
                outcome.empty_reason = Some("定稿阶段输出非法，全部条目放弃".into());
                persist(&transcript, &outcome);
                return Ok(outcome);
            }
        };
    transcript.push_str("\n### assistant\n\n");
    transcript.push_str(&raw);
    transcript.push('\n');

    let kept_ids: HashSet<&str> = kept.iter().map(|k| k.id.as_str()).collect();
    let form_of = |id: &str| formed.iter().find(|(fid, _)| fid == id).map(|(_, f)| *f);

    // 洞察落库（store 全闸把守；被拒条目诚实记录不炸对话）。
    for fi in p4.insights {
        if !kept_ids.contains(fi.id.as_str()) {
            outcome
                .dropped
                .push(format!("insight {}：不在对账保留清单，丢弃", fi.id));
            continue;
        }
        if !form_of(&fi.id).map(Form::wants_insight).unwrap_or(false) {
            outcome
                .dropped
                .push(format!("insight {}：定形未选 insight 形态，丢弃", fi.id));
            continue;
        }
        // 主语复核臂（双缝互通缝二 2026-11-25）：subject=user 的结论是
        // 关于用户的习惯发现——不进 InsightStore（洞察是助手资产），按
        // 置信度/词表分流回抛画像（接口二激活）。
        if fi.subject.trim().eq_ignore_ascii_case("user") {
            route_user_finding(profile_ws, &fi, &mut outcome);
            continue;
        }
        match insights.add_candidate_checked(
            &fi.statement,
            &fi.category,
            &fi.evidence,
            InsightSource::Dialogue,
            fi.confidence,
            scope,
        ) {
            Ok((entry, activated)) => {
                if activated {
                    // 复现达标自动激活：进注入面的成员变化，outcome 三面
                    // （outcome.json/WSAPI/前端）透传（BUG-4）。
                    outcome.activated.push(entry.id.clone());
                }
                outcome.insights.push(LearnedInsightOut {
                    id: entry.id.clone(),
                    statement: entry.statement.clone(),
                });
                // supersede 退役（仅 active 目标——candidate 目标无需退役，
                // 两者共存不矛盾）。
                let target = kept
                    .iter()
                    .find(|k| k.id == fi.id)
                    .and_then(|k| k.supersede_target.clone());
                if let Some(target) = target.filter(|t| t.starts_with("ins-")) {
                    match retire_if_active(insights, &target) {
                        Ok(true) => outcome.retired.push(target),
                        Ok(false) => {}
                        Err(e) => outcome
                            .dropped
                            .push(format!("supersede 目标 {target} 退役失败：{e}")),
                    }
                }
            }
            Err(e) => outcome
                .dropped
                .push(format!("insight {}：落库被拒（{e}）", fi.id)),
        }
    }

    // skill 草稿（批次三不落 workspace——批次四 A 线 tests 闸负责）。
    for fs in p4.skills {
        if !kept_ids.contains(fs.id.as_str()) {
            outcome
                .dropped
                .push(format!("skill {}：不在对账保留清单，丢弃", fs.id));
            continue;
        }
        if !form_of(&fs.id).map(Form::wants_skill).unwrap_or(false) {
            outcome
                .dropped
                .push(format!("skill {}：定形未选 skill 形态，丢弃", fs.id));
            continue;
        }
        // 主语防御闸（缝二）：prompt 已约束 skill subject 恒 assistant；
        // 模型仍产出 user 主语技能时诚实丢弃（用户习惯不写成 agent 技能
        // ——装进技能库会反过来指导 agent 行为，方向性错误，不猜）。
        if fs.subject.trim().eq_ignore_ascii_case("user") {
            outcome.dropped.push(format!(
                "skill {}：主语为 user（用户习惯不产出 agent 技能），丢弃",
                fs.id
            ));
            continue;
        }
        let slug = fs.slug.trim();
        if slug.is_empty() || fs.skill_md.trim().is_empty() {
            outcome
                .dropped
                .push(format!("skill {}：slug/skill_md 为空，丢弃", fs.id));
            continue;
        }
        let has_tests = match crate::skill_tests::parse_skill_tests(&fs.skill_md) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                outcome
                    .dropped
                    .push(format!("skill {slug}：tests 段非法（批次四闸将拒绝）：{e}"));
                false
            }
        };
        outcome.skills.push(LearnedSkillDraft {
            slug: slug.to_string(),
            skill_md: fs.skill_md,
            has_tests,
        });
    }

    outcome.phases_completed = 4;
    persist(&transcript, &outcome);
    Ok(outcome)
}

/// supersede 目标退役守卫：只退役在场且 **active** 的条目（candidate 目
/// 标共存不矛盾；已 retired 幂等拒绝由 store 把守）。
fn retire_if_active(insights: &InsightStore, target: &str) -> Result<bool, String> {
    let all = insights.list(None)?;
    let is_active = all
        .iter()
        .any(|e| e.id == target && e.status == InsightStatus::Active);
    if !is_active {
        return Ok(false);
    }
    insights.retire(target, RetireReason::Superseded)?;
    Ok(true)
}

/// 用户侧结论分流（缝二路由，2026-11-25）：过闸（置信度 ≥ 画像反思同款
/// 阈值 [`userprofile::store::CONFIDENCE_THRESHOLD`] 且类别在画像白名单
/// [`userprofile::reflection::REFLECT_CATEGORIES`] 内）→
/// `store::ingest_external_entry` 直入画像（source=`learn_dialogue`，
/// auto=false 反思配额桶，合并语义/安全边界过滤与自产同源）；低置信/
/// 非法词表 → 观察提名队列人工裁决（宁滥勿缺不猜——与画像反思低置信
/// 带同款纪律）；入画像被拒（画像损坏/写失败）→ 同样转观察提名（结论
/// 不因单点写失败而丢失）。`profile_ws=None`（headless/无工作区形态）=
/// 诚实丢弃不回抛，不留半态。
fn route_user_finding(
    profile_ws: Option<&Path>,
    fi: &FinalInsight,
    outcome: &mut LearnDialogueOutcome,
) {
    let statement = fi.statement.trim().to_string();
    if statement.is_empty() {
        outcome
            .dropped
            .push(format!("insight {}：主语 user 且陈述为空，丢弃", fi.id));
        return;
    }
    let Some(ws) = profile_ws else {
        outcome.dropped.push(format!(
            "insight {}：主语 user 但无画像工作区（headless），不回抛",
            fi.id
        ));
        return;
    };
    let category = fi.category.trim().to_lowercase();
    let in_vocab = REFLECT_CATEGORIES.contains(&category.as_str());
    // 安全词表闸（BUG-20261125-052）：违规陈述（含禁词，如「跳过审批」）
    // 即使高置信+合法类别也不能走 ingest——upsert_habit 的静默闸会丢弃条
    // 目但 ingest_external_entry 返回 Ok，outcome 记 disposition="ingested"
    // 而条目三方失配（无画像条目、无观察提名、回执造假）。违规 = 走观察
    // 提名人工裁决（与文档「非法词表 → 观察提名」同语义）。
    let violates = violates_security_boundary(&statement);
    let now = chrono::Local::now().to_rfc3339();
    if fi.confidence >= CONFIDENCE_THRESHOLD && in_vocab && !violates {
        let entry = HabitEntry {
            category: category.clone(),
            content: statement.clone(),
            confidence: fi.confidence,
            hits: 0,
            first_seen: now.clone(),
            last_seen: now,
            status: crate::userprofile::store::STATUS_ACTIVE.into(),
            pinned: false,
            steps: Vec::new(),
            scope: String::new(),
            evidence: if fi.evidence.trim().is_empty() {
                Vec::new()
            } else {
                vec![fi.evidence.trim().to_string()]
            },
            auto: false,
            kind: crate::userprofile::store::KIND_HABIT.into(),
            source: String::new(), // ingest 强制 source 参数（权威）
            rule_id: String::new(),
        };
        // 对话教训（学习自对话会话）→ 对话画像（双 scope 拆分 2026-11-25）。
        match crate::userprofile::store::ingest_external_entry(
            ws,
            crate::userprofile::store::ProfileScope::Dialogue,
            entry,
            LEARN_DIALOGUE_SOURCE,
        ) {
            Ok(()) => {
                tracing::info!(
                    proposal = %fi.id,
                    category = %category,
                    confidence = fi.confidence,
                    "[Forge] learn: user finding ingested into profile"
                );
                outcome.user_findings.push(UserFindingOut {
                    disposition: "ingested".into(),
                    id: fi.id.clone(),
                    statement,
                    category,
                    confidence: fi.confidence,
                });
            }
            Err(e) => {
                outcome.dropped.push(format!(
                    "insight {}：用户结论入画像被拒（{e}），转观察提名",
                    fi.id
                ));
                push_user_observation(ws, fi, &statement, &category, outcome);
            }
        }
    } else {
        push_user_observation(ws, fi, &statement, &category, outcome);
    }
}

/// 用户侧结论落观察提名队列（人工裁决带；id/kind/status 由 bridge 发号
/// 强制）。队列满（cap）= 未收录诚实记 dropped——不覆盖既有 pending。
fn push_user_observation(
    ws: &Path,
    fi: &FinalInsight,
    statement: &str,
    category: &str,
    outcome: &mut LearnDialogueOutcome,
) {
    let obs = crate::userprofile::bridge::ObservationEntry {
        id: String::new(),
        ts_ms: chrono::Utc::now().timestamp_millis(),
        kind: String::new(),
        category: category.to_string(),
        content: statement.to_string(),
        confidence: fi.confidence,
        steps: Vec::new(),
        status: String::new(),
        resolved_ts_ms: None,
    };
    let admitted = crate::userprofile::bridge::append_observations(ws, &[obs]);
    if admitted > 0 {
        tracing::info!(
            proposal = %fi.id,
            category = %category,
            confidence = fi.confidence,
            "[Forge] learn: user finding parked as observation (manual adjudication)"
        );
        outcome.user_findings.push(UserFindingOut {
            disposition: "observation".into(),
            id: fi.id.clone(),
            statement: statement.to_string(),
            category: category.to_string(),
            confidence: fi.confidence,
        });
    } else {
        outcome.dropped.push(format!(
            "insight {}：观察提名队列已满（cap），本轮未收录",
            fi.id
        ));
    }
}

/// 模型裁决备注（note）追加为丢弃理由的语义尾巴（空 = 无尾巴）。
fn note_suffix(note: &str) -> String {
    let n = note.trim();
    if n.is_empty() {
        String::new()
    } else {
        format!("：{n}")
    }
}

/// 阶段 4 保留清单渲染（给 LLM 的最终指令原料）。
fn render_kept_items(proposals: &[Proposal], formed: &[(String, Form)], kept: &[Kept]) -> String {
    let mut out = String::new();
    for k in kept {
        let Some(p) = proposals.iter().find(|p| p.id == k.id) else {
            continue;
        };
        let form = formed
            .iter()
            .find(|(id, _)| id == &k.id)
            .map(|(_, f)| f.label())
            .unwrap_or("?");
        out.push_str(&format!(
            "- {} [{}] {}\n  观察：{}\n  教训：{}{}\n",
            p.id,
            form,
            p.title,
            p.observation,
            p.lesson,
            k.supersede_target
                .as_deref()
                .map(|t| format!("\n  取代目标：{t}（新内容将取代旧条目）"))
                .unwrap_or_default()
        ));
    }
    out
}

/// 留痕落盘：`transcript.md` + `outcome.json`（尽力而为，失败 WARN 不炸
/// 对话——留痕是回查面不是正确性依赖）。
fn persist_trace(trace_dir: &Path, transcript: &str, outcome: &LearnDialogueOutcome) {
    if let Err(e) = std::fs::create_dir_all(trace_dir) {
        tracing::warn!(
            dir = %trace_dir.display(), error = %e,
            "[learn] 留痕目录创建失败，本轮无留痕"
        );
        return;
    }
    if let Err(e) = std::fs::write(trace_dir.join("transcript.md"), transcript) {
        tracing::warn!(error = %e, "[learn] transcript.md 写入失败");
    }
    match serde_json::to_string_pretty(outcome) {
        Ok(json) => {
            if let Err(e) = std::fs::write(trace_dir.join("outcome.json"), json) {
                tracing::warn!(error = %e, "[learn] outcome.json 写入失败");
            }
        }
        Err(e) => tracing::warn!(error = %e, "[learn] outcome.json 序列化失败"),
    }
}

#[cfg(test)]
mod tests;
