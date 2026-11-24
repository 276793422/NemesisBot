//! LLM 反思通道（Forge 化重构批二，G1/G2/G3/G7）。
//!
//! 旧形态三重设计缺陷（用户拍板「这是个 BUG，最终结果就是什么都学不到」）：
//! - 采样 60 条×120 字符——「多步仪式」被截成无序列的碎片拼贴，结构性
//!   不可见（G2）；
//! - 触发 20-sweep 闸且计数器内存态——重启归零，频繁重启设备可能从未
//!   反思（G1，计数器已随批一持久化进画像文件，本模块提供触发判定纯函数）；
//! - 提示词无序列意识——即便采到完整链路也不知道要输出步骤（G3）。
//!
//! 新形态：
//! - 输入 = [`super::collection::collect_dialogue_after`] 的**全量结构化行**
//!   （user 500 字摘录 + assistant 工具时间线投影），按会话分组保序；
//! - 提示词带**序列意识**（明确要求识别多步仪式并输出 steps 数组）+
//!   **ledger 上下文**（上次反思结论回灌——知识复利：不重复、可细化）；
//! - 结论落 ledger（`<ws>/forge/userprofile/reflections.jsonl`），负反馈
//!   事件（veto/rewind）由 surface/rewind 臂另行落 events.jsonl 进 prompt
//!   的负样本注记面（G7 证据面从「无」到「有」）。

use std::path::Path;

use super::bridge::LessonEntry;
use super::collection::DialogueRow;
use super::store::{HabitEntry, KIND_HABIT, KIND_LESSON, MAX_EVIDENCE};

/// 反思触发：sweep 计数闸缺省值（config `user_profile.reflect_every_sweeps`
/// 可调）。
pub const REFLECT_EVERY_SWEEPS_DEFAULT: u32 = 20;
/// 反思触发：窗内新增用户消息数闸缺省值（config
/// `user_profile.reflect_min_new_messages` 可调；G1 主触发——活跃用户
/// 两天就能攒够 25 条新消息，不必等 20 个小时 tick）。
pub const REFLECT_MIN_NEW_MESSAGES_DEFAULT: u32 = 25;
/// 反思摘要总预算（字符）。超出丢最旧会话并加诚实注记。
const DIGEST_MAX_CHARS: usize = 24000;
/// user 消息摘录长度。
const USER_EXCERPT_CHARS: usize = 500;
/// assistant 消息摘录长度（assistant 输出长，投影更短）。
const ASSISTANT_EXCERPT_CHARS: usize = 200;
/// 反思候选类别白名单（含扩容位 environment/quality_bar）。
pub const REFLECT_CATEGORIES: &[&str] = &[
    "workflow",
    "correction",
    "tool_preference",
    "style",
    "environment",
    "quality_bar",
];

/// 反思触发判定（纯函数；G1 触发重构）：
/// - sweep 计数到顶（持久化计数器 [`super::store::UserProfile::sweeps_since_reflect`]）
///   或 窗内新增用户消息 ≥ `min_new` → 触发；
/// - `reflect_every` 归一 ≥1（除零防御）；`min_new` 由 config 访问器
///   归一（0 = 回落缺省 25）。
pub fn should_reflect(
    sweeps_since_reflect: u32,
    reflect_every: u32,
    new_user_msgs: usize,
    min_new: u32,
) -> bool {
    let every = reflect_every.max(1);
    if sweeps_since_reflect >= every {
        return true;
    }
    if min_new > 0 && new_user_msgs >= min_new as usize {
        return true;
    }
    false
}

