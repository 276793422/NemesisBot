//! WS9/P18：分支摘要（branch summary）——fork/rewind 把会话后缀遗弃时，
//! 用小模型通道（`agents.small_model`，杂务槽位）对**被遗弃部分**生成一份
//! 结构化前情提要，落盘到新会话 sidecar meta（`branch_summary` 字段），
//! 新会话首轮 build_messages 注入为临时 system 节（`# Branch Context`），
//! 让新分支「知道被分出去之前发生了什么、为什么」。
//!
//! 设计要点（与 compaction 摘要同构但语义不同）：
//! - **复用六节 schema**（Goal / Constraints / Progress / Decisions / Files
//!   / Next Steps）与 [`super::parse_structured_summary`] 解析——schema
//!   单一真相源在 compact 域，这里只消费不复制；
//! - **只走 small_model 槽位**（与 E7 标题生成同通道）：未配置 = 诚实跳过
//!   （INFO 日志 + 不落盘），**绝不回退主模型**——分支摘要属杂务，不该
//!   消耗大模型预算，也不该因摘要失败阻塞 fork/rewind 本体；
//! - **阈值闸**：遗弃 <3 个完整 user 轮不生成（没什么可摘要的）；
//! - **两段式 prepare/run**：`prepare_*` 在调用方帧内从 AgentLoop 提取
//!   provider/model/prompt（纯 owned 数据）→ `run()` 自含异步执行——
//!   `tokio::spawn` 只能拿 `'static`，不能把 `&AgentLoop` 带进去；
//! - **预算双钳**：转写截断 [`BRANCH_SUMMARY_TRANSCRIPT_MAX_CHARS`]、
//!   摘要截断 [`chat_log::BRANCH_SUMMARY_MAX_CHARS`]（写入侧还有一次
//!   防御性钳制，两边共用同一常量）。

use super::AgentLoop;
// LlmMessage/LlmProvider 真相源在 crate::r#loop::llm_types（loop.rs glob
// 再导出）——不在 crate::types。
use crate::r#loop::{LlmMessage, LlmProvider};
use serde_json::Value;

/// 触发阈值：遗弃部分不足 3 个完整 user 轮不值得摘要（新分支丢失的上下
/// 文可忽略；生成成本不值）。
pub const BRANCH_SUMMARY_MIN_TURNS: usize = 3;

/// 送模型的遗弃转写硬上限（chars）。超长遗弃段按行截断——摘要只需要
/// 「发生了什么」的轮廓，不需要全量字节。
pub const BRANCH_SUMMARY_TRANSCRIPT_MAX_CHARS: usize = 12000;

/// 单行转写的内容上限（chars）——防止单条超长消息（粘贴/工具输出）独占
/// 整个转写预算。
const TRANSCRIPT_ROW_MAX_CHARS: usize = 600;

/// P18 第一段（同步、调用方帧内）：从 `&AgentLoop` 提取 small_model 槽位
/// 并把遗弃行编成 prompt。返回 `None` 的三种诚实情形：
/// - 遗弃轮数 < [`BRANCH_SUMMARY_MIN_TURNS`]（不值得摘要）；
/// - `agents.small_model` 未配置（杂务槽位缺席，**不回退主模型**）；
/// - 遗弃行全部为空（无可转写内容）。
///
/// `dropped_rows` = 被遗弃的 chat_log jsonl 行（fork: `rows[cut..]`；
/// rewind: 截断前 `removed`），`dropped_turns` = 其中的完整 user 轮数。
/// `pub`：nemesis-web 的 fork 端点复用同一入口（P18 单一生成通道）。
pub fn prepare_branch_summary(
    run_loop: &AgentLoop,
    session_key: &str,
    dropped_rows: &[Value],
    dropped_turns: usize,
) -> Option<PreparedBranchSummary> {
    // 杂务槽位（small_model）：None = 诚实跳过，不回退主模型。
    let Some((provider, model)) = run_loop.small_model_slot() else {
        tracing::info!(
            "[branch_summary] session {session_key} 遗弃 {dropped_turns} 轮，但 agents.small_model 未配置，跳过分支摘要"
        );
        return None;
    };
    prepare_branch_summary_with(provider, model, session_key, dropped_rows, dropped_turns)
}

/// F5（2026-09-27）：CLI 形态的摘要 prepared 构造——调用方**自带**
/// small_model 槽位（provider+model），无 AgentLoop 依赖。CLI `session
/// fork` 用它与 WSAPI fork 对齐（同一生成通道：阈值/转写/空行三闸与
/// prompt 构造都在本函数，单一真相源；`prepare_branch_summary` 只是
/// AgentLoop 槽位提取 + 委托本函数）。
pub fn prepare_branch_summary_with(
    provider: std::sync::Arc<dyn LlmProvider>,
    model: String,
    session_key: &str,
    dropped_rows: &[Value],
    dropped_turns: usize,
) -> Option<PreparedBranchSummary> {
    if dropped_turns < BRANCH_SUMMARY_MIN_TURNS {
        return None;
    }
    let transcript = build_transcript(dropped_rows);
    if transcript.is_empty() {
        return None;
    }
    Some(PreparedBranchSummary {
        provider,
        model,
        prompt: build_branch_summary_prompt(&transcript),
        session_key: session_key.to_string(),
    })
}

