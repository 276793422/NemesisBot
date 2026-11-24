//! 确定性挖掘层（Forge 化重构批一/批三，G5/G10/例1/例2；扩展总计划批 B
//! 追加零成本挖掘器群）。
//!
//! 全部纯函数（数据进、候选出，零 LLM 成本，可全量单测断言）。通道清单：
//! - [`stat_candidates`]：v1 三条统计规则**原样迁入**（行为不变——四天实战
//!   学到的 2 条就是它们出的；auto=true 进独立配额桶，G8）+ 批 B 增量
//!   （提问粒度长文派活 / 中英混用术语保留）；
//! - [`ritual_candidates`]：会话内**仪式序列**状态机（例 1：计划→多轮回顾
//!   确认→步骤清单→测试验收。旧系统对它最多靠 60×120 采样碰运气截到残片，
//!   「多步序列」在扁平采样下结构性不可见——G3/G5 根修）；
//! - [`todo_candidates`]：todo 清单收尾模式挖掘（例 1 第 3 步的机器可读
//!   面——「完成一个标记一个、测试收尾」直接读 todo 文件验证，无 LLM）；
//! - [`antecedent_candidates`]：WSAPI 遥测**时序前因**挖掘（例 2：诊断
//!   页先行 → N 分钟内集群/看板操作，重复 ≥K 天 → 「用前先诊断」条目）。
//!
//! 批 B 追加（扩展总计划 §四，同构纪律：一句话 content + steps 可选 +
//! evidence 溯源 + 保守阈值宁缺勿滥）：
//! - [`route_chain_candidates`]：页面足迹链（`profile.route` 行按日挖导航
//!   仪式——antecedent 的姊妹通道，挖「纯导航序列」）；
//! - [`todo_completion_stats`]：todo 完成/中弃统计（**统计面不注入**，批 D
//!   Dashboard 消费）；
//! - [`todo_sequence_candidates`]：步骤类别序列（部署类步骤常收尾）；
//! - [`opener_candidates`]：开场白模式（每会话首条 user 消息形态）；
//! - [`urge_candidates`]：催促/复述模式（短窗同文重发）；
//! - [`delegation_candidates`]：分工风格（微观管理 vs 派单，对话时间结构）；
//! - [`rework_candidates`]：返工节奏（rewind 用户读法：迭代型 vs 一次成型）；
//! - [`retry_candidates`]：障碍处置顺序（失败重试用户读法）；
//! - [`approval_signal_candidates`]：审批交互弱信号（姿态层，权限语义不动）。
//!
//! 批 C 追加（采集增强的挖掘面，同构纪律不变）：
//! - [`work_domain_candidates`]：工作域画像（assistant 行文件接触种类
//!   占比——file_changes/deliverables 扩展名分类，批 C ①）；
//! - [`model_switch_candidates`]：模型切换行为（chat_log model 徽标变化
//!   + /model 指令，批 C ③ 替代形态——替代原计划 request_logs 接入）；
//! - [`approval_strong_candidates`]：审批交互**强**信号（signals.jsonl
//!   审批卡决策耗时，批 C ④——与弱信号通道互补：弱信号看遥测响应成败，
//!   强信号看真实卡片的决策墙钟）。
//!
//! 2026-11-24 T 批追加（终端纳入）：[`shell_preference_candidates`]（PTY
//! 审计 `# shell:` 头主导 shell 偏好）+ [`terminal_command_candidates`]
//! （首 token 命令族，**参数不采集**——敏感面零暴露）。输入 = 采集层
//! （[`super::collection::collect_terminal_sessions`]）凭据防线清洗后的
//! 命令行投影，挖掘器不再见原始审计。

use super::collection::{DialogueRow, TodoSnapshot};
use super::store::HabitEntry;

/// 统计通道候选（v1 三条规则，行为与 nemesis-agent/user_profile.rs 完全
/// 一致；条目改为 [`HabitEntry::auto_entry`] 构造——auto=true 固定产物，
/// 进独立配额桶不再与反思条目挤名额，G8）。
///
/// 输入 = 证据窗内的用户消息列表（仅 enabled_at 之后的数据由调用方过滤）。
/// 规则：
/// - 语言：消息 ≥10 条且中文字符占比 >0.7 → 中文交流习惯；
/// - 简洁：平均用户长度 <40 字 → 简洁交互习惯；
/// - 纠正：纠正前缀（不对/不，/我意思是/别/不要）频次 ≥3 →
///   「先确认理解再动手」习惯。
pub fn stat_candidates(user_messages: &[String], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let n = user_messages.len();
    if n == 0 {
        return out;
    }

    // 语言。
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
            out.push(HabitEntry::auto_entry(
                "style",
                "用户主要使用中文交流，回复使用中文",
                (0.5 + ratio * 0.4).min(0.95),
                n as u32,
                now,
            ));
        }
    }

    // 简洁。
    let avg_len = total_chars as f64 / n as f64;
    if n >= 10 && avg_len < 40.0 {
        out.push(HabitEntry::auto_entry(
            "style",
            "用户消息简短直接，回复宜简洁、结论先行",
            0.7,
            n as u32,
            now,
        ));
    }

    // 纠正前缀。
    const PREFIXES: &[&str] = &["不对", "不，", "不是的", "我意思是", "别", "不要"];
    let corrections = user_messages
        .iter()
        .filter(|m| {
            let t = m.trim_start();
            PREFIXES.iter().any(|p| t.starts_with(p))
        })
        .count();
    if corrections >= 3 {
        out.push(HabitEntry::auto_entry(
            "correction",
            "用户常用纠正性反馈——动手前优先复述理解、小步确认方向",
            (0.5 + corrections as f64 * 0.1).min(0.9),
            corrections as u32,
            now,
        ));
    }

    // 提问粒度（批 B ⑤）：长文详述需求——与「简洁」规则互补（avg<40 简洁 /
    // avg≥200 长需求书，中间带不上榜）。「等完整输入不抢跑」是 agent 侧配合
    // 姿态，不是 agent 错误归纳（主语=用户）。
    const LONGFORM_AVG_CHARS: f64 = 200.0;
    if n >= 10 && avg_len >= LONGFORM_AVG_CHARS {
        out.push(HabitEntry::auto_entry(
            "style",
            "用户习惯长文详述需求——等完整输入再动手，不要按开头片段抢跑猜测",
            0.72,
            n as u32,
            now,
        ));
    }

    // 语言形态（批 B ⑥）：中英混用——同时含 CJK 与 ASCII 字母词的消息占比
    // ≥0.5 且样本 ≥10 → 术语保留英文原词（不必强行翻译）。
    const MIXED_TERM_MIN_MSGS: usize = 10;
    const MIXED_TERM_RATIO: f64 = 0.5;
    let is_cjk = |c: char| {
        let c = c as u32;
        (0x4E00..=0x9FFF).contains(&c)
    };
    let mixed = user_messages
        .iter()
        .filter(|m| m.chars().any(is_cjk) && m.chars().any(|c| c.is_ascii_alphabetic()))
        .count();
    if n >= MIXED_TERM_MIN_MSGS {
        let ratio = mixed as f64 / n as f64;
        if ratio >= MIXED_TERM_RATIO {
            out.push(HabitEntry::auto_entry(
                "style",
                "用户中英混用交流，技术术语保留英文原词，不必强行翻译",
                (0.5 + ratio * 0.4).min(0.9),
                mixed as u32,
                now,
            ));
        }
    }

    out
}

