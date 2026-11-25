//! 纠正候选采集 + 证据包组装——自学习铺路批次二「上匝道」（2026-11-24）。
//!
//! 「LLM 当脑子、Rust 当路」的上匝道：从会话日志确定性抓「agent 停下问
//! → 用户答复 → turn 收敛」三段形状的纠正候选（宁滥勿缺——语义真假归
//! 批次三对话期 LLM 判），落 append-only 候选台账，并组装预算封顶的证据
//! 包供学习对话消费。本模块只做**确定性**部分：形状匹配 / 去重 / 游标
//! 记账 / 预算截断；学什么、判什么归对话引擎（批次三）。
//!
//! 真相源：`docs/PLAN/2026-11-24_自学习铺路计划.md` §4.2 / §3。
//!
//! 确定性触发形状（v1 范围，用户批复）：**question 工具调用且用户已作答**。
//! 数据来源与形态锚点（动工前逐行复核）：
//! - chat_log assistant 行 `"work"` 时间线（`work_timeline::WorkTimeline`
//!   serde 形态）——question 调用记录（args_preview / result_preview）随
//!   **turn 终局**落盘，故「问→答→turn 收敛」三段天然压进同一条行内；
//!   turn 中途崩溃/estop 打断的问询不落 work = 天然排除（无假候选）。
//! - 答复判定按 result 前缀（`interactive.rs` 单一真相源）：
//!   `User selected:` / `User selected N option(s):` / `User answered form:`
//!   = 已作答；`No answer:`（超时/estop）= 用户没回，不是候选——v1 只抓
//!   卡片作答形态（§11.3 审批卡形态同样不抓）。
//! - 会话身份 = `logs/session_logs/*.jsonl` 文件名消毒键（chat_log 寻址
//!   唯一可恢复身份；`sanitize_path_segment` 有损不可逆，原始键不落盘
//!   也不需要——目标摘要已给对话引擎语义锚点）。
//! - 任务目标摘要 = 会话首条**真实** user 行（跳过 system_notice/steer
//!   注入行——它们 role=user 但不是用户说的话）。
//!
//! 消费模型：`candidates.jsonl` append-only 不删行；消费按**按消费者游
//! 标**记账（`consumed.json`）——同一候选行可被画像 sweep 与学习对话各
//! 自消费（一次捕获、两次提取：画像=关于用户的模式，台账=关于 agent 的
//! 准则），互不遮挡。
//!
//! 测试：[`tests`]（独立文件，仓库内联测试禁令合规）。

use crate::insights::InsightEntry;
use crate::insights::fingerprint;
use crate::userprofile::bridge::LessonEntry;
use nemesis_types::forge::Experience;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// ask 摘要上限（字符）。
pub const ASK_MAX_CHARS: usize = 200;
/// reply 摘要上限（字符）。
pub const REPLY_MAX_CHARS: usize = 200;
/// 任务目标摘要上限（字符）。
pub const GOAL_MAX_CHARS: usize = 200;
/// 收敛结果摘要上限（字符）。
pub const RESULT_MAX_CHARS: usize = 300;
/// 证据包总预算（字符）。超限**截证据包不截对话轮次**（§3 预算纪律）。
pub const EVIDENCE_PACK_MAX_CHARS: usize = 6000;
/// 证据包流程级材料：最近失败经验 digest 条数上限。
pub const EXPERIENCE_DIGEST_MAX_ITEMS: usize = 12;
/// 证据包流程级材料：最新反思报告摘录上限（字符，头截断）。
pub const REPORT_EXCERPT_MAX_CHARS: usize = 1200;
/// 证据包候选节条数上限（BUG-1 修复：候选无上限会吃穿总预算并连带
/// 丢弃后续节；超出部分**不消费**，留待下一轮）。cap 修剪不是截断——
/// 不置 `truncated`。
pub const CANDIDATE_PACK_MAX: usize = 12;
/// 证据包现有 active 洞察节条数上限（对账原料按需取头部即可，同族
/// 节帽纪律对齐：④模式 6 条 / ⑤失败 12 条）。
pub const ACTIVE_PACK_MAX: usize = 8;
/// 证据包用户会话教训节条数上限（画像↔自学习双缝互通 2026-11-25：pending
/// lessons 并入证据包独立节；超出部分**不 ack**、留待下一轮，与候选节
/// 超量不消费同纪律）。
pub const LESSON_PACK_MAX: usize = 8;
/// 教训条目 content 进包截断上限（字符；反思归纳陈述应为短句，防御性
/// 截断防超长条目吃穿预算）。
pub const LESSON_CONTENT_MAX_CHARS: usize = 200;
/// 教训条目 evidence 串接进包截断上限（字符）。
pub const LESSON_EVIDENCE_MAX_CHARS: usize = 120;

