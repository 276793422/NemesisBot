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

#[cfg(test)]
mod tests;