/// 反思输入摘要（G2 根修）：按会话分组、组内时间保序——user 行 500 字
/// 摘录，assistant 行 200 字 + 工具时间线投影（`tools: a, b`）。总预算
/// [`DIGEST_MAX_CHARS`]：从最新会话往回装填，放不下的更早会话整体丢弃并
/// 在头部加诚实注记（不让 LLM 在无声截断的碎片上归纳）。
pub fn build_digest(rows: &[DialogueRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    // 按会话分组（首次出现序 = 时间序；rows 时间升序由采集层保证）。
    let mut order: Vec<&str> = Vec::new();
    let mut groups: std::collections::HashMap<&str, Vec<&DialogueRow>> = Default::default();
    for r in rows {
        if !groups.contains_key(r.session.as_str()) {
            order.push(&r.session);
        }
        groups.entry(r.session.as_str()).or_default().push(r);
    }
    // 从最新会话往回装填预算。
    let mut kept_rev: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut dropped = 0usize;
    for session in order.iter().rev() {
        let block = render_session(session, groups.get(session).expect("grouped above"));
        if used + block.len() > DIGEST_MAX_CHARS && !kept_rev.is_empty() {
            dropped = order.len() - kept_rev.len();
            break;
        }
        used += block.len();
        kept_rev.push(block);
    }
    if dropped == 0 && kept_rev.len() < order.len() {
        // 单块超预算且是唯一块：整体保留（诚实展示畸形会话，不静默丢）。
        dropped = order.len() - kept_rev.len();
    }
    let mut out = String::new();
    if dropped > 0 {
        out.push_str(&format!("（更早 {dropped} 个会话超出摘要预算，已截去）\n"));
    }
    for block in kept_rev.iter().rev() {
        out.push_str(block);
    }
    out
}

/// 单会话块渲染。
///
/// 批 C ② 完整化：assistant 行投影补三面——**文件种类**（`file_changes`/
/// `deliverables` 扩展名分类，工作域读法素材）、**中间轮叙述**（work 时间线
/// text 缩进行，模型「怎么干的」过程读法素材）、**模型名**（切换行为读法
/// 素材）；user 行带**来源通道**标记（IM vs Dashboard 风格读法素材）。
/// 全部缺省跳过（空 = 不加标签，摘要紧凑优先）。
fn render_session(session: &str, rows: &[&DialogueRow]) -> String {
    let mut out = format!("### 会话 {session}\n");
    for (i, r) in rows.iter().enumerate() {
        if r.role == "user" {
            let excerpt: String = r.content.chars().take(USER_EXCERPT_CHARS).collect();
            let channel = if r.channel.is_empty() {
                String::new()
            } else {
                format!("@{}", r.channel)
            };
            out.push_str(&format!("{}. [用户{channel}] {excerpt}\n", i + 1));
        } else {
            let excerpt: String = r.content.chars().take(ASSISTANT_EXCERPT_CHARS).collect();
            let mut tags = String::new();
            if !r.work_tools.is_empty() {
                tags.push_str(&format!(" | 工具: {}", r.work_tools.join(", ")));
            }
            if !r.file_kinds.is_empty() {
                tags.push_str(&format!(" | 文件: {}", r.file_kinds.join(",")));
            }
            if !r.model.is_empty() {
                tags.push_str(&format!(" | 模型: {}", r.model));
            }
            out.push_str(&format!("{}. [助手{tags}] {excerpt}\n", i + 1));
            for note in &r.work_notes {
                out.push_str(&format!("    · {note}\n"));
            }
        }
    }
    out
}

/// 反思通道：LLM 按主语分型归纳 →（习惯候选，教训候选）二元组。
/// **反思通道产物 auto=false**，与挖掘器固定产物分桶（G8）。
/// `prior_conclusions` = ledger 尾部结论（知识复利上下文）；`threshold` =
/// 置信度闸（config）。
///
/// 提示词四要素：① **主语分型**（批 A——同一信号两种读法：主语=用户的
/// 稳定偏好归 `habit`（画像注入面），主语=助手自身的失误/低效/系统性
/// 问题归 `lesson`（分流 bridge 队列给 Forge 自学习，不丢弃也不伪装成
/// 用户习惯）；② 序列意识（明确要求识别**跨消息的多步仪式**，输出
/// steps 数组，G3）；③ 负样本注记（veto/forget/rewind 事件摘要——被否决
/// 的归纳不再提）；④ ledger 回灌（已有结论不重复，可细化补充步骤）。
pub fn reflect_semantic_habits(
    caller: &std::sync::Arc<dyn crate::reflector_llm::LLMCaller>,
    rows: &[DialogueRow],
    prior_conclusions: &[String],
    negative_events: &[serde_json::Value],
    rt: &tokio::runtime::Handle,
    threshold: f64,
) -> Result<(Vec<HabitEntry>, Vec<LessonEntry>), String> {
    let digest = build_digest(rows);
    if digest.trim().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let now = chrono::Local::now().to_rfc3339();
    let mut prompt = String::from(
        "以下是某用户近期与 AI 助手的对话记录（按会话与时间顺序排列）。请按**主语**分两类归纳：\n\n\
         1. kind=\"habit\"：该**用户**稳定的偏好/习惯（工作流形态、风格、工具偏好、环境、质量标准）\
         ——用于让助手更懂这位用户、配合得更顺手；\n\
         2. kind=\"lesson\"：**助手自身**的失误/低效/系统性问题（例如工具用法反复出错、流程走了弯路、\
         交付形态不合需求被返工）——这些是给助手改进用的教训，不是用户的习惯。\n\n\
         判定方法：这条结论的主语是用户还是助手？用户怎么做事 → habit；助手哪里做得不好 → lesson。\
         两类都输出，不要互相伪装（不要把助手的失误写成用户的偏好，也不要把用户的习惯写成助手的教训）。\n\
         两类都只限风格与流程层面，绝不包含权限、审批、安全相关语义。\n\n\
         注意：\n\
         - 跨多条消息的**多步流程仪式**（例如「先做计划→反复确认→转成步骤清单→测试收尾」）请识别为 workflow 条目，\
         并把每个阶段写进 steps 数组（按执行顺序，每步一句话）；\n\
         - 协作节奏维度（与确定性挖掘同向覆盖，谁先有证据谁上榜）：**开场形态**\
         （用户是开场即派活还是寒暄铺垫）、**催促模式**（等待中会不会复述/催促）、\
         **分工风格**（微观管理型高频插细节指令 vs 派单型派后沉默）、**返工节奏**\
         （迭代型先快出再看 vs 一次成型型先对齐方案）；有跨会话证据时输出为条目；\n\
         - 只在有充分证据时输出；单次偶然行为不算；\n\
         - content 必须一句话（不含换行）。",
    );
    if !prior_conclusions.is_empty() {
        prompt.push_str(
            "\n\n此前已归纳的结论（不要原样重复；如本轮证据支持，可给出 steps 细化版本）：",
        );
        for c in prior_conclusions {
            prompt.push_str(&format!("\n- {c}"));
        }
    }
    if !negative_events.is_empty() {
        prompt.push_str("\n\n用户曾否决/回退以下归纳（不要再次提出同类结论）：");
        for ev in negative_events {
            let kind = ev.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
            let content = ev.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if content.is_empty() {
                prompt.push_str(&format!("\n- [{kind}]"));
            } else {
                prompt.push_str(&format!("\n- [{kind}] {content}"));
            }
        }
    }
    prompt.push_str(&format!(
        "\n\n只输出 JSON 数组，每项 {{\"kind\": \"habit\"|\"lesson\", \"category\": \"{}\", \
         \"content\": \"<一句话陈述>\", \"confidence\": <0.0-1.0>, \
         \"steps\": [\"步骤1\", \"步骤2\"]}}（kind 缺省按 habit；steps 可省略）。\
         无稳定结论则输出 []。\n\n对话记录：\n{digest}",
        REFLECT_CATEGORIES.join("|")
    ));
    let resp =
        rt.block_on(caller.chat("你是用户行为分析师。只输出 JSON。", &prompt, Some(2048)))?;
    parse_reflection_response(&resp, &now, threshold)
}

/// 反思响应提取（数组感知）：[`crate::reflector_llm::extract_json`] 只认
/// `{...}` 对象段（first `{` → last `}`），而反思响应按提示词约定是**数组**
/// 形态——对象段提取会把数组切成元素片段永远解析失败。这里先做 `[...]`
/// 数组段提取（首 `[` → 尾 `]`），失败回落单对象（模型偶尔只回一条结论时
/// 包成单元素数组；对象须带 `category` 才认——裸 JSON 对象误回不收）。
fn extract_entry_values(resp: &str) -> Option<Vec<serde_json::Value>> {
    let trimmed = resp.trim();
    if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.rfind(']'))
        && end > start
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end])
        && let Some(arr) = v.as_array()
    {
        return Some(arr.clone());
    }
    let obj = crate::reflector_llm::extract_json(resp)?;
    if obj.get("category").is_some() {
        Some(vec![obj])
    } else {
        None
    }
}