/// 消费者键：学习对话（批次三接线）。
pub const CONSUMER_DIALOGUE: &str = "dialogue";
/// 消费者键：B 线画像 sweep（共享读取，一次捕获两次提取）。
pub const CONSUMER_PROFILE: &str = "profile";

/// 答复前缀（`loop_tools/interactive.rs` 返回文本的单一真相源镜像；
/// 形态变更时本表与 interactive 测试同步改——drift 由测试钉住）。
const ANSWERED_PREFIXES: [&str; 3] = ["User selected:", "User selected ", "User answered form:"];

/// 扫描产出的候选草稿（未落账前的内存形态；`id` 由 ingest 发号）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateDraft {
    /// 会话日志文件名消毒键（见模块注释「会话身份」）。
    pub session_key: String,
    /// 触发行的 timestamp（turn 收敛时刻，RFC3339 原样）。
    pub ts: String,
    /// question 参数摘要（JSON 可解则取 `question` 字段，否则原始 preview）。
    pub ask: String,
    /// 用户答复摘要（`User selected …` / `User answered form …` 原样截断）。
    pub reply: String,
    /// 任务目标摘要（会话首条真实 user 行）。
    pub goal: String,
    /// 收敛结果摘要（该 assistant 行 content 尾段——turn 最终回复）。
    pub result: String,
    /// 去重指纹：sha256(session|ask|reply) 前 16 hex（重复扫描不重复落账）。
    pub fp: String,
    /// 场景戳（洞察页与场景拆分 2026-11-24）：会话所属项目 id（sidecar
    /// `.meta.json` `project_id`）；None = 对话组会话（无 sidecar/无
    /// project_id/损坏 = 对话，诚实缺省）。serde default 旧台账行零迁移。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// 候选台账行（`<forge>/evidence/candidates.jsonl` 单行 serde 形态）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrectionCandidate {
    /// `cand-<yyyymmdd>-<seq>`（序号按同日最大值 +1，全量扫描）。
    pub id: String,
    #[serde(flatten)]
    pub draft: CandidateDraft,
}

/// 候选台账 + 消费游标账的存储视图。实例只是路径视图（与
/// [`crate::insights::InsightStore`] 同形）——按调用现构造合法，RMW 由
/// 模块级写锁与 ingest/mark_consumed 串行。
pub struct CandidateStore {
    dir: PathBuf,
}

/// 模块级全局 RMW 写锁：跨 store 实例串行化 ingest（读 fp 集→追加）与
/// mark_consumed（读游标→推进）全程。与 `insights::WRITE_LOCK` 同形。
static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

impl CandidateStore {
    pub fn from_forge_dir(forge_dir: &Path) -> Self {
        Self {
            dir: forge_dir.join("evidence"),
        }
    }

    /// candidates.jsonl 路径（WSAPI/CLI 展示用）。
    pub fn candidates_path(&self) -> PathBuf {
        self.dir.join("candidates.jsonl")
    }

    fn consumed_path(&self) -> PathBuf {
        self.dir.join("consumed.json")
    }

