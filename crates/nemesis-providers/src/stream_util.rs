//! 流式聚合器（2026-10-13 stream 键实现 Step 3）。
//!
//! per-model `stream` 键（`yes`=强制内部流式）落到 provider `chat()` 后，
//! 复用各 lane **现成**的 `chat_stream`（SSE 解析/tool_calls 累积/usage 提取
//! 全部同源），本模块只做最后一步：把 chunk 流聚合成非流式
//! [`LLMResponse`]，让 `chat()` 对调用方的签名与返回形态零变化
//! （agent loop / forge / workflow / compact / 集群 B 端等 12 处消费点自动受益）。
//!
//! chunk 契约（两条 lane 共同遵守）：文本增量走 `delta`；
//! `tool_calls` / `usage` / `reasoning_content` / `finish_reason` 只在终态
//! chunk 整表出现（流内部已各自累积完毕）——本聚合器对后四者做
//! 「出现即覆盖」，不做增量合并。

use crate::failover::FailoverError;
use crate::http_provider::StreamChunk;
use crate::types::LLMResponse;

/// SSE 增量 UTF-8 解码：把一个网络 chunk 安全并入行缓冲 `buffer`
/// （BUG-HUNT R38）。`pending` 携带上一 chunk 留下的未完成尾随序列。
///
/// 逐 chunk 独立 `from_utf8_lossy` 会把被 chunk 边界劈开的多字节字符
/// （CJK 3 字节 / emoji 4 字节）替换成 U+FFFD——TCP 分片边界是任意的，
/// 长 CJK 回复几乎每个边界都可能烂字。这里分段扫描：尾部**不完整**序列
/// （`error_len()==None`）留待与下一 chunk 拼接后整体解码；真坏字节
/// （`error_len()==Some`）当场 lossy 替换后**从坏段之后继续扫**（同一
/// chunk 里坏字节后面跟着的合法字符/残尾不受牵连）。`pending` 恒 ≤3
/// 字节，有界。
pub fn push_chunk_incremental(buffer: &mut String, pending: &mut Vec<u8>, bytes: &[u8]) {
    let mut chunk = std::mem::take(pending);
    chunk.extend_from_slice(bytes);
    let mut pos = 0usize;
    loop {
        match std::str::from_utf8(&chunk[pos..]) {
            Ok(s) => {
                buffer.push_str(s);
                pos = chunk.len();
                break;
            }
            Err(e) => {
                // valid_up_to()/error_len() 都相对 &chunk[pos..]，v/n 换算回绝对下标。
                let v = pos + e.valid_up_to();
                match e.error_len() {
                    Some(n) => {
                        // 合法前缀 [pos..v) + 坏段 [v..v+n)（→U+FFFD）入缓冲，
                        // 越过坏段继续扫（其后可能还有合法字符/残尾）。
                        buffer.push_str(&String::from_utf8_lossy(&chunk[pos..v + n]));
                        pos = v + n;
                        if pos == chunk.len() {
                            break;
                        }
                    }
                    None => {
                        // 尾部不完整（序列被 chunk 边界劈开）：合法前缀
                        // [pos..v) 入缓冲，残尾 [v..) 留给下一 chunk。
                        buffer.push_str(&String::from_utf8_lossy(&chunk[pos..v]));
                        pos = v;
                        break;
                    }
                }
            }
        }
    }
    *pending = chunk.split_off(pos);
}

/// Consume a lane's `chat_stream` receiver and aggregate into a non-streaming
/// `LLMResponse`.
///
/// 流中途 `Err` → 原样上抛（lane 已做诚实分类）。流断且无终态 chunk
/// （channel 关闭仍无 `finish_reason`，生产者 panic 等异常路径）→
/// `Timeout`（可重试/可故障转移），与 codex lane「流断无终态」既有约定同门。
/// `raw_request_body` / `raw_response_body` 恒 `None`——流式路径没有完整
/// 响应体快照（请求日志器对 None 字段本就省略，非流式路径不受影响）。
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
            Err(e) => return Err(e),
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
        None => Err(FailoverError::Timeout {
            provider: provider.to_string(),
            model: model.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