// ---------- 仪式序列挖掘（例 1，G3/G5） ----------

/// 阶段关键词表（`.contains` 语义，大小写不敏感由调用方转小写）。
const KW_PLAN: &[&str] = &["计划", "plan"];
const KW_CONFIRM: &[&str] = &["回顾", "确认", "复核", "check", "review"];
const KW_CHECKBOX: &[&str] = &["复选框", "步骤", "清单", "流程", "checklist"];
const KW_TEST: &[&str] = &["测试", "验收", "test"];
/// confirm 阶段要求出现次数（用户例：「好几遍回顾确认」）。
const CONFIRM_MIN_HITS: usize = 2;
/// 仪式成立的会话数下限。
const RITUAL_MIN_SESSIONS: usize = 2;

fn contains_any(lower: &str, kws: &[&str]) -> bool {
    kws.iter().any(|k| lower.contains(k))
}

/// 单会话仪式状态机：用户消息按序扫描，阶段单调推进
/// `plan → confirm(×2) → checkbox → test`；四阶段全走完 = 该会话命中
/// 完整仪式。中途出现的非当前阶段关键词不推进也不回退（含关键词的闲聊
/// 不破坏链路；乱序使用不构成仪式——比如先说测试后说计划的状态机不会
/// 因 test 关键词提前命中）。
fn session_ritual_matched(messages_in_order: &[&str]) -> bool {
    // 0=none 1=plan 2=confirm 3=checkbox 4=test
    let mut phase = 0usize;
    let mut confirm_hits = 0usize;
    for m in messages_in_order {
        let lower = m.to_lowercase();
        if phase < 1 && contains_any(&lower, KW_PLAN) {
            phase = 1;
            continue;
        }
        if phase == 1 && contains_any(&lower, KW_CONFIRM) {
            confirm_hits += 1;
            if confirm_hits >= CONFIRM_MIN_HITS {
                phase = 2;
            }
            continue;
        }
        if phase == 2 && contains_any(&lower, KW_CHECKBOX) {
            phase = 3;
            continue;
        }
        if phase == 3 && contains_any(&lower, KW_TEST) {
            phase = 4;
            continue;
        }
    }
    phase == 4
}

/// 仪式序列候选：按会话分组用户消息（时间升序由采集层保证），每会话过
/// [`session_ritual_matched`] 状态机；**≥2 个会话**命中完整链路 → 一条
/// workflow 条目（多会话重现=稳定习惯；单会话可能是任务偶然形态，不上榜）。
///
/// steps = 四阶段步骤清单（结构化注入面：模型读到的是「先计划、多轮
/// 确认、清单执行、测试收尾」的可执行流程，不是一句压扁的陈述）；evidence
/// = 命中的会话名（溯源）；confidence 随命中会话数爬升（2 会话=0.75，
/// 过 0.7 注入闸）。
pub fn ritual_candidates(rows: &[DialogueRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    // 按会话分组，组内保序（rows 已时间升序；BTreeMap 键排序只为稳定遍历）。
    let mut sessions: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for r in rows {
        if r.role == "user" {
            sessions
                .entry(r.session.as_str())
                .or_default()
                .push(&r.content);
        }
    }
    let matched: Vec<&str> = sessions
        .iter()
        .filter(|(_, msgs)| session_ritual_matched(msgs))
        .map(|(s, _)| *s)
        .collect();
    if matched.len() < RITUAL_MIN_SESSIONS {
        return out;
    }
    let n = matched.len();
    let mut entry = HabitEntry::auto_entry(
        "workflow",
        "计划式工作流：先制定计划，多轮回顾确认后转成步骤清单逐项执行，完成后测试验收",
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        n as u32,
        now,
    );
    entry.steps = HabitEntry::sanitize_steps(&[
        "先制定计划".to_string(),
        "回顾并确认计划正确性（多轮）".to_string(),
        "将计划转化为步骤清单/复选框，完成一项标记一项".to_string(),
        "全部完成后做测试验收".to_string(),
    ]);
    entry.evidence = matched
        .iter()
        .take(super::store::MAX_EVIDENCE)
        .map(|s| s.to_string())
        .collect();
    out.push(entry);
    out
}

// ---------- todo 收尾模式挖掘（例 1 第 3 步，G10） ----------

/// 收尾测试项关键词。
const TODO_TEST_KW: &[&str] = &["测试", "验证", "test", "检查"];
/// 会话 todo 参与判定的步骤数下限（1-2 步的清单谈不上流程仪式）。
const TODO_MIN_STEPS: usize = 3;
/// 收尾测试的 todo 快照占比闸。
const TODO_RATIO: f64 = 0.7;
/// 参与 todo 挖掘的快照数下限。
const TODO_MIN_SESSIONS: usize = 2;

fn is_test_step(content: &str) -> bool {
    let lower = content.to_lowercase();
    contains_any(&lower, TODO_TEST_KW)
}

/// 判定单个 todo 快照是否「测试收尾」：≥3 步且**最后两步**都是测试类
/// （「做好测试了么」的机器可读投影——todo 清单尾部挂测试项 = 用户把
/// 验收固化进了流程）。
fn snapshot_test_last(s: &TodoSnapshot) -> bool {
    if s.items.len() < TODO_MIN_STEPS {
        return false;
    }
    s.items.iter().rev().take(2).all(|(c, _)| is_test_step(c))
}

/// todo 收尾模式候选：占比 ≥0.7 × ≥2 个快照 → quality_bar 条目。
/// steps = 各命中快照的收尾测试项内容（去重清洗——模型读到的是用户真实
/// 的验收项措辞，不是抽象描述）；evidence = 命中的 todo 文件名。
pub fn todo_candidates(snapshots: &[TodoSnapshot], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    if snapshots.len() < TODO_MIN_SESSIONS {
        return out;
    }
    let hit: Vec<&TodoSnapshot> = snapshots.iter().filter(|s| snapshot_test_last(s)).collect();
    if hit.is_empty() {
        return out;
    }
    let ratio = hit.len() as f64 / snapshots.len() as f64;
    if ratio < TODO_RATIO {
        return out;
    }
    let n = hit.len();
    let mut entry = HabitEntry::auto_entry(
        "quality_bar",
        "任务清单常以测试/检查项收尾——完成主体步骤后主动做测试验证再交付",
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        n as u32,
        now,
    );
    let mut steps: Vec<String> = Vec::new();
    for s in &hit {
        for (c, _) in s.items.iter().rev().take(2) {
            if !steps.iter().any(|e| e == c) {
                steps.push(c.clone());
            }
        }
    }
    entry.steps = HabitEntry::sanitize_steps(&steps);
    entry.evidence = hit
        .iter()
        .take(super::store::MAX_EVIDENCE)
        .map(|s| s.file.clone())
        .collect();
    out.push(entry);
    out
}

// ---------- WSAPI 遥测前因挖掘（例 2，批三） ----------

/// WSAPI 遥测行投影（`<ws>/logs/wsapi_usage.jsonl` 单行）。
#[derive(Debug, Clone)]
pub struct UsageRow {
    pub ts_ms: i64,
    pub module: String,
    pub cmd: String,
    pub ok: bool,
}

/// 遥测行时间窗（诊断后 N 分钟内的集群/看板操作算一次前因 occurrence）。
const ANTECEDENT_WINDOW_MS: i64 = 60 * 60 * 1000;
/// 前因模式成立的去重日期数下限（≥3 天 = 跨日稳定行为）。
const ANTECEDENT_MIN_DAYS: usize = 3;

/// 读 wsapi_usage 遥测（磁盘 → [`UsageRow`]；缺失/损坏行跳过）。供 mod.rs
/// 编排调用；[`antecedent_candidates`] 本体保持纯函数。
pub fn read_usage_rows(workspace: &std::path::Path) -> Vec<UsageRow> {
    let path = workspace.join("logs").join("wsapi_usage.jsonl");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let ts_ms = v.get("ts_ms").and_then(|t| t.as_i64());
        let module = v.get("module").and_then(|m| m.as_str());
        let cmd = v.get("cmd").and_then(|c| c.as_str());
        if let (Some(ts), Some(m), Some(c)) = (ts_ms, module, cmd) {
            out.push(UsageRow {
                ts_ms: ts,
                module: m.to_string(),
                cmd: c.to_string(),
                ok: v.get("ok").and_then(|o| o.as_bool()).unwrap_or(true),
            });
        }
    }
    out.sort_by_key(|r| r.ts_ms);
    out
}