    /// 逐行读台账；末行半截（append 中断撕裂）跳过并 WARN——append-only
    /// 日志的宽容读纪律。行级 serde 失败同样跳过不炸。
    pub fn read_all(&self) -> Vec<CorrectionCandidate> {
        let path = self.candidates_path();
        let Ok(file) = std::fs::File::open(&path) else {
            return Vec::new(); // 文件缺席 = 空台账（合法初态）
        };
        let mut out = Vec::new();
        for (idx, line) in BufReader::new(file).lines().enumerate() {
            let Ok(line) = line else {
                tracing::warn!(path = %path.display(), line = idx, "[evidence] 行读取失败，跳过");
                continue;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<CorrectionCandidate>(trimmed) {
                Ok(c) => out.push(c),
                Err(e) => {
                    tracing::warn!(path = %path.display(), line = idx, error = %e,
                        "[evidence] 行解析失败（撕裂/损坏），跳过");
                }
            }
        }
        out
    }

    /// 落账：按 `fp` 去重（重复扫描同会话不重复追加），新行发号追加。
    /// 返回本次新追加条数。调用方（cycle 扫描）重复喂全量草稿是预期用法
    /// ——幂等。
    pub fn ingest(&self, drafts: Vec<CandidateDraft>) -> Result<usize, String> {
        if drafts.is_empty() {
            return Ok(0);
        }
        let _serial = WRITE_LOCK.lock();
        let existing = self.read_all();
        let seen: std::collections::HashSet<&str> =
            existing.iter().map(|c| c.draft.fp.as_str()).collect();
        let mut added = 0usize;
        let mut next_id_state = NextIdState::from_existing(&existing);
        let mut lines = String::new();
        for d in drafts {
            if seen.contains(d.fp.as_str()) {
                continue;
            }
            let id = next_id_state.take();
            let row = CorrectionCandidate { id, draft: d };
            lines.push_str(&serde_json::to_string(&row).map_err(|e| format!("serialize: {e}"))?);
            lines.push('\n');
            added += 1;
        }
        if added == 0 {
            return Ok(0);
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("create evidence dir: {e}"))?;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.candidates_path())
            .map_err(|e| format!("open candidates.jsonl: {e}"))?;
        f.write_all(lines.as_bytes())
            .map_err(|e| format!("append candidates.jsonl: {e}"))?;
        Ok(added)
    }

    /// 消费游标账（`{"cursors": {<consumer>: 已消费行数}}`）。损坏 =
    /// WARN + 归零（重投递宁滥勿缺，不丢原始行）。
    fn load_cursors(&self) -> std::collections::HashMap<String, u64> {
        let raw = match std::fs::read(self.consumed_path()) {
            Ok(b) => b,
            Err(_) => return std::collections::HashMap::new(),
        };
        let bytes: &[u8] = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &raw[3..]
        } else {
            &raw[..]
        };
        match serde_json::from_slice::<ConsumedLedger>(bytes) {
            Ok(l) => l.cursors,
            Err(e) => {
                tracing::warn!(path = %self.consumed_path().display(), error = %e,
                    "[evidence] 游标账损坏，归零重投递（宁滥勿缺）");
                std::collections::HashMap::new()
            }
        }
    }

    fn save_cursors(&self, cursors: &std::collections::HashMap<String, u64>) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("create evidence dir: {e}"))?;
        let json = serde_json::to_string_pretty(&ConsumedLedger {
            cursors: cursors.clone(),
        })
        .map_err(|e| format!("serialize cursors: {e}"))?;
        nemesis_utils::write_file_atomic(
            &self.consumed_path().to_string_lossy(),
            json.as_bytes(),
            0o600,
        )
        .map_err(|e| format!("write consumed.json: {e}"))
    }

    /// 指定消费者未消费的候选行（游标之后的全量）。
    pub fn unconsumed(&self, consumer: &str) -> Vec<CorrectionCandidate> {
        let all = self.read_all();
        let offset = self.load_cursors().get(consumer).copied().unwrap_or(0) as usize;
        all.into_iter().skip(offset).collect()
    }

    /// 消费游标读数（实效统计消费面，自学习铺路批次四；无记录 = 0）。
    pub fn consumed_count(&self, consumer: &str) -> u64 {
        self.load_cursors().get(consumer).copied().unwrap_or(0)
    }

    /// 推进消费者游标（按行数；超总行数钳到总行数）。
    pub fn mark_consumed(&self, consumer: &str, count: usize) -> Result<(), String> {
        if count == 0 {
            return Ok(());
        }
        let _serial = WRITE_LOCK.lock();
        let total = self.read_all().len();
        let mut cursors = self.load_cursors();
        let cur = cursors.get(consumer).copied().unwrap_or(0) as usize;
        let next = (cur + count).min(total);
        cursors.insert(consumer.to_string(), next as u64);
        self.save_cursors(&cursors)
    }
}

#[derive(Serialize, Deserialize)]
struct ConsumedLedger {
    cursors: std::collections::HashMap<String, u64>,
}

/// `cand-<yyyymmdd>-<seq>` 发号器（同日最大序号 +1；与 insights::next_id
/// 同形，独立前缀互不串号）。
struct NextIdState {
    prefix: String,
    next: u32,
}