/// P18 第二段（异步、`'static`）：执 prepared 请求，解析/截断后返回摘要
/// 文本。LLM 失败 / 空回复 = `None`（调用方诚实跳过，绝不阻塞主流程）。
/// 字段私有——调用方只经 [`prepare_branch_summary`] 构造 + `run`/
/// `spawn_write` 消费（两段式契约防半初始化外泄）。
pub struct PreparedBranchSummary {
    provider: std::sync::Arc<dyn LlmProvider>,
    model: String,
    prompt: String,
    session_key: String,
}

impl PreparedBranchSummary {
    pub async fn run(self) -> Option<String> {
        // 杂务旁路护栏（重试版）：aux 预算 + 显式禁思考 + 墙钟超时 + 空输出
        // 单次重试——与 E7 标题/compact 摘要同一治理面（此前本调用点是裸调
        // provider.chat 的漏网之鱼）。失败/超时/空输出 → None（调用方诚实
        // 跳过，绝不阻塞主流程）。
        let text = crate::r#loop::guarded_llm_call_retrying(
            "branch-summary",
            crate::r#loop::AUX_SUMMARY_TIMEOUT,
            || {
                let prompt = self.prompt.clone();
                let model = self.model.clone();
                let provider = self.provider.clone();
                async move {
                    provider
                        .chat(
                            &model,
                            vec![LlmMessage {
                                role: "user".to_string(),
                                content: prompt,
                                tool_calls: None,
                                tool_call_id: None,
                                reasoning_content: None,
                                images: Vec::new(),
                            }],
                            Some(crate::r#loop::aux_chat_options(
                                crate::r#loop::AUX_SUMMARY_MAX_TOKENS,
                            )),
                            Vec::new(),
                        )
                        .await
                        .map(|r| r.content)
                }
            },
        )
        .await
        .ok()?;
        let text = text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        // 六节 schema 解析（compact 域单一真相源）；解析失败回退自由文本
        // 原样使用——解析失败绝不允许丢摘要。
        let summary = super::parse_structured_summary(&text).unwrap_or(text);
        let summary = summary.trim();
        if summary.is_empty() {
            return None;
        }
        Some(
            summary
                .chars()
                .take(crate::chat_log::BRANCH_SUMMARY_MAX_CHARS)
                .collect(),
        )
    }

    /// 后台落盘：spawn 异步执行 + 写 sidecar meta（fork/rewind 调用方的
    /// 单一入口——两处共享同一段 spawn/写/日志逻辑）。失败/跳过只 INFO/
    /// WARN，绝不传染主流程。
    pub fn spawn_write(self) {
        let key = self.session_key.clone();
        tokio::spawn(async move {
            match self.run().await {
                Some(summary) => {
                    crate::chat_log::write_session_branch_summary(&key, &summary);
                    tracing::info!(
                        "[branch_summary] session {key} 分支摘要已落盘（{} chars）",
                        summary.chars().count()
                    );
                }
                None => {
                    tracing::info!(
                        "[branch_summary] session {key} 分支摘要生成失败/为空，诚实跳过"
                    );
                }
            }
        });
    }
}

/// 遗弃行 → 喂模型的转写文本（`[role] content` 逐行；单行截断 + 总量
/// 截断；空内容行跳过）。
fn build_transcript(rows: &[Value]) -> String {
    let mut out = String::new();
    for row in rows {
        let role = row.get("role").and_then(|r| r.as_str()).unwrap_or("?");
        let content = row
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            continue;
        }
        let clipped: String = content.chars().take(TRANSCRIPT_ROW_MAX_CHARS).collect();
        out.push_str(&format!("[{role}] {clipped}\n"));
        if out.chars().count() >= BRANCH_SUMMARY_TRANSCRIPT_MAX_CHARS {
            out.push_str("…（转写已截断）\n");
            break;
        }
    }
    out.trim().to_string()
}

/// 分支摘要请求的 prompt（六节 schema 与 compaction 摘要同源——标题列表
/// 直接引用 [`super::SUMMARY_SCHEMA_SECTIONS`]（文本真相在 nemesis-prompts
/// aux 模块），防两处漂移）。
fn build_branch_summary_prompt(transcript: &str) -> String {
    let mut p = String::from(
        "以下是一次会话被分叉/回退时被遗弃的后缀对话记录（这些内容不在新分支的上下文里）。\
请为它们生成一份结构化「分支前情提要」，供新分支的后续对话快速了解：被遗弃部分做了什么、得出了什么结论、留下了哪些未完成事项。",
    );
    p.push_str(&crate::prompt::render_summary_schema_suffix());
    p.push_str("\n\n被遗弃对话记录：\n");
    p.push_str(transcript);
    p
}
