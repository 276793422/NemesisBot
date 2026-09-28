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

/// 标题生成的输出 token 上限。
///
/// 2026-09-28 从 64 上调至 8192（用户裁决：预算是防失控上限不是支出，全仓
/// 杂务文本任务统一 8192）：64 是非思考时代遗产，thinking 系模型（GLM
/// anthropic 兼容端点默认开思考）把思考 token 一并计入 max_tokens，思考先
/// 烧穿 64 → 正文为空 → 真机 5/5 空输出（标题功能等于关闭）。8192 对非思考
/// 模型封顶失控长输出、对思考模型留足正文余量；标题清洗仍钳 24 字，超量
/// 零成本。`AUX_TITLE_TIMEOUT` 与重试护栏保持独立兜底。
pub const AUX_TITLE_MAX_TOKENS: u32 = 8192;

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

/// 杂务调用的 ChatOptions 组装单点：钳 `max_tokens` + **显式禁思考**
/// （`reasoning_effort: Some("off")`，anthropic lane 映射为
/// `thinking:{type:"disabled"}`；openai-compat lane 过滤不透传）——杂务
/// 调用（标题/摘要/分支摘要/Forge 桥）不需要思考，思考只会烧穿预算。仅
/// 作用于按此函数组装 options 的杂务请求，主循环 effort（low/medium/high/
/// None）不受影响。采样参数不设值（留 adapter 默认，与既有线上行为一致）。
pub fn aux_chat_options(max_tokens: u32) -> crate::types::ChatOptions {
    crate::types::ChatOptions {
        max_tokens: Some(max_tokens),
        reasoning_effort: Some("off".to_string()),
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

/// 单次重试原语（类型无关）：工厂闭包 `make_call` 每次调用产出一个新
/// future——重试必须重建请求（future 不可重复 poll），因此收闭包不收
/// future。首个 `Ok` 直接返回；`Err` 则 warn 后重建再试一次，两次全败
/// 返回第二次的 `Err`。采样抖动型失败（空输出/瞬态上游错误）由这一层
/// 兜底；外层护栏的墙钟预算不变（重试在超时窗口内进行，不翻倍最坏延迟）。
pub async fn with_one_retry<T, F, Fut>(label: &str, mut make_call: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    match make_call().await {
        Ok(v) => Ok(v),
        Err(first_err) => {
            tracing::warn!("[bypass:{}] 首次调用失败，重试一次：{}", label, first_err);
            make_call().await
        }
    }
}

/// [`guarded_llm_call`] 的重试版：空输出/瞬态错误重试一次，仍失败才上抛。
///
/// 关键：每次尝试**先**过「空输出=失败」归一再进 [`with_one_retry`]——
/// 否则空输出以 `Ok("")` 形态穿过重试原语（它只对 `Err` 重试），护栏只在
/// 最终结果上校验，重试对空输出（thinking 系模型烧穿的主要失败形态）永远
/// 不触发。超时仍包在重试外层（总墙钟预算 = timeout，不随重试放大）；
/// 最终结果再过一次 [`guarded_llm_call`] 的完整校验（幂等，兜住归一外的
/// 形态）。
pub async fn guarded_llm_call_retrying<F, Fut>(
    label: &str,
    timeout: Duration,
    mut make_call: F,
) -> Result<String, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    // 归一在闭包体内完成（future 只持有已产出的 Fut，不借环境——保住
    // FnMut 的可重复调用性）。
    let attempt = || {
        let r = make_call();
        async move {
            match r.await {
                Ok(c) if c.trim().is_empty() => {
                    tracing::warn!("[bypass:{label}] 模型返回空输出，按失败处理");
                    Err(format!("[bypass:{label}] 空输出，按失败处理"))
                }
                other => other,
            }
        }
    };
    guarded_llm_call(label, timeout, with_one_retry(label, attempt)).await
}