impl NextIdState {
    fn from_existing(existing: &[CorrectionCandidate]) -> Self {
        let prefix = format!("cand-{}-", chrono::Local::now().format("%Y%m%d"));
        let mut max_seq = 0u32;
        for c in existing {
            if let Some(seq) =
                c.id.strip_prefix(&prefix)
                    .and_then(|s| s.parse::<u32>().ok())
            {
                max_seq = max_seq.max(seq);
            }
        }
        Self {
            prefix,
            next: max_seq + 1,
        }
    }

    fn take(&mut self) -> String {
        let id = format!("{}{:03}", self.prefix, self.next);
        self.next += 1;
        id
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 扫描器（确定性形状匹配）
// ─────────────────────────────────────────────────────────────────────────

/// 单个 session_logs jsonl → 候选草稿列表（含任务目标摘要提取）。
/// `scope`：会话场景戳（调用方从同名 sidecar 解析，见
/// [`scan_workspace_logs`]）；None = 对话组会话。
pub fn scan_chat_log(session_key: &str, scope: Option<&str>, bytes: &[u8]) -> Vec<CandidateDraft> {
    let text = String::from_utf8_lossy(bytes);
    let mut goal = String::new();
    let mut drafts = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue; // 坏行宽容跳过（chat_log 读侧纪律）
        };
        let role = row["role"].as_str().unwrap_or("");
        let content = row["content"].as_str().unwrap_or("");
        let ts = row["timestamp"].as_str().unwrap_or("");
        if role == "user" && goal.is_empty() {
            // 首条真实 user 行（system_notice / steer 注入行不是用户的话）
            let is_injected = row.get("system_notice").is_some()
                || row.get("steer").and_then(|v| v.as_bool()).unwrap_or(false);
            if !is_injected && !content.trim().is_empty() {
                goal = truncate_chars(content, GOAL_MAX_CHARS);
            }
        }
        if role != "assistant" {
            continue;
        }
        let Some(tools) = row
            .get("work")
            .and_then(|w| w.get("rounds"))
            .and_then(|r| r.as_array())
            .map(|rounds| {
                rounds
                    .iter()
                    .filter_map(|r| r.get("tools").and_then(|t| t.as_array()))
                    .flatten()
                    .collect::<Vec<&serde_json::Value>>()
            })
        else {
            continue;
        };
        for t in tools {
            if t["tool"].as_str() != Some("question") {
                continue;
            }
            let result_preview = t["result_preview"].as_str().unwrap_or("");
            let Some(reply) = answered_reply(result_preview) else {
                continue; // 超时/estop/空 = 用户没回，不是候选
            };
            let args_preview = t["args_preview"].as_str().unwrap_or("");
            drafts.push(CandidateDraft {
                session_key: session_key.to_string(),
                ts: ts.to_string(),
                ask: extract_ask(args_preview),
                reply: truncate_chars(reply, REPLY_MAX_CHARS),
                goal: goal.clone(),
                result: tail_chars(content, RESULT_MAX_CHARS),
                fp: candidate_fingerprint(session_key, args_preview, result_preview),
                scope: scope.map(|s| s.to_string()),
            });
        }
    }
    drafts
}

/// 扫描 session_logs 目录下全部 `*.jsonl`（显式排序——目录序是平台实现
/// 细节；meta sidecar `.meta.json` 与 boundary 无后缀文件天然被过滤）。
/// 每个会话文件从同名 `.meta.json` 旁车读 `project_id` 盖场景戳（缺席/
/// 损坏/键缺失 = 对话洞察，None——诚实缺省不猜测）。
pub fn scan_workspace_logs(logs_dir: &Path) -> Vec<CandidateDraft> {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    paths.sort(); // NTFS/ext4 目录序不可依赖，显式排序
    let mut out = Vec::new();
    for p in paths {
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let scope = sidecar_scope(&p);
        out.extend(scan_chat_log(stem, scope.as_deref(), &bytes));
    }
    out
}

