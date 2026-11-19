//! 流式聚合器（2026-10-13 stream 键实现 Step 3）。
//!
//! per-model `stream` 键（`yes`=强制内部流式）落到 provider `chat()` 后，
//! 复用各 lane **现成**的 `chat_stream`（SSE 解析/tool_calls 累积/usage 提取
//! 全部同源），本模块只做最后一步：把 chunk 流聚合成非流式
//! [`LLMResponse`]，让 `chat()` 对调用方的签名与返回形态零变化
//! （agent loop / forge / workflow / compact / 集群 B 端等 12 处消费点自动受益）。
//!
//! chunk 契约（两条 lane 共同遵守）：文本增量走 `delta`；
//! `tool_calls` / `usage` / `finish_reason` 只在终态 chunk 整表出现（流内部
//! 已各自累积完毕）——本聚合器对后三者做「出现即覆盖」，不做增量合并。
//! `reasoning_content` 自 B3（2026-11-09）起有**两种形态**：终态 chunk 带
//! 全文（既有契约，覆盖兜底）+ fragment chunk（delta 空 + finish None +
//! reasoning Some = 新增片段，思考增量放行）。片段在本聚合器里被「出现即
//! 覆盖」暂收，终态全文到达即覆盖为权威全文——最终结果与片段流之前完全
//! 一致（片段只是路过，不参与拼接）。

use crate::failover::FailoverError;
use crate::http_provider::StreamChunk;
use crate::types::LLMResponse;

/// SSE 增量 UTF-8 解码（2026-11-09 下沉单一真相源到 `nemesis_utils::sse`：
/// nemesis-mcp HTTP/SSE 传输同族缺陷 BUG-20261109-007 一并修复，三 lane
/// 经此路径消费；实现与文档见 nemesis-utils）。
pub use nemesis_utils::sse::push_chunk_incremental;

