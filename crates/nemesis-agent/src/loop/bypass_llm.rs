//! 杂务旁路 LLM 调用（prompt-pack pro M4）。
//!
//! 标题生成（E7）、前情摘要（G1/T4/multipart）、Forge 评审与反思等辅助调用
//! 共用的统一护栏：此前这些调用点各自裸调 `provider.chat`——无 token 上限
//! （走 provider 默认 8192）、无墙钟超时（慢模型/挂死连接会拖住后台任务）、
//! 空输出各自解读。本模块把「限 token + 超时 + 输出校验 + 遥测」收敛为单一
//! 真相源，消费方只保留各自的失败回退语义（标题→None 跳过；摘要→None 保持
//! 历史不折叠；Forge 评审→启发式/默认分）。
//!
//! 安全语义不在此处：guardian LLM 审计是安全判定点（fail-closed），其无上下
//! 文宪法、只升格消费与预筛词表独立演化，不迁入本模块。
use std::time::Duration;

/// 标题生成的输出 token 上限（≤24 字标题，64 token 余量充足）。
pub const AUX_TITLE_MAX_TOKENS: u32 = 64;

/// 前情摘要的输出 token 上限（结构化摘要；2026-09-28 真模型验证从 2048 上调
/// 至 8192：thinking 系模型（GLM anthropic 兼容端点默认开思考）会把思考 token
/// 一并计入 max_tokens，2048 被思考烧完后 text 为空 → 摘要「无有效内容」静默
/// 失败。8192 对非思考模型封顶失控长输出、对思考模型留足正文余量）。
pub const AUX_SUMMARY_MAX_TOKENS: u32 = 8192;

/// 标题生成墙钟上限（小模型短输出，60s 足够；超时 = 本轮放弃，下轮再试）。
pub const AUX_TITLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 前情摘要墙钟上限（大历史 + 慢模型的兜底上限；再往上就该让 compact 失败
/// 保持历史不折叠，而不是挂住后台任务）。
pub const AUX_SUMMARY_TIMEOUT: Duration = Duration::from_secs(300);

/// Forge 评审/反思/草稿生成墙钟上限（LLMCaller 桥共用；评审与反思都是
/// 千 token 级短输出）。
pub const AUX_FORGE_TIMEOUT: Duration = Duration::from_secs(180);

/// 杂务调用的 ChatOptions 组装单点：只钳 `max_tokens`，采样参数不设值
/// （留 adapter 默认，与既有线上行为一致）。
pub fn aux_chat_options(max_tokens: u32) -> crate::types::ChatOptions {
    crate::types::ChatOptions {
        max_tokens: Some(max_tokens),
        ..Default::default()
    }
}

/// 杂务旁路调用的统一护栏：墙钟超时 + 空输出校验 + 时长遥测。
///
/// 对任意「最终产出文本」的 LLM 调用未来生效（`Result<String, String>`）；
/// 返回 `Err` 的情形：上游报错原样透传、超时、空/纯空白输出（按失败处理，
/// 不让空串流进下游当成功结果）。成功路径打 debug 级时长日志，失败路径
/// warn（带标签，便于按消费点归因）。
pub async fn guarded_llm_call<F>(
    label: &str,
    timeout: Duration,
    llm_call: F,
) -> Result<String, String>
where
    F: std::future::Future<Output = Result<String, String>>,
{
    let start = std::time::Instant::now();
    match tokio::time::timeout(timeout, llm_call).await {
        Ok(Ok(content)) if !content.trim().is_empty() => {
            tracing::debug!(
                "[bypass:{}] 完成（{}ms，{} 字符）",
                label,
                start.elapsed().as_millis(),
                content.len()
            );
            Ok(content)
        }
        Ok(Ok(_)) => {
            tracing::warn!("[bypass:{}] 模型返回空输出，按失败处理", label);
            Err(format!("[bypass:{}] 空输出，按失败处理", label))
        }
        Ok(Err(e)) => {
            tracing::warn!(
                "[bypass:{}] LLM 调用失败（{}ms）：{}",
                label,
                start.elapsed().as_millis(),
                e
            );
            Err(e)
        }
        Err(_) => {
            tracing::warn!(
                "[bypass:{}] LLM 调用超时（上限 {}s）",
                label,
                timeout.as_secs()
            );
            Err(format!(
                "[bypass:{}] 超时（上限 {}s）",
                label,
                timeout.as_secs()
            ))
        }
    }
}