/// 会话 sidecar 场景解析：同名 `.meta.json` 的 `project_id` 字段。
/// sidecar 缺席 / 非法 JSON / 键缺失 / 空串 = None（对话组会话）。
fn sidecar_scope(log_path: &Path) -> Option<String> {
    let stem = log_path.with_extension("");
    let sidecar = stem.with_extension("meta.json");
    let Ok(bytes) = std::fs::read(&sidecar) else {
        return None;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return None;
    };
    v["project_id"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
}

/// 答复判定：命中任一已答前缀 = Some(答复原文)；否则 None。
fn answered_reply(result_preview: &str) -> Option<&str> {
    let t = result_preview.trim_start();
    if t.is_empty() {
        return None;
    }
    ANSWERED_PREFIXES
        .iter()
        .find(|p| t.starts_with(*p))
        .map(|_| t)
}

/// ask 提取：args 是 JSON 且带 `question` 字段 → 取字段；否则原始
/// preview 截断（preview 可能是截断的半截 JSON——解析失败是常态不是异常）。
fn extract_ask(args_preview: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(args_preview).ok();
    if let Some(q) = parsed.as_ref().and_then(|v| v["question"].as_str()) {
        return truncate_chars(q, ASK_MAX_CHARS);
    }
    truncate_chars(args_preview, ASK_MAX_CHARS)
}

/// 去重指纹：sha256(session|ask|reply) 前 16 hex——同会话同问同答 = 同
/// 候选（重复扫描幂等）；同会话不同答复（改选）= 新候选（答复是语义的一半）。
fn candidate_fingerprint(session_key: &str, ask: &str, reply: &str) -> String {
    fingerprint(&format!("{session_key}|{ask}|{reply}"))
}

/// 头截断（字符级，中文安全）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// 尾截断（收敛结果取 turn 最终回复的结尾）。
fn tail_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        s.chars().skip(count - max).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 证据包组装器（预算封顶）
// ─────────────────────────────────────────────────────────────────────────

/// 证据包：组装产物 + 是否截断标记（§3「超限截证据包，不截对话轮次」）。
#[derive(Clone, Debug, PartialEq)]
pub struct EvidencePack {
    /// 组装后的包文本（给对话引擎当 user/上下文载荷；空候选也产出合法包
    /// ——「没有值得学的」是一等答案，对话引擎据此诚实收场）。
    pub text: String,
    /// 总预算截断标记。
    pub truncated: bool,
    /// 包内候选条数（对话引擎入参速查）。
    pub candidate_count: usize,
    /// 实际进包的用户会话教训 id（双缝互通 2026-11-25）——对话成功后按
    /// 此 ack；**凡确认消费的教训必须真实进过包**（BUG-1 同款纪律：
    /// lesson_ids ≡ lessons 节实际装箱条目，超容量/超预算未进包的不在
    /// 其中，留待下一轮）。
    pub lesson_ids: Vec<String>,
}

/// 组装证据包。节优先级：候选（新信号）→ 用户会话教训（画像反思分流的
/// 新信号，双缝互通 2026-11-25）→ 任务目标 → 现有 active 洞察 → 模式
/// 高亮（监控闭环检出的结构化模式）→ 最近失败经验 → 最新反思报告摘录
/// （报告改道：报告是文本侧观察的沉淀，对话引擎消费同一份观察）→ 技能
/// 库摘要（批次三从 agent 侧传入；None = 节省略）。候选节与教训节**逐条
/// 装箱**且共用一个预算窗（联合预留节头/尾注/节间符上界后按序装箱——
/// 两节各自独立预留会互相顶飞对方，甚至让后装节在总装配期被丢弃）：
/// 节帽 CANDIDATE_PACK_MAX / LESSON_PACK_MAX + 总预算双闸，超出容量的
/// 候选不消费、教训不 ack、留待下一轮（节帽致停 ≠ 截断；预算致停诚实
/// 置 `truncated`）；active 节按节帽修剪；其余各节按总预算截断，
/// `truncated` 诚实标记。`candidate_count` ≡ 实际进包候选条数——消费
/// 游标推进必须以此为准（凡标记消费的候选必须真实进过包），不得用请求
/// 侧 len()。`lesson_ids` ≡ 实际进包教训 ids——ack 必须以此为凭，同
/// 纪律（凡确认消费的教训必须真实进过包）。
#[allow(clippy::too_many_arguments)]
pub fn build_evidence_pack(
    goal: Option<&str>,
    candidates: &[CorrectionCandidate],
    lessons: &[LessonEntry],
    insights: Option<&[InsightEntry]>,
    patterns_digest: Option<&str>,
    experiences: &[Experience],
    latest_report: Option<&str>,
    skills_digest: Option<&str>,
) -> EvidencePack {
    let mut sections: Vec<String> = Vec::new();

    // ① 候选节 + ② 教训节（新信号，共用预算窗逐条装箱）。联合预留口径
    // 与总装箱一致：包头 "# 学习证据包\n" + 两节的节头/尾注（按总条数
    // 格式化 = 长度上界）+ 两节的节间 '\n'（总装配每节前推 1 字节）——
    // 上界预留保证两个装箱节**必然**通过总装配检查（candidate_count /
    // lesson_ids 恒 ≡ 最终文本里的实际条目，节不会被装配期丢弃）。
    let mut candidate_count = 0usize;
    let mut lesson_ids: Vec<String> = Vec::new();
    let mut new_signal_budget_stop = false;
    let base = "# 学习证据包\n".chars().count() + 1;
    let cand_reserve = usize::from(!candidates.is_empty())
        * (format!(
            "## 纠正候选（{} / {} 条）\n",
            candidates.len(),
            candidates.len()
        )
        .chars()
        .count()
            + format!(
                "\n（另有 {} 条候选超出本包容量，未列入——留待下一轮，本轮不标记消费）\n",
                candidates.len()
            )
            .chars()
            .count());
    let lesson_reserve = usize::from(!lessons.is_empty())
        * (format!(
            "## 用户会话教训（{} / {} 条）\n",
            lessons.len(),
            lessons.len()
        )
        .chars()
        .count()
            + format!(
                "\n（另有 {} 条教训超出本包容量，未列入——留待下一轮，本轮不确认消费）\n",
                lessons.len()
            )
            .chars()
            .count());
    let section_seps = 2; // 候选节 + 教训节各自的节间 '\n'
    let mut budget =
        EVIDENCE_PACK_MAX_CHARS.saturating_sub(base + cand_reserve + lesson_reserve + section_seps);

    // ① 候选节。超出的候选**不消费**、留待下一轮（BUG-1 根治：旧实现
    // 整节装箱失败即整体丢弃 + 游标按整批推进 = 未进包候选被静默标记
    // 消费，证据永久丢失；现 candidate_count 恒 ≡ 实际进包条数）。节帽
    // 致停 ≠ 截断；预算致停诚实置 truncated。
    let mut candidate_section = String::new();
    if candidates.is_empty() {
        candidate_section.push_str(
            "## 纠正候选\n\n（本批无候选——没有检测到「agent 停下问→用户答复→任务收敛」形状）",
        );
    } else {
        let mut body = String::new();
        let mut included = 0usize;
        for c in candidates.iter().take(CANDIDATE_PACK_MAX) {
            let entry = format!(
                "\n### {} [{}]\n- 会话任务目标：{}\n- agent 问：{}\n- 用户答：{}\n- 任务收敛结果：{}\n",
                c.id, c.draft.ts, c.draft.goal, c.draft.ask, c.draft.reply, c.draft.result
            );
            let n = entry.chars().count();
            if n > budget {
                new_signal_budget_stop = true;
                break;
            }
            budget -= n;
            included += 1;
            body.push_str(&entry);
        }
        candidate_count = included;
        candidate_section.push_str(&format!(
            "## 纠正候选（{} / {} 条）\n",
            included,
            candidates.len()
        ));
        candidate_section.push_str(&body);
        let excluded = candidates.len() - included;
        if excluded > 0 {
            candidate_section.push_str(&format!(
                "\n（另有 {} 条候选超出本包容量，未列入——留待下一轮，本轮不标记消费）\n",
                excluded
            ));
        }
    }
    sections.push(candidate_section);

    // ② 用户会话教训节（双缝互通：pending lessons 进包深学）。lessons
    // 为空 = 节整体省略（队列空是常态；候选节「无候选」是对话的一等
    // 信号，教训节不是）。逐条装箱共用同一预算窗：超容量/超预算的教训
    // 不 ack、留待下一轮（lesson_ids 只记实际装箱者）。
    if !lessons.is_empty() {
        let mut body = String::new();
        let mut included = 0usize;
        for l in lessons.iter().take(LESSON_PACK_MAX) {
            let mut entry = format!(
                "\n### {} [{}] 置信 {:.2}\n- 教训：{}\n",
                l.id,
                l.category,
                l.confidence,
                truncate_chars(&l.content, LESSON_CONTENT_MAX_CHARS)
            );
            if !l.evidence.is_empty() {
                entry.push_str(&format!(
                    "- 依据：{}\n",
                    truncate_chars(&l.evidence.join(", "), LESSON_EVIDENCE_MAX_CHARS)
                ));
            }
            let n = entry.chars().count();
            if n > budget {
                new_signal_budget_stop = true;
                break;
            }
            budget -= n;
            included += 1;
            lesson_ids.push(l.id.clone());
            body.push_str(&entry);
        }
        let mut lesson_section =
            format!("## 用户会话教训（{} / {} 条）\n", included, lessons.len());
        lesson_section.push_str(&body);
        let excluded = lessons.len() - included;
        if excluded > 0 {
            lesson_section.push_str(&format!(
                "\n（另有 {} 条教训超出本包容量，未列入——留待下一轮，本轮不确认消费）\n",
                excluded
            ));
        }
        sections.push(lesson_section);
    }

    // ③ 任务目标节（显式传入时——单会话手动触发形态）
    if let Some(g) = goal.filter(|g| !g.trim().is_empty()) {
        sections.push(format!("## 当前任务目标\n\n{g}"));
    }

    // ④ 现有 active 洞察（对账原料——新候选是否已被覆盖）。封顶
    // ACTIVE_PACK_MAX 条（对齐节帽纪律；BUG-2：active 增长无上限会
    // 挤占后续节预算）。
    if let Some(list) = insights {
        let actives: Vec<&InsightEntry> = list
            .iter()
            .filter(|e| e.status == crate::insights::InsightStatus::Active)
            .collect();
        if !actives.is_empty() {
            let shown = actives.len().min(ACTIVE_PACK_MAX);
            let mut s = format!("## 现有 active 洞察（{} / {} 条）\n", shown, actives.len());
            for e in actives.iter().take(shown) {
                s.push_str(&format!("- [{}] {}\n", e.category, e.statement));
            }
            if actives.len() > shown {
                s.push_str(&format!(
                    "\n（另有 {} 条 active 未列入本包）\n",
                    actives.len() - shown
                ));
            }
            sections.push(s);
        }
    }

    // ⑤ 模式高亮（监控闭环检出的结构化模式——digest 文本直接进包；
    // None/空 = 节省略。组装方（forge.rs）从 learning_engine
    // extract_patterns 产出，证据块已脱敏）。
    if let Some(pd) = patterns_digest.filter(|pd| !pd.trim().is_empty()) {
        sections.push(format!("## 监控闭环模式高亮\n\n{pd}"));
    }

    // ⑥ 最近失败经验 digest（流程级材料；只取失败、尾部 N 条）
    let failures: Vec<&Experience> = experiences.iter().filter(|e| !e.success).collect();
    let start = failures.len().saturating_sub(EXPERIENCE_DIGEST_MAX_ITEMS);
    if failures.len() > start {
        let mut s = format!(
            "## 最近失败经验（{} / {} 条失败）\n",
            failures.len() - start,
            failures.len()
        );
        for e in &failures[start..] {
            s.push_str(&format!(
                "- [{}] {} → {}\n",
                truncate_chars(&e.tool_name, 24),
                truncate_chars(&e.input_summary, 80),
                truncate_chars(&e.output_summary, 80)
            ));
        }
        sections.push(s);
    }

    // ⑦ 最新反思报告摘录（报告改道：报告是文本侧观察的沉淀，节头
    // 截断进包；None/空 = 节省略）。
    if let Some(lr) = latest_report.filter(|lr| !lr.trim().is_empty()) {
        sections.push(format!(
            "## 最新反思报告摘录\n\n{}",
            truncate_chars(lr.trim(), REPORT_EXCERPT_MAX_CHARS)
        ));
    }

    // ⑧ 技能库摘要（对账原料——是否值得成 skill）
    if let Some(sd) = skills_digest.filter(|sd| !sd.trim().is_empty()) {
        sections.push(format!("## 现有技能库\n\n{sd}"));
    }

    // 预算封顶：按序装箱，超限截断 + 诚实标记（候选节/教训节已逐条装箱
    // 必然装得下——联合预留含节间符；预算致停同样计入诚实标记）。
    let mut text = String::from("# 学习证据包\n");
    let mut truncated = new_signal_budget_stop;
    for s in sections {
        if text.chars().count() + s.chars().count() + 1 > EVIDENCE_PACK_MAX_CHARS {
            truncated = true;
            break;
        }
        text.push('\n');
        text.push_str(&s);
    }
    EvidencePack {
        text,
        truncated,
        candidate_count,
        lesson_ids,
    }
}

#[cfg(test)]
mod tests;
