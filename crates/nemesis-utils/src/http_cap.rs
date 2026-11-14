//! HTTP 响应体流式累计闸（BUG-20261101-273 哲学的 workspace 共享单一真相源）。
//!
//! 「承诺了响应体上限，却在 `bytes()` 全量读入内存**之后**才核对」= 核对
//! 发生之前，恶意/失控源已经把任意大 body 灌进内存——上限只约束落盘/返回，
//! 不约束读入过程本身。BUG-20261101-273 在 nemesis-skills 的 clawhub ZIP
//! 下载侧修复过此形态（本地 `read_body_capped`），但同一 crate 的
//! github_tree / github_registry 与全仓 web_fetch / 价目表同步仍是旧形态
//! （BUG-20261107-005 横扫收口）。本模块把「chunk 分块拉取 + 累计超限在
//! 读入中途即断」提升为共享实现，各下载数据面换用同一份，杜绝
//! bytes()-后置-检查手写形态回归。
//!
//! 哲学对齐 [`crate::pipe_cap`]（子进程 stdout/stderr 有界排干）：拒绝
//! 发生在读入过程本身，而不是读入完成之后。

/// 累计判定核心（与 [`read_body_capped`] 读循环内的三行判定同形；独立成
/// 纯函数是为了离线钉死「超限即停、不消费后续块」语义——计数迭代器测试
/// 在「读完全量才查」的旧形态下必红）。
pub fn accumulate_capped<'a, I>(chunks: I, cap: u64) -> Result<Vec<u8>, String>
where
    I: Iterator<Item = &'a [u8]>,
{
    let mut out: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    for chunk in chunks {
        total = total.saturating_add(chunk.len() as u64);
        if total > cap {
            return Err(format!(
                "response exceeds limit (>{cap} bytes); aborted mid-read"
            ));
        }
        out.extend_from_slice(chunk);
    }
    Ok(out)
}

/// 流式读响应体，累计超 `cap` **中途断开**：超限判定发生在每个 chunk 到达
/// 时，Err 返回即 drop `response`（连接关闭，后续数据不再拉取）——恶意源
/// 的巨大 body 在撑内存前被拒，内存峰值 = 已到达的前缀。
///
/// 错误返回 `String`（本模块不依赖 nemesis-types），调用方按各自错误
/// 体系 map。消息形态含 `exceeds limit`（273 既有测试断言的子串）。
pub async fn read_body_capped(
    mut response: reqwest::Response,
    cap: u64,
) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?
    {
        total = total.saturating_add(chunk.len() as u64);
        if total > cap {
            return Err(format!(
                "response exceeds limit (>{cap} bytes); aborted mid-read"
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// 错误响应体有帽读取（BUG-20261113-164 单一真相源）。
///
/// 错误体只喂错误消息摘要（`from_status` 取 200 字符）与日志——全量读入
/// 毫无收益，却让失控/恶意源的巨大错误体（GB 级调试转储、中转网关回显
/// 请求体）在 `text()` 里灌满进程内存（reqwest 无默认体上限；600s 超时
/// 在快管路上不约束体积）。帽取 MCP 先例同款 256KB
///（`nemesis-mcp` http_transport `MAX_ERROR_BODY_READ_BYTES`）。
///
/// 语义与 [`read_body_capped` 的「超限即 hard error」]刻意不同：错误路径
/// 状态码已定类，截断信息够用——**超帽截断加注记**（不是 Err），读中断
/// 返回已积累前缀（调用方旧 `unwrap_or_default` 的「失败=空串」语义保留
/// 为零积累下位形态，信息严格不减）。成功路径响应本体**不得**用本函数
/// （图像 b64 / API JSON 是合法大载荷，须全量读）。
pub async fn read_error_body(mut response: reqwest::Response) -> String {
    const ERROR_BODY_READ_CAP: usize = 256 * 1024;
    let mut bytes: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        let chunk = match response.chunk().await {
            Ok(c) => c,
            // 读中断（连接断/超时）：返回已积累前缀，不伪装成完整错误体。
            Err(_) => break,
        };
        let Some(chunk) = chunk else { break };
        let room = ERROR_BODY_READ_CAP - bytes.len();
        if chunk.len() >= room {
            bytes.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut s = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        s.push_str("\n…[error body truncated]");
    }
    s
}

#[cfg(test)]
mod tests;