/// 时序前因候选（例 2 场景）：时间升序扫描遥测行——
/// - 标记行为：`module==cluster && cmd 以 "diagnostics" 开头`（集群诊断页
///   的诊断动作簇）→ 挂起「已诊断」时间戳；
/// - 后果行为：挂起窗内（60min）出现 `module==cluster`（非诊断）或
///   `module==board` → 记一次 occurrence（按 UTC 日期去重）；
/// - **≥3 个不同日期**都出现该序列 → workflow 条目「使用集群/看板前先做
///   集群诊断检查」（跨日重现 = 稳定行为习惯；单日可能是一时兴起）。
///
/// steps 是可执行的动作对（先诊断、确认在线、再操作）；evidence =
/// occurrence 日期（UTC YYYY-MM-DD，截 [`MAX_EVIDENCE`]）。
pub fn antecedent_candidates(rows: &[UsageRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let mut pending_diag: Option<i64> = None;
    let mut dates: Vec<String> = Vec::new();
    for r in rows {
        if r.module == "cluster" && r.cmd.starts_with("diagnostics") {
            pending_diag = Some(r.ts_ms);
            continue;
        }
        let is_consequence =
            (r.module == "cluster" && !r.cmd.starts_with("diagnostics")) || r.module == "board";
        if !is_consequence {
            continue;
        }
        if let Some(t0) = pending_diag {
            if r.ts_ms - t0 <= ANTECEDENT_WINDOW_MS {
                let date = chrono::DateTime::from_timestamp_millis(r.ts_ms)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                if !date.is_empty() && !dates.contains(&date) {
                    dates.push(date);
                }
                pending_diag = None; // 一次诊断只计一次 occurrence
            } else {
                pending_diag = None; // 窗外：诊断作废
            }
        }
    }
    if dates.len() < ANTECEDENT_MIN_DAYS {
        return out;
    }
    let n = dates.len();
    let mut entry = HabitEntry::auto_entry(
        "workflow",
        "使用集群/看板前先到集群诊断页执行一轮诊断，确认周边节点在线后再操作",
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        n as u32,
        now,
    );
    entry.steps = HabitEntry::sanitize_steps(&[
        "操作前先打开集群诊断页执行一轮诊断".to_string(),
        "确认周边节点在线/状态正常".to_string(),
        "再进行集群或看板操作".to_string(),
    ]);
    entry.evidence = dates.into_iter().take(super::store::MAX_EVIDENCE).collect();
    out.push(entry);
    out
}

// ---------- 批 B：页面足迹链挖掘（route 序列仪式，§四 1） ----------

/// 页面足迹链成立的去重日期数下限（跨 ≥3 日重现 = 稳定导航仪式）。
const ROUTE_MIN_DAYS: usize = 3;
/// 单次挖掘最多产出的导航条目数（防热点页对刷屏）。
const ROUTE_MAX_ENTRIES: usize = 2;

/// 页面足迹链候选：`module=="route"` 遥测行（Dashboard 侧栏导航打点）按
/// UTC 日分组，组内**相邻去重**成页面序列，序列的相邻对 `(from, to)` 按
/// 出现日期去重计数；同一转移跨 ≥[`ROUTE_MIN_DAYS`] 个不同日期 → workflow
/// 条目「先 X 再 Y」（steps=两步页面序列，evidence=日期）。保守阈值：只挖
/// 相邻对不挖长链；产出条目封顶（日期多者优先）。
pub fn route_chain_candidates(rows: &[UsageRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    // 按日切页面序列（rows 时间升序；组内相邻去重）。
    let mut days: Vec<(String, Vec<String>)> = Vec::new();
    for r in rows {
        if r.module != "route" || r.cmd.is_empty() {
            continue;
        }
        let day = chrono::DateTime::from_timestamp_millis(r.ts_ms)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        match days.last_mut() {
            Some((d, _)) if *d == day => {}
            _ => days.push((day, Vec::new())),
        }
        let (_, seq) = days.last_mut().expect("刚 push 过");
        if seq.last() != Some(&r.cmd) {
            seq.push(r.cmd.clone());
        }
    }
    // (from, to) → 出现过的 UTC 日期（去重）。
    let mut transitions: std::collections::BTreeMap<(String, String), Vec<String>> =
        Default::default();
    for (day, seq) in &days {
        for pair in seq.windows(2) {
            let entry = transitions
                .entry((pair[0].clone(), pair[1].clone()))
                .or_default();
            if !entry.iter().any(|d| d == day) {
                entry.push(day.clone());
            }
        }
    }

    // 日期多者优先（BTreeMap 键序只做稳定 tie-break）。
    let mut hits: Vec<((String, String), Vec<String>)> = transitions
        .into_iter()
        .filter(|(_, dates)| dates.len() >= ROUTE_MIN_DAYS)
        .collect();
    hits.sort_by_key(|(_, dates)| std::cmp::Reverse(dates.len()));
    for ((from, to), dates) in hits.into_iter().take(ROUTE_MAX_ENTRIES) {
        let n = dates.len();
        let mut entry = HabitEntry::auto_entry(
            "workflow",
            &format!("导航习惯：常先访问 {from} 页再进入 {to} 页"),
            (0.65 + 0.05 * n.min(4) as f64).min(0.85),
            n as u32,
            now,
        );
        entry.steps =
            HabitEntry::sanitize_steps(&[format!("先访问 {from} 页"), format!("再进入 {to} 页")]);
        entry.evidence = dates.into_iter().take(super::store::MAX_EVIDENCE).collect();
        out.push(entry);
    }
    out
}

// ---------- 批 B：todo 完成/中弃统计（§四 2，统计面不注入） ----------

/// todo 快照完成度统计（批 B 出数；批 D Dashboard 消费。**统计面永不注入**
/// ——注入克制原则，防上下文膨胀）。
#[derive(Debug, Clone, PartialEq)]
pub struct TodoStats {
    /// 参与统计的快照数。
    pub snapshot_count: usize,
    /// 全部条目数。
    pub item_count: usize,
    /// 已完成条目数（status=="completed"）。
    pub completed_count: usize,
    /// 完成率 = completed / item_count（无条目 = 0.0）。
    pub completion_ratio: f64,
    /// 仍含未完成项的快照数（中弃候选——「列了没做完」的清单）。
    pub snapshots_with_pending: usize,
}

/// 跨快照聚合完成度（纯统计，无候选产出）。
pub fn todo_completion_stats(snapshots: &[TodoSnapshot]) -> TodoStats {
    let mut stats = TodoStats {
        snapshot_count: snapshots.len(),
        item_count: 0,
        completed_count: 0,
        completion_ratio: 0.0,
        snapshots_with_pending: 0,
    };
    for s in snapshots {
        let mut has_pending = false;
        for (_, status) in &s.items {
            stats.item_count += 1;
            if status == "completed" {
                stats.completed_count += 1;
            } else {
                has_pending = true;
            }
        }
        if has_pending {
            stats.snapshots_with_pending += 1;
        }
    }
    if stats.item_count > 0 {
        stats.completion_ratio = stats.completed_count as f64 / stats.item_count as f64;
    }
    stats
}

// ---------- 批 B：todo 步骤类别序列（§四 3） ----------

/// 部署/发布类步骤关键词（`.contains` 语义，调用方转小写；与收尾测试词表
/// 正交——「检查」归测试类）。
const SEQ_DEPLOY_KW: &[&str] = &["部署", "发布", "上线", "deploy", "release"];
/// 含部署步骤的快照参与判定的最少步数。
const SEQ_DEPLOY_MIN_STEPS: usize = 3;
/// 部署收尾判定的快照数下限。
const SEQ_DEPLOY_MIN_SNAPSHOTS: usize = 3;
/// 「部署步收尾」的快照占比闸。
const SEQ_DEPLOY_RATIO: f64 = 0.7;

fn has_any_kw(lower: &str, kws: &[&str]) -> bool {
    kws.iter().any(|k| lower.contains(k))
}

/// todo 步骤类别序列候选：「部署/发布」类步骤常收尾——在含部署类步骤且
/// ≥[`SEQ_DEPLOY_MIN_STEPS`] 步的快照里，「部署步位于最后两步」的占比
/// ≥[`SEQ_DEPLOY_RATIO`] 且快照数 ≥[`SEQ_DEPLOY_MIN_SNAPSHOTS`] →
/// quality_bar 条目（交付序列里部署/发布环节固定殿后）。steps = 命中快照
/// 的部署步原文（去重清洗）；evidence = 快照文件名。
pub fn todo_sequence_candidates(snapshots: &[TodoSnapshot], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let mut with_deploy = 0usize;
    let mut hit: Vec<&TodoSnapshot> = Vec::new();
    let mut deploy_steps: Vec<String> = Vec::new();
    for s in snapshots {
        if s.items.len() < SEQ_DEPLOY_MIN_STEPS {
            continue;
        }
        let deploy_positions: Vec<usize> = s
            .items
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| has_any_kw(&c.to_lowercase(), SEQ_DEPLOY_KW))
            .map(|(i, _)| i)
            .collect();
        if deploy_positions.is_empty() {
            continue;
        }
        with_deploy += 1;
        let last = s.items.len() - 1;
        // 收尾 = 最后两步之一（len≥3 时 last≥2，无下溢）。
        let tail_hit = deploy_positions.iter().any(|i| *i + 1 >= last);
        if tail_hit {
            hit.push(s);
            for (c, _) in s
                .items
                .iter()
                .enumerate()
                .filter(|(i, _)| *i + 1 >= last)
                .filter(|(i, _)| deploy_positions.contains(i))
                .map(|(_, item)| item)
            {
                if !deploy_steps.iter().any(|e| e == c) {
                    deploy_steps.push(c.clone());
                }
            }
        }
    }
    if with_deploy < SEQ_DEPLOY_MIN_SNAPSHOTS {
        return out;
    }
    let ratio = hit.len() as f64 / with_deploy as f64;
    if ratio < SEQ_DEPLOY_RATIO {
        return out;
    }
    let n = hit.len();
    let mut entry = HabitEntry::auto_entry(
        "quality_bar",
        "清单中部署/发布类步骤常收尾——主体完成后预留部署验证环节再交付",
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        n as u32,
        now,
    );
    entry.steps = HabitEntry::sanitize_steps(&deploy_steps);
    entry.evidence = hit
        .iter()
        .take(super::store::MAX_EVIDENCE)
        .map(|s| s.file.clone())
        .collect();
    out.push(entry);
    out
}

// ---------- 批 B：开场白模式（§四 4） ----------

/// 开场白成立的会话数下限。
const OPENER_MIN_SESSIONS: usize = 3;
/// 主流形态占比闸。
const OPENER_DOMINANCE: f64 = 0.7;

/// 每会话首条 user 消息的形态分类（判定顺序：继续上次 → 贴报错 → 寒暄 →
/// 直接派活兜底——「继续昨天的报错排查」是继续不是贴报错）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OpenerKind {
    /// 直接派活。
    Task,
    /// 寒暄铺垫。
    Greeting,
    /// 贴报错开场。
    ErrorPaste,
    /// 继续之前的活。
    Resume,
}

