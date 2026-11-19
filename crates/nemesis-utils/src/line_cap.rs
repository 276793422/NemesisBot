//! 带帽单行读（横扫角度 B 续：外部流行读无界入内存——行帧形态）。
//!
//! 「从外部流逐行读」路径原为裸 `AsyncBufReadExt::read_line`：它把整行
//! 无界积累进 buf 才返回，无法设帽。对端（MCP 服务器 / IRC 服务器 /
//! 邮件服务器 / 设备 / 用户脚本）一行无限输出即可打爆宿主内存——对端
//! 崩溃、被替换或恶意均不受我方控制。nemesis-lsp 既有先例
//! （`MAX_LSP_HEADER_LINE_BYTES` 64KB + 注释明示 read_line 缺陷），本模块
//! 是修复后的统一实现：MCP stdio（stdout 响应行 16MB / stderr 排水 64KB）
//! 与 nemesis-channels 四通道（IRC / external / maixcam / email SMTP+IMAP）
//! 同源消费，帽值由调用方按协议语义传入（本模块不设默认帽）。
//!
//! 与 [`crate::pipe_cap`] 的分工：pipe_cap 管「整流排干」形态
//! （丢头保尾、读尽即收），本模块管「行帧」形态（帽内完整行、超帽截断
//! 丢弃剩余但游标对齐下一行行首——后续行不受毒丸行影响）。

/// 带帽单行读：手动 fill_buf 扫描，返回 `None` = EOF 且无字节；`Some`
/// 为 `(行字节含 `\n` 若装得下, 是否截断)`。截断 = 行超 `cap`：前 `cap`
/// 字节返回，剩余字节静默丢弃至行尾/EOF——游标始终对齐下一行行首，
/// 后续行不受毒丸行影响。行字节收齐后一次性 lossy 转换由调用方执行
/// （分块 lossy 会把 chunk 边界劈开的多字节字符换成替换符，静默损坏
/// JSON/文本值——BUG-20261108-027 同族纪律）。
///
/// 取消安全性：与 `read_line` 同级（非 cancel-safe——future 在读中途被
/// drop，已消费的字节丢失、游标停在行中间）。select! 臂消费方需自行
/// 保证取消后的连接处置（既有通道读循环同语义）。
pub async fn read_line_capped<R>(
    reader: &mut R,
    cap: usize,
) -> std::io::Result<Option<(Vec<u8>, bool)>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut raw: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // EOF：积累过字节 = 无换行尾行（read_line 同语义 Ok(n>0)）；
            // 全空 = 纯 EOF。
            return if raw.is_empty() && !truncated {
                Ok(None)
            } else {
                Ok(Some((raw, truncated)))
            };
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                if !truncated && total_would_exceed(raw.len(), pos + 1, cap) {
                    truncated = true;
                }
                if truncated {
                    let keep = cap.saturating_sub(raw.len());
                    raw.extend_from_slice(&available[..keep.min(pos + 1)]);
                    // 丢弃剩余字节直到行尾，游标落在下一行行首。
                    reader.consume(pos + 1);
                    return Ok(Some((raw, true)));
                }
                raw.extend_from_slice(&available[..pos + 1]);
                reader.consume(pos + 1);
                return Ok(Some((raw, false)));
            }
            None => {
                if !truncated && total_would_exceed(raw.len(), available.len(), cap) {
                    truncated = true;
                }
                if truncated {
                    let keep = cap.saturating_sub(raw.len());
                    raw.extend_from_slice(&available[..keep]);
                    let n = available.len();
                    reader.consume(n);
                    // 丢弃模式：消费到行尾/EOF 为止（毒丸行不再进内存）。
                    continue;
                }
                let n = available.len();
                raw.extend_from_slice(available);
                reader.consume(n);
            }
        }
    }
}

fn total_would_exceed(current: usize, incoming: usize, cap: usize) -> bool {
    current.saturating_add(incoming) > cap
}

/// [`read_line_capped`] 的同步孪生（`std::io::BufRead`；sync/async 双形态
/// 先例同 `crate::pipe_cap` 的 `drain_capped_sync`/`drain_capped_async`）。
/// 语义逐条对齐 async 版：
/// - 帽内完整行（含尾部 `\n`）；超帽截断 = 前 `cap` 字节 + 截断标记，
///   剩余静默丢弃至行尾，游标对齐下一行行首；
/// - EOF 且无字节 = `None`；EOF 收尾的部分行仍返回；
/// - **不做 UTF-8 转换**——行字节原样返回，由调用方整行收齐后一次性
///   lossy（非 UTF-8 字节不构成错误——外部对端 locale 输出不应一行
///   杀死消费循环，BUG-20261113-135）。
pub fn read_line_capped_sync<R>(
    reader: &mut R,
    cap: usize,
) -> std::io::Result<Option<(Vec<u8>, bool)>>
where
    R: std::io::BufRead,
{
    let mut raw: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            // EOF：积累过字节 = 无换行尾行；全空 = 纯 EOF。
            return if raw.is_empty() && !truncated {
                Ok(None)
            } else {
                Ok(Some((raw, truncated)))
            };
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                if !truncated && total_would_exceed(raw.len(), pos + 1, cap) {
                    truncated = true;
                }
                if truncated {
                    let keep = cap.saturating_sub(raw.len());
                    raw.extend_from_slice(&available[..keep.min(pos + 1)]);
                    // 丢弃剩余字节直到行尾，游标落在下一行行首。
                    reader.consume(pos + 1);
                    return Ok(Some((raw, true)));
                }
                raw.extend_from_slice(&available[..pos + 1]);
                reader.consume(pos + 1);
                return Ok(Some((raw, false)));
            }
            None => {
                if !truncated && total_would_exceed(raw.len(), available.len(), cap) {
                    truncated = true;
                }
                if truncated {
                    let keep = cap.saturating_sub(raw.len());
                    raw.extend_from_slice(&available[..keep]);
                    let n = available.len();
                    reader.consume(n);
                    // 丢弃模式：消费到行尾/EOF 为止。
                    continue;
                }
                let n = available.len();
                raw.extend_from_slice(available);
                reader.consume(n);
            }
        }
    }
}

#[cfg(test)]
mod tests;