/// 反思响应解析（纯函数可单测）。纪律沿旧实现：类别白名单（扩容位
/// environment/quality_bar）、换行拒绝、长度截断、置信度 clamp 后过闸；
/// steps 数组清洗（[`HabitEntry::sanitize_steps`]）。
/// 批 A：`kind` 分型解析——缺省/未知值回落 habit（旧响应零迁移）；
/// `lesson` 项构造 [`LessonEntry`]（confidence 同闸、同白名单、evidence
/// 解析截断），与 habit 项分流返回。
pub fn parse_reflection_response(
    resp: &str,
    now: &str,
    threshold: f64,
) -> Result<(Vec<HabitEntry>, Vec<LessonEntry>), String> {
    let arr = extract_entry_values(resp).ok_or("reflect response is not valid JSON")?;
    let mut habits = Vec::new();
    let mut lessons = Vec::new();
    for item in arr {
        let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("style");
        let confidence = item
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let kind = item
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or(KIND_HABIT);
        let content = content.trim();
        if content.is_empty() || content.contains('\n') || content.contains('\r') {
            continue; // 空陈述 / 夹带换行（会伪造注入行结构）= 整条跳过
        }
        if !REFLECT_CATEGORIES.contains(&category) {
            continue;
        }
        // 先钳制再过阈值：>1.0 的候选经 clamp 后仍可参与比较。
        let confidence = confidence.clamp(0.0, 1.0);
        if confidence < threshold {
            continue;
        }
        let steps: Vec<String> = item
            .get("steps")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let evidence: Vec<String> = item
            .get("evidence")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        if kind == KIND_LESSON {
            lessons.push(LessonEntry {
                id: String::new(), // bridge::append_lessons 入队时分配
                ts_ms: chrono::Utc::now().timestamp_millis(),
                kind: KIND_LESSON.into(),
                category: category.to_string(),
                content: content.chars().take(200).collect(),
                confidence,
                evidence: {
                    let mut e = evidence;
                    e.truncate(MAX_EVIDENCE);
                    e
                },
                session_count: 0, // 反思单批归纳无精确会话计数，诚实填 0
                status: super::bridge::LESSON_STATUS_PENDING.into(),
            });
            continue;
        }
        let mut entry = HabitEntry {
            category: category.to_string(),
            // 单行约束已由换行拒绝保证；长度截 200 字符防 LLM 长篇。
            content: content.chars().take(200).collect(),
            confidence,
            hits: 1,
            first_seen: now.to_string(),
            last_seen: now.to_string(),
            status: super::store::STATUS_ACTIVE.into(),
            pinned: false,
            steps: HabitEntry::sanitize_steps(&steps),
            scope: String::new(),
            evidence: Vec::new(),
            auto: false,
            kind: KIND_HABIT.into(),
            source: String::new(),
        };
        entry.evidence.truncate(MAX_EVIDENCE);
        habits.push(entry);
    }
    Ok((habits, lessons))
}