fn classify_opener(content: &str) -> OpenerKind {
    let t = content.trim_start();
    const RESUME_PREFIX: &[&str] = &["继续", "接着", "上次", "昨天", "之前"];
    if RESUME_PREFIX.iter().any(|p| t.starts_with(p)) {
        return OpenerKind::Resume;
    }
    let lower = t.to_lowercase();
    const ERR_KW: &[&str] = &[
        "报错",
        "错误",
        "error",
        "failed",
        "失败",
        "exception",
        "panic",
        "traceback",
        "堆栈",
    ];
    if lower.contains("```") || ERR_KW.iter().any(|k| lower.contains(k)) {
        return OpenerKind::ErrorPaste;
    }
    const GREET_KW: &[&str] = &[
        "你好",
        "您好",
        "hi",
        "hello",
        "在吗",
        "早上好",
        "下午好",
        "晚上好",
        "辛苦",
    ];
    if t.chars().count() <= 30 && GREET_KW.iter().any(|k| lower.contains(k)) {
        return OpenerKind::Greeting;
    }
    OpenerKind::Task
}

fn opener_content(kind: OpenerKind) -> &'static str {
    match kind {
        OpenerKind::Task => "用户开场即直接派活，无需寒暄——可直接确认任务要点开工",
        OpenerKind::Greeting => "用户开场常寒暄铺垫——先简短回应问候再进入任务",
        OpenerKind::ErrorPaste => "用户常直接贴报错开场——先读报错定位问题，无需追问背景",
        OpenerKind::Resume => "用户常接着之前的进度开场——先回顾上次进展再继续",
    }
}