/// Consume a lane's `chat_stream` receiver and aggregate into a non-streaming
/// `LLMResponse`.
///
/// 流中途 `Err` / channel 关闭无终态（生产者 panic 等异常路径）时：
/// 已聚合文本为空 → 维持旧语义（原错误原样上抛 / `Timeout`——lane 已做
/// 诚实分类，错误值不吞）；**非空 → `StreamInterrupted`**（partial 是模型
/// 已产出的一等工作成果，随错误值携带不丢——，消费方是 recovery 重试
/// 环与终局通知）。tool_calls 出现过由 `has_partial_tool_calls` 旗标声明
/// （半截不可当纯文本续用）。`raw_request_body` / `raw_response_body` 恒
/// `None`——流式路径没有完整响应体快照（请求日志器对 None 字段本就省略，
/// 非流式路径不受影响）。
///
/// **终态唯一性契约（BUG-20261120-003）**：终态 chunk（finish_reason Some）
/// 已聚合后再到的 Err 是终态后传输噪音（http lane 的 SSE 读错误臂 / 墙钟
/// deadline 臂在 finish_emitted 后仍可发射——RST-after-FIN keep-alive 竞态、
/// 挂死代理不发 `[DONE]` 撞墙钟），一律忽略并按正常完成收尾。wire 语义下
/// finish_reason 即响应完成；不闸会把已完成响应改写成 `StreamInterrupted`
/// → 拼接续写把完整回复当半截去续写（缝合怪终答）+ 白烧一轮重试。同 lane
/// BUG-20261114-030 已把流内 error 事件闸在终态后帧闸之后，传输层错误同契约。
pub async fn collect_stream_response(
    provider: &str,
    model: &str,
    mut rx: tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>>,
) -> Result<LLMResponse, FailoverError> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut usage = None;
    let mut reasoning_content = None;
    let mut finish_reason: Option<String> = None;

    while let Some(item) = rx.recv().await {
        match item {
            Ok(chunk) => {
                content.push_str(&chunk.delta);
                if !chunk.tool_calls.is_empty() {
                    tool_calls = chunk.tool_calls;
                }
                if chunk.usage.is_some() {
                    usage = chunk.usage;
                }
                if chunk.reasoning_content.is_some() {
                    reasoning_content = chunk.reasoning_content;
                }
                if chunk.finish_reason.is_some() {
                    finish_reason = chunk.finish_reason;
                }
            }
            Err(e) => {
                // 终态唯一性契约（BUG-20261120-003）：终态 chunk 已聚合后再
                // 到的 Err 是终态后传输噪音（http lane SSE 读错误臂 / deadline
                // 臂在 finish_emitted 后仍可发射），一律忽略——wire 语义下
                // finish_reason 即响应完成，改写 StreamInterrupted 会触发
                // 拼接续写把已完成回复当半截去续写。break 走下方正常收尾
                //（finish_reason 在场 → Ok）。闸位在空判定之前：终态后的
                // 合法空补全不走「空 → 原错误上抛」臂。
                if finish_reason.is_some() {
                    tracing::debug!(
                        provider = %provider,
                        model = %model,
                        error = %e,
                        "[stream_util] post-terminal stream error ignored — finish_reason already aggregated"
                    );
                    break;
                }
                // 已聚合文本/工具调用非空 → partial 随错误携带不丢；
                // 空 → lane 原错误原样上抛（诚实分类不吞）。
                if content.trim().is_empty() && tool_calls.is_empty() {
                    return Err(e);
                }
                // LLM-ERR-B（2026-11-17 治本批）：确定性应用层错误（限流/
                // 认证/计费）**透传不包 StreamInterrupted**——上游 error 事件
                // 明确宣告本次请求失败，partial 是被拒请求的半成品不是可续写
                // 工作成果。包 StreamInterrupted 会：(a) 把分类拍扁进 reason
                // 字符串（Display 前缀短路恒判 Timeout → 限流信号被当传输
                // 抖动零退避立即重试，429 风暴形态，实测基线复现）；(b) 触发
                // 拼接续写把半截文本缝进重试后的完整回复（实测缝合怪终答）。
                // 透传保住变体语义：loop 侧 classified/词表双轨都按 RateLimit
                // 进限流退避环（或 Auth/Billing 诚实终局），重试成功后终答
                // 干净。Timeout/Overloaded/Format/Unknown 等维持既有
                // StreamInterrupted 携带契约（传输中断的 partial 是真成果）。
                if matches!(
                    e,
                    FailoverError::RateLimit { .. }
                        | FailoverError::Auth { .. }
                        | FailoverError::Billing { .. }
                ) {
                    return Err(e);
                }
                return Err(FailoverError::StreamInterrupted {
                    provider: provider.to_string(),
                    model: model.to_string(),
                    reason: e.to_string(),
                    partial_content: content,
                    has_partial_tool_calls: !tool_calls.is_empty(),
                });
            }
        }
    }

    match finish_reason {
        Some(finish_reason) => Ok(LLMResponse {
            content,
            tool_calls,
            finish_reason,
            usage,
            reasoning_content,
            extra: Default::default(),
            raw_request_body: None,
            raw_response_body: None,
        }),
        // 流断无终态（codex lane 既有约定同门）：空 → Timeout（可重试/可
        // 故障转移）；非空 → StreamInterrupted 携带 partial。
        None => {
            if content.trim().is_empty() && tool_calls.is_empty() {
                return Err(FailoverError::Timeout {
                    provider: provider.to_string(),
                    model: model.to_string(),
                });
            }
            Err(FailoverError::StreamInterrupted {
                provider: provider.to_string(),
                model: model.to_string(),
                reason: "stream closed without finish_reason".to_string(),
                partial_content: content,
                has_partial_tool_calls: !tool_calls.is_empty(),
            })
        }
    }
}

/// SSE 事件边界定位单一真相源在 `nemesis_utils::sse`（2026-11-08 下沉：
/// nemesis-mcp HTTP/SSE 传输同族缺陷一并修复，三 lane 经此路径消费）。
/// 规范行终止 LF/CR/CRLF 三态逐行独立 + 贪婪 CRLF + 尾随 `\r` 半包等待。
pub use nemesis_utils::sse::next_event_boundary;

#[cfg(test)]
mod tests;

// collect_stream_response 两臂改造回归钉。
#[cfg(test)]
mod collect_tests;