// ---------- ledger（反思结论持久化，知识复利面） ----------

/// ledger 路径（`<ws>/forge/userprofile/reflections.jsonl`）。
fn ledger_path(workspace: &Path) -> std::path::PathBuf {
    workspace
        .join("forge")
        .join("userprofile")
        .join("reflections.jsonl")
}

/// 追加一轮反思记录（`{ts_ms, conclusions}`；失败静默——学习侧旁路数据）。
pub fn append_ledger(workspace: &Path, conclusions: &[String]) {
    if conclusions.is_empty() {
        return;
    }
    let path = ledger_path(workspace);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let record = serde_json::json!({
        "ts_ms": chrono::Utc::now().timestamp_millis(),
        "conclusions": conclusions,
    });
    if let Ok(line) = serde_json::to_string(&record)
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// 读最近 `limit` 条结论（时间升序尾部取回；反思 prompt 的回灌上下文）。
/// 缺失/损坏 = 空 vec。
pub fn read_recent_conclusions(workspace: &Path, limit: usize) -> Vec<String> {
    let path = ledger_path(workspace);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(arr) = v.get("conclusions").and_then(|c| c.as_array()) {
            for c in arr.iter().filter_map(|c| c.as_str()) {
                out.push(c.to_string());
            }
        }
    }
    let start = out.len().saturating_sub(limit);
    out.split_off(start)
}

#[cfg(test)]
mod tests;