/// 开场白模式候选：每会话取首条 user 消息分类（rows 时间升序由采集层保证，
/// 组内同毫秒保持文件/行序）；会话数 ≥[`OPENER_MIN_SESSIONS`] 且主流形态
/// 占比 ≥[`OPENER_DOMINANCE`] → style 条目（主流形态的配合姿态一句话）。
/// evidence = 主流形态命中的会话名。
pub fn opener_candidates(rows: &[DialogueRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let mut firsts: std::collections::BTreeMap<&str, (&str, OpenerKind)> = Default::default();
    for r in rows {
        if r.role != "user" || r.content.trim().is_empty() {
            continue;
        }
        firsts
            .entry(r.session.as_str())
            .or_insert((r.content.as_str(), classify_opener(&r.content)));
    }
    if firsts.len() < OPENER_MIN_SESSIONS {
        return out;
    }
    let mut by_kind: std::collections::BTreeMap<OpenerKind, Vec<&str>> = Default::default();
    for (session, (_, kind)) in &firsts {
        by_kind.entry(*kind).or_default().push(session);
    }
    let Some((dominant, sessions)) = by_kind
        .iter()
        .max_by_key(|(_, s)| s.len())
        .map(|(k, s)| (*k, s.clone()))
    else {
        return out;
    };
    let ratio = sessions.len() as f64 / firsts.len() as f64;
    if ratio < OPENER_DOMINANCE {
        return out;
    }
    let n = sessions.len();
    let mut entry = HabitEntry::auto_entry(
        "style",
        opener_content(dominant),
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        n as u32,
        now,
    );
    entry.evidence = sessions
        .into_iter()
        .take(super::store::MAX_EVIDENCE)
        .map(|s| s.to_string())
        .collect();
    out.push(entry);
    out
}

// ---------- 批 B：催促/复述模式（§四 7） ----------

/// 复述判定窗（同文重发间隔上限）。
const URGE_WINDOW_MS: i64 = 2 * 60 * 1000;
/// 一簇成立的发送数下限（原发 + ≥2 次复述）。
const URGE_MIN_SENDS: usize = 3;
/// 催促成立的簇数下限（跨内容/跨时点重现；单簇可能是复制粘贴事故）。
const URGE_MIN_BURSTS: usize = 2;

/// 复述归一（trim + 压空白——只做保守归一，不猜同义词）。
fn norm_urge(s: &str) -> String {
    s.split_whitespace().collect()
}

/// 催促/复述候选：user 消息时间升序扫描——同归一文本、间隔 ≤2min 的连发
/// 构成一簇，簇内 ≥[`URGE_MIN_SENDS`] 条 = 一次催促；催促簇（按归一文本
/// 去重）≥[`URGE_MIN_BURSTS`] → workflow 条目「等待约 2 分钟会复述催促」
/// （agent 侧配合姿态：长操作主动汇报进度）。evidence = 簇首条内容截断。
pub fn urge_candidates(rows: &[DialogueRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    // 单遍状态机：同归一文本、间隔 ≤2min 的连发构成一簇；簇闭合时达
    // URGE_MIN_SENDS 记一次催促（按归一文本聚合簇数/总发送数/簇首条）。
    let mut by_norm: std::collections::BTreeMap<String, (usize, String, usize)> =
        Default::default();
    let mut cur_norm = String::new();
    let mut cur_count = 0usize;
    let mut cur_first = String::new();
    let mut cur_last_ts = 0i64;
    for r in rows {
        if r.role != "user" {
            continue;
        }
        let norm = norm_urge(&r.content);
        if norm.is_empty() {
            continue;
        }
        let continues =
            norm == cur_norm && cur_count > 0 && r.ts_ms - cur_last_ts <= URGE_WINDOW_MS;
        if continues {
            cur_count += 1;
            cur_last_ts = r.ts_ms;
        } else {
            if cur_count >= URGE_MIN_SENDS {
                let e =
                    by_norm
                        .entry(cur_norm.clone())
                        .or_insert((0usize, cur_first.clone(), 0usize));
                e.0 += 1;
                e.2 += cur_count;
            }
            cur_norm = norm;
            cur_count = 1;
            cur_first = r.content.trim().chars().take(40).collect();
            cur_last_ts = r.ts_ms;
        }
    }
    if cur_count >= URGE_MIN_SENDS {
        let e = by_norm
            .entry(cur_norm.clone())
            .or_insert((0usize, cur_first.clone(), 0usize));
        e.0 += 1;
        e.2 += cur_count;
    }

    let qualified: Vec<(&String, &(usize, String, usize))> =
        by_norm.iter().filter(|(_, (n, _, _))| *n > 0).collect();
    if qualified.len() < URGE_MIN_BURSTS {
        return out;
    }
    let total_sends: usize = qualified.iter().map(|(_, (_, _, s))| s).sum();
    let n = qualified.len();
    let mut entry = HabitEntry::auto_entry(
        "workflow",
        "用户等待约 2 分钟无响应会复述/催促——长操作中主动汇报进度，不要长时间静默",
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        total_sends as u32,
        now,
    );
    entry.evidence = qualified
        .iter()
        .take(super::store::MAX_EVIDENCE)
        .map(|(_, (_, first, _))| first.clone())
        .collect();
    out.push(entry);
    out
}

// ---------- 批 B：分工风格（§四 8） ----------

/// 微观管理判定：fast 插话窗（user 紧跟 assistant 之后）。
const MICRO_WINDOW_MS: i64 = 90 * 1000;
/// 参与微观判定的会话 user 消息数下限。
const MICRO_MIN_USER_MSGS: usize = 4;
/// fast 插话占比闸。
const MICRO_RATIO: f64 = 0.6;
/// 微观成立的会话数下限。
const MICRO_MIN_SESSIONS: usize = 2;
/// 派单判定：两次 user 消息间的静默窗。
const DISPATCH_SILENT_MS: i64 = 10 * 60 * 1000;
/// 派单成立的静默段总数下限。
const DISPATCH_MIN_STRETCHES: usize = 4;
/// 派单成立的会话数下限。
const DISPATCH_MIN_SESSIONS: usize = 2;

/// 分工风格候选（对话时间结构，主语=用户的工作组织方式）：
/// - **微观管理型**：≥2 个会话各满足「user 消息 ≥4 条且 fast 插话（紧跟
///   assistant 之后 ≤90s）占比 ≥0.6」，且全数据静默段 <2 → agent 小步推进
///   高频汇报；
/// - **派单型**：静默段（两次 user 之间 >10min 且中间有 assistant 活动）
///   总数 ≥4 且跨 ≥2 会话，且零微观会话 → agent 自主推进攒大结果再汇报；
/// - 两态并存（既常插话又常长沉默）= 风格不明，不上榜（宁缺勿滥）。
pub fn delegation_candidates(rows: &[DialogueRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let mut sessions: std::collections::BTreeMap<&str, Vec<&DialogueRow>> = Default::default();
    for r in rows {
        sessions.entry(r.session.as_str()).or_default().push(r);
    }
    let mut micro_sessions: Vec<&str> = Vec::new();
    let mut stretch_sessions: Vec<&str> = Vec::new();
    let mut total_stretches = 0usize;
    for (session, srows) in &sessions {
        // fast 插话：紧跟 assistant 行且间隔 ≤MICRO_WINDOW_MS 的 user 行。
        let user_count = srows.iter().filter(|r| r.role == "user").count();
        if user_count >= MICRO_MIN_USER_MSGS {
            let mut fast = 0usize;
            for (i, r) in srows.iter().enumerate() {
                if r.role == "user"
                    && i > 0
                    && srows[i - 1].role == "assistant"
                    && r.ts_ms - srows[i - 1].ts_ms <= MICRO_WINDOW_MS
                {
                    fast += 1;
                }
            }
            if fast as f64 / user_count as f64 >= MICRO_RATIO {
                micro_sessions.push(session);
            }
        }
        // 静默段：相邻两条 user 间隔 >DISPATCH_SILENT_MS 且中间有 assistant 行。
        let user_rows: Vec<&DialogueRow> =
            srows.iter().filter(|r| r.role == "user").copied().collect();
        let mut session_stretches = 0usize;
        for w in user_rows.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b.ts_ms - a.ts_ms > DISPATCH_SILENT_MS
                && srows
                    .iter()
                    .any(|r| r.role == "assistant" && r.ts_ms > a.ts_ms && r.ts_ms < b.ts_ms)
            {
                session_stretches += 1;
            }
        }
        if session_stretches > 0 {
            stretch_sessions.push(session);
            total_stretches += session_stretches;
        }
    }

    if micro_sessions.len() >= MICRO_MIN_SESSIONS && total_stretches < 2 {
        let n = micro_sessions.len();
        let mut entry = HabitEntry::auto_entry(
            "workflow",
            "用户是微观管理型：工作过程中高频插入细节指令——小步推进、频繁汇报中间状态",
            (0.65 + 0.05 * n.min(4) as f64).min(0.85),
            n as u32,
            now,
        );
        entry.evidence = micro_sessions
            .into_iter()
            .take(super::store::MAX_EVIDENCE)
            .map(|s| s.to_string())
            .collect();
        out.push(entry);
    } else if micro_sessions.is_empty()
        && total_stretches >= DISPATCH_MIN_STRETCHES
        && stretch_sessions.len() >= DISPATCH_MIN_SESSIONS
    {
        let n = total_stretches;
        let mut entry = HabitEntry::auto_entry(
            "workflow",
            "用户是派单型：派活后长时间静默——自主推进、攒齐完整结果再一次性汇报",
            (0.65 + 0.05 * n.min(4) as f64).min(0.85),
            n as u32,
            now,
        );
        entry.evidence = stretch_sessions
            .into_iter()
            .take(super::store::MAX_EVIDENCE)
            .map(|s| s.to_string())
            .collect();
        out.push(entry);
    }
    out
}

// ---------- 批 B：返工节奏（§四 9，rewind 用户读法） ----------

/// 迭代型成立的 rewind 数下限（窗内 ≥4 次回退精修）。
const REWORK_MIN_REWINDS: usize = 4;
/// 一次成型型成立的会话数下限（多会话零回退）。
const ONESHOT_MIN_SESSIONS: usize = 6;

/// 返工节奏候选（rewind 的**用户读法**——主语=用户的工作节奏，不是
/// 「agent 哪里做错了」）：
/// - rewind ≥[`REWORK_MIN_REWINDS`] → 迭代型（先快出第一版看完再回退精修
///   ——agent 大胆快速出稿、减少事前反复确认）；
/// - 窗内会话 ≥[`ONESHOT_MIN_SESSIONS`] 且零 rewind → 一次成型型（重要
///   改动先对齐方案再动手）。
///
/// 输入由编排层供给：rewind 计数 = events.jsonl 尾部 rewind 行；会话数 =
/// 证据窗内出现过的会话数。
pub fn rework_candidates(rewind_count: usize, session_count: usize, now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    if rewind_count >= REWORK_MIN_REWINDS {
        let mut entry = HabitEntry::auto_entry(
            "workflow",
            "用户偏迭代式返工：先快出第一版看完再回退精修——大胆快速出稿，减少事前反复确认",
            (0.65 + 0.05 * rewind_count.min(4) as f64).min(0.85),
            rewind_count as u32,
            now,
        );
        entry.evidence = vec![format!("rewind×{rewind_count}")];
        out.push(entry);
    } else if rewind_count == 0 && session_count >= ONESHOT_MIN_SESSIONS {
        let mut entry = HabitEntry::auto_entry(
            "workflow",
            "用户偏一次成型：重要改动先对齐方案再动手——动手前先给方案/计划征得确认",
            0.72,
            session_count as u32,
            now,
        );
        entry.evidence = vec![format!("sessions×{session_count} 无回退")];
        out.push(entry);
    }
    out
}

// ---------- 批 B：障碍处置顺序（§四 10，失败重试用户读法） ----------

/// 同操作重发判定窗。
const RETRY_WINDOW_MS: i64 = 5 * 60 * 1000;
/// 重试模式成立的去重日期数下限。
const RETRY_MIN_DAYS: usize = 3;

/// 障碍处置顺序候选（失败重试的**用户读法**）：遥测行时间升序扫描——
/// `ok=false` 行后 [`RETRY_WINDOW_MS`] 内同 `(module, cmd)` 成功重发 → 记
/// 一次「自己重试解决」；跨 ≥[`RETRY_MIN_DAYS`] 个不同日期重现 → workflow
/// 条目（用户习惯自己折腾几次才求助——agent 看到报错直接给解法与下一步，
/// 别只陈述失败）。evidence = 日期。
pub fn retry_candidates(rows: &[UsageRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let mut dates: Vec<String> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if r.ok {
            continue;
        }
        for later in &rows[i + 1..] {
            if later.ts_ms - r.ts_ms > RETRY_WINDOW_MS {
                break;
            }
            if later.module == r.module && later.cmd == r.cmd && later.ok {
                let date = chrono::DateTime::from_timestamp_millis(r.ts_ms)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                if !date.is_empty() && !dates.contains(&date) {
                    dates.push(date);
                }
                break;
            }
        }
    }
    if dates.len() < RETRY_MIN_DAYS {
        return out;
    }
    let n = dates.len();
    let mut entry = HabitEntry::auto_entry(
        "workflow",
        "用户遇到操作失败习惯立即自己重试——给出错时直接附解法与下一步动作，不要只陈述失败",
        (0.65 + 0.05 * n.min(4) as f64).min(0.85),
        n as u32,
        now,
    );
    entry.evidence = dates.into_iter().take(super::store::MAX_EVIDENCE).collect();
    out.push(entry);
    out
}

// ---------- 批 B：审批交互弱信号（§四 11，姿态层） ----------

/// 参与判定的 respond 事件数下限。
const APPROVAL_MIN_EVENTS: usize = 5;
/// 拉锯判定失败占比闸。
const APPROVAL_FRICTION_RATIO: f64 = 0.3;
/// 顺畅判定失败占比闸（≤10%）。
const APPROVAL_SMOOTH_RATIO: f64 = 0.1;

/// 审批交互弱信号候选（**只做交流姿态**——权限/安全语义照旧走安全层，
/// 一个字不改）：wsapi 遥测里 `approval.respond` 的 ok/fail 分布——
/// - 失败占比 ≥0.3 → 「审批交互常有拉锯——提交审批时附完整上下文减少来回」；
/// - 失败占比 ≤0.1 → 「审批决策快而稳——审批卡说明保持精炼、要点先行」。
///
/// 中间带（0.1~0.3）不上榜。evidence = ok/fail 计数（溯源到数据面本身）。
pub fn approval_signal_candidates(rows: &[UsageRow], now: &str) -> Vec<HabitEntry> {
    let mut out = Vec::new();
    let responds = rows
        .iter()
        .filter(|r| r.module == "approval" && r.cmd == "respond");
    let total = responds.clone().count();
    if total < APPROVAL_MIN_EVENTS {
        return out;
    }
    let fails = responds.filter(|r| !r.ok).count();
    let ratio = fails as f64 / total as f64;
    let (content, category) = if ratio >= APPROVAL_FRICTION_RATIO {
        (
            "审批交互常有拉锯（失败/重试占比高）——提交审批请求时附完整上下文与影响面，减少来回",
            "workflow",
        )
    } else if ratio <= APPROVAL_SMOOTH_RATIO {
        (
            "用户对常规操作的审批决策快而稳——审批卡说明保持精炼、要点先行即可",
            "trust",
        )
    } else {
        return out;
    };
    let mut entry = HabitEntry::auto_entry(
        category,
        content,
        (0.65 + 0.05 * total.min(4) as f64).min(0.85),
        total as u32,
        now,
    );
    entry.evidence = vec![
        format!("respond ok={}", total - fails),
        format!("respond fail={fails}"),
    ];
    out.push(entry);
    out
}

/// 工作域画像阈值：带文件接触的 assistant 行数下限（宁缺勿滥——低于此
/// 样本的「主导种类」是噪声）。
const WORK_DOMAIN_MIN_ROWS: usize = 10;
/// 工作域画像阈值：主导种类占比下限（六成以上才算「为主」）。
const WORK_DOMAIN_DOMINANCE: f64 = 0.6;

/// 工作域挖掘（批 C ①）：assistant 行文件接触种类（`file_changes` +
/// `deliverables` 路径扩展名分类，采集层投影）占比统计。种类分布一超
/// 阈值 → 工作域条目（workflow 类——指导交付形态）。`other` 不出条目
/// （未知扩展名的「主导」没有可执行含义，宁缺勿滥）。
pub fn work_domain_candidates(rows: &[DialogueRow], now: &str) -> Vec<HabitEntry> {
    let touched: Vec<&DialogueRow> = rows
        .iter()
        .filter(|r| r.role == "assistant" && !r.file_kinds.is_empty())
        .collect();
    if touched.len() < WORK_DOMAIN_MIN_ROWS {
        return Vec::new();
    }
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for r in &touched {
        for k in &r.file_kinds {
            *counts.entry(k.as_str()).or_default() += 1;
        }
    }
    let Some((dominant, count)) = counts.iter().max_by_key(|(_, c)| **c) else {
        return Vec::new();
    };
    let (dominant, count) = (*dominant, *count);
    if dominant == "other" {
        return Vec::new();
    }
    let ratio = count as f64 / touched.len() as f64;
    if ratio < WORK_DOMAIN_DOMINANCE {
        return Vec::new();
    }
    let content = match dominant {
        "code" => "用户的任务以代码工作为主——改动后主动跑构建/测试验证，不要停在「应该能编译」",
        "doc" => "用户的任务以文档写作为主——交付注意结构、措辞与可读性，产出即成品",
        "config" => "用户的任务以配置调优为主——改动前先核对现值，改动后说明生效条件与回滚方式",
        "image" => "用户的任务以图像素材处理为主——产出注意尺寸、格式与命名规范",
        _ => return Vec::new(),
    };
    let mut entry = HabitEntry::auto_entry(
        "workflow",
        content,
        (0.65 + 0.05 * touched.len().min(4) as f64).min(0.85),
        count as u32,
        now,
    );
    entry.evidence = vec![format!("files {dominant}={count}/{}", touched.len())];
    vec![entry]
}

/// 模型切换阈值：证据窗内切换次数下限（偶发两次是试用，三次起是习惯）。
const MODEL_SWITCH_MIN: usize = 3;

/// 模型切换行为挖掘（批 C ③ 替代形态）：assistant 行 `model` 徽标的
/// 相邻变化次数 + user 行 `/model` slash 指令次数。切换 ≥ 阈值 →
/// tool_preference 条目（「按任务换模型」是 deliberate 行为——多模型
/// 部署点的真实信号；替代原计划的 request_logs 接入，chat_log 每行
/// 自带 model 徽标，同一信号零新采集面）。
pub fn model_switch_candidates(rows: &[DialogueRow], now: &str) -> Vec<HabitEntry> {
    let mut switches = 0usize;
    let mut slash_switches = 0usize;
    let mut last_model = String::new();
    for r in rows {
        if r.role == "assistant" && !r.model.is_empty() {
            // 首次见到模型名 = 窗口起点状态，不算切换（否则每窗白送 +1）。
            if !last_model.is_empty() && r.model != last_model {
                switches += 1;
            }
            last_model = r.model.clone();
        } else if r.role == "user" && r.content.trim_start().starts_with("/model") {
            slash_switches += 1;
        }
    }
    let total = switches + slash_switches;
    if total < MODEL_SWITCH_MIN {
        return Vec::new();
    }
    let mut entry = HabitEntry::auto_entry(
        "tool_preference",
        "用户会按任务切换模型——对模型能力差异敏感，注意不同模型的工具调用形态差异，不要假定单一模型持续可用",
        (0.65 + 0.05 * total.min(4) as f64).min(0.85),
        total as u32,
        now,
    );
    entry.evidence = vec![
        format!("model changes={switches}"),
        format!("/model={slash_switches}"),
    ];
    vec![entry]
}

/// 审批强信号阈值：signals.jsonl 审批卡样本下限 + 决策耗时两档
/// （快 = 秒批，慢 = 系统性迟疑；中间带 = 无结论不出条目）。
const APPROVAL_STRONG_MIN_EVENTS: usize = 5;
const APPROVAL_FAST_MS: u64 = 5_000;
const APPROVAL_SLOW_MS: u64 = 60_000;

/// 审批交互强信号挖掘（批 C ④）：signals.jsonl 的审批卡决策耗时
/// （`duration_ms` = 卡片创建到裁决的墙钟，web_approval respond/respond_batch
/// 落盘）。**只加观测读法，审批语义一个字不改**——条目只影响注入面
/// 的建议文本。耗时取**批准卡**平均（拒绝可能是谨慎速拒，不混入
/// 「决策快」读法）。
pub fn approval_strong_candidates(signals: &[serde_json::Value], now: &str) -> Vec<HabitEntry> {
    let approved: Vec<u64> = signals
        .iter()
        .filter(|s| s.get("kind").and_then(|k| k.as_str()) == Some("approval"))
        .filter_map(|s| {
            let approved = s.get("approved").and_then(|a| a.as_bool())?;
            let duration = s.get("duration_ms").and_then(|d| d.as_u64())?;
            approved.then_some(duration)
        })
        .collect();
    if approved.len() < APPROVAL_STRONG_MIN_EVENTS {
        return Vec::new();
    }
    let avg = approved.iter().sum::<u64>() / approved.len() as u64;
    let (content, category) = if avg <= APPROVAL_FAST_MS {
        (
            "用户审批决策快而稳（平均秒级通过）——常规操作的审批请求把要点写清楚即可，不必前置反复确认",
            "trust",
        )
    } else if avg >= APPROVAL_SLOW_MS {
        (
            "用户审批响应慢（常超一分钟才决策）——审批请求附完整 why/impact 与影响面，给足一次决策所需的上下文",
            "workflow",
        )
    } else {
        return Vec::new();
    };
    let mut entry = HabitEntry::auto_entry(
        category,
        content,
        (0.65 + 0.05 * approved.len().min(4) as f64).min(0.85),
        approved.len() as u32,
        now,
    );
    entry.evidence = vec![format!("cards={}", approved.len()), format!("avg_ms={avg}")];
    vec![entry]
}

// ---------- 终端会话挖掘（2026-11-24 T 批） ----------

/// shell 偏好挖掘的最低用过会话数。
const SHELL_PREF_MIN_SESSIONS: usize = 3;
/// shell 偏好挖掘的主导占比闸（≥70% 认定偏好）。
const SHELL_PREF_DOMINANT_RATIO: f64 = 0.7;
/// 命令族挖掘的最低总次数。
const CMD_FAMILY_MIN_TOTAL: usize = 8;
/// 命令族挖掘的最低跨越会话数（单会话循环敲同一条不构成习惯）。
const CMD_FAMILY_MIN_SESSIONS: usize = 3;
/// 命令族上榜条数上限（top-K）。
const CMD_FAMILY_TOP_K: usize = 3;
/// 噪声命令（shell 内建/无信息量）：不参与命令族挖掘。
const CMD_NOISE: &[&str] = &[
    "exit", "clear", "cls", "cd", "ls", "dir", "pwd", "history", "q", "help", "man",
];

/// 首命令 token（首 token = 程序名；**参数不采集**——参数里可能出现路径/
/// 主机名/密钥等敏感面，挖掘器只看「用什么命令」）。路径形态 argv[0]
/// （绝对路径/UNC/`./x.sh`）降为文件名（BUG-20261125-005）：首 token
/// 本身是路径时携带用户名/主机名/目录结构，与「参数含路径」同一敏感面
/// ——原样进条目 content 会落盘画像并注入全部会话、随每次 LLM 请求上云。
/// 引号含空格路径退化为目录尾段（split_whitespace 拆不了带空格路径），
/// 敏感面（盘符+目录）已剥，程序名失真是可接受退化。
fn first_token(line: &str) -> Option<&str> {
    let t = line.split_whitespace().next()?;
    let t = t.trim_matches(|c: char| matches!(c, '"' | '\'' | '`'));
    let base = t.rsplit(['/', '\\']).next().unwrap_or(t);
    if base.is_empty() { None } else { Some(base) }
}

/// shell 偏好挖掘（T 批）：≥3 个用过终端的会话且主导 shell 占比 ≥70%
/// → environment 条目（「终端默认用 pwsh」）。缺 `# shell:` 头声明的
/// 会话不参与占比分母（未知 shell 无读法）。
pub fn shell_preference_candidates(
    sessions: &[super::collection::TerminalSession],
    now: &str,
) -> Vec<HabitEntry> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut known = 0usize;
    for s in sessions {
        if s.shell.is_empty() {
            continue;
        }
        known += 1;
        *counts.entry(s.shell.to_lowercase()).or_default() += 1;
    }
    if known < SHELL_PREF_MIN_SESSIONS {
        return Vec::new();
    }
    let Some((shell, cnt)) = counts.iter().max_by_key(|(_, c)| **c) else {
        return Vec::new();
    };
    let ratio = *cnt as f64 / known as f64;
    if ratio < SHELL_PREF_DOMINANT_RATIO {
        return Vec::new();
    }
    let mut entry = HabitEntry::auto_entry(
        "environment",
        &format!("用户终端默认用 {shell}——环境/安装类命令与说明按 {shell} 语法给出"),
        (0.6 + ratio * 0.2).min(0.85),
        *cnt as u32,
        now,
    );
    entry.evidence = vec![format!("sessions={known}"), format!("ratio={ratio:.2}")];
    vec![entry]
}

/// 常用命令族挖掘（T 批）：首 token 总次数 ≥8 且跨越 ≥3 个会话且不在
/// 噪声表 → tool_preference 条目 top-3（按次数降序；并列按名字典序——
/// 纯函数确定性）。**参数不采集**（敏感面零暴露）。
pub fn terminal_command_candidates(
    sessions: &[super::collection::TerminalSession],
    now: &str,
) -> Vec<HabitEntry> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut session_spread: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();
    for (i, s) in sessions.iter().enumerate() {
        let sid = format!("s{i}");
        for line in &s.commands {
            let Some(tok) = first_token(line) else {
                continue;
            };
            let tok = tok.to_lowercase();
            if CMD_NOISE.contains(&tok.as_str()) {
                continue;
            }
            *counts.entry(tok.clone()).or_default() += 1;
            session_spread.entry(tok).or_default().insert(sid.clone());
        }
    }
    let mut qualified: Vec<(String, usize, usize)> = counts
        .into_iter()
        .filter(|(cmd, total)| {
            *total >= CMD_FAMILY_MIN_TOTAL
                && session_spread.get(cmd).map(|s| s.len()).unwrap_or(0) >= CMD_FAMILY_MIN_SESSIONS
        })
        .map(|(cmd, total)| {
            let spread = session_spread.get(&cmd).map(|s| s.len()).unwrap_or(0);
            (cmd, total, spread)
        })
        .collect();
    // 次数降序，并列按名字典序（BTreeMap 迭代序兜底确定性）。
    qualified.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if qualified.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (cmd, total, spread) in qualified.into_iter().take(CMD_FAMILY_TOP_K) {
        let mut entry = HabitEntry::auto_entry(
            "tool_preference",
            &format!("用户终端常用 {cmd}——涉及该工具的任务优先考虑 {cmd} 相关方案"),
            (0.55 + (total.min(30) as f64 / 30.0) * 0.25).min(0.8),
            total as u32,
            now,
        );
        entry.evidence = vec![format!("count={total}"), format!("sessions={spread}")];
        out.push(entry);
    }
    out
}

#[cfg(test)]
mod tests;
