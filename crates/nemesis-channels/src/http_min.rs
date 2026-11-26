//! 手写 webhook 服务器的最小 HTTP 请求**有界重组读**（wechat/line 共用单一真相源）。
//!
//! # 为什么不能单次 `read()`
//!
//! BUG-20261126-106：`TcpStream::read()` 语义是「返回内核缓冲区**当时已有**
//! 的字节」——互联网 MTU（~1460B）分段下一个 >1.4KB 的 POST 会跨多个 TCP
//! segment 到达，单次读拿到的是**部分请求**。旧形态（wechat.rs / line.rs
//! 各自的 `handle_connection` 单次读）对截断 body 验签/解析必然失败：
//! line 是 fail-closed 验签 → 401，wechat 是 body 解析失败 → 400——真实
//! 网络部署下大消息（CJK 文本轻松 >4KB）**永久丢失**且平台重试同样分段
//! 无法自愈。wechat.rs 模块头声明的「>64KB 超长截断 → 400」只罩超帽形态，
//! 未罩分段形态（本轮修复的正是后者）。
//!
//! # 重组纪律
//!
//! - 读到 header 区完整（`\r\n\r\n`）后解析 `Content-Length`（ASCII 大小写
//!   不敏感，首个命中生效）；
//! - 有 CL：继续读直到 body 满 CL（**在满即返回**——管道化的后续请求字节
//!   不消费，Connection: close 形态下无影响）；
//! - 无 CL（HTTP/1.0 EOF 终止或异常形态兜底）：header 完整后进入静默窗
//!   排空（[`MinReadConfig::idle_timeout`] 无新字节即视为到齐；EOF 视为
//!   HTTP/1.0 合法终止）；
//! - 全程墙钟超时（[`MinReadConfig::overall_timeout`]）——慢速滴注
//!   （slowloris）有界；旧单读形态首字节等待本来就无界，超时是严格收紧；
//! - 64KB 帽（[`MinReadConfig::max_bytes`]）：缓冲超帽或 CL 声明总量超帽
//!   都立即 [`MinReadError::Oversize`]（调用方 → 400；等同旧「超长 → 400」
//!   声明语义，只是不再吞掉后半段的等待）。
//!
//! # 诚实边界
//!
//! - `Transfer-Encoding: chunked` 不支持（LINE/微信平台 webhook 均带 CL；
//!   chunked 形态走无 CL 静默窗排空后按原样字节交调用方，解析失败由调用
//!   方诚实拒绝，不做 chunk 解帧）；
//! - 请求行/header 合法性不在本层校验（调用方已各自解析并 fail-closed）。

use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// 请求重组默认帽：64KB（wechat.rs 旧声明的超长帽同值，语义延续）。
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024;
/// 默认全程墙钟超时。
pub const DEFAULT_OVERALL_TIMEOUT: Duration = Duration::from_secs(10);
/// 无 CL 形态的静默窗（无新字节即视为请求到齐）。
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_millis(300);

/// 重组读配置（默认值见各常量）。
#[derive(Clone, Copy, Debug)]
pub struct MinReadConfig {
    pub overall_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_bytes: usize,
}

impl Default for MinReadConfig {
    fn default() -> Self {
        Self {
            overall_timeout: DEFAULT_OVERALL_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// 重组读失败（调用方映射：Oversize → 400；其余静默断开，与旧 Ok(0)|Err 形态一致）。
#[derive(Debug)]
pub enum MinReadError {
    /// 对端在 header 完成前断开，或带 CL 请求 body 未满即断（截断请求不投递）。
    Eof,
    /// 全程墙钟超时（含 header 未完成阶段的首字节等待）。
    Timeout,
    /// 缓冲超帽或 CL 声明总量超帽。
    Oversize,
    /// 底层 IO 错误。
    Io(std::io::Error),
}

/// 有界重组读一整个 HTTP 请求（默认配置）。
pub async fn read_request_bounded(stream: &mut TcpStream) -> Result<Vec<u8>, MinReadError> {
    read_request_bounded_with(stream, MinReadConfig::default()).await
}

/// 有界重组读一整个 HTTP 请求（显式配置——测试用短超时/短静默窗）。
pub async fn read_request_bounded_with(
    stream: &mut TcpStream,
    cfg: MinReadConfig,
) -> Result<Vec<u8>, MinReadError> {
    let deadline = tokio::time::Instant::now() + cfg.overall_timeout;
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 4096];
    // 无 CL 静默窗排空模式（header 完整且解析不出 CL 时置位）。
    let mut no_cl_drain = false;

    loop {
        if buf.len() > cfg.max_bytes {
            return Err(MinReadError::Oversize);
        }

        let header_end = find_header_end(&buf);
        if let Some(hdr_end) = header_end
            && !no_cl_drain
        {
            match content_length(&buf[..hdr_end]) {
                Some(cl) => {
                    let total = hdr_end + cl;
                    if total > cfg.max_bytes {
                        return Err(MinReadError::Oversize);
                    }
                    if buf.len() >= total {
                        return Ok(buf);
                    }
                }
                None => no_cl_drain = true,
            }
        }

        // 本轮读等待：无 CL 模式等静默窗；否则等剩余墙钟。
        let wait = if no_cl_drain {
            cfg.idle_timeout
        } else {
            deadline.saturating_duration_since(tokio::time::Instant::now())
        };
        if wait.is_zero() {
            return Err(MinReadError::Timeout);
        }

        match tokio::time::timeout(wait, stream.read(&mut chunk)).await {
            Err(_elapsed) => {
                if no_cl_drain {
                    // 静默窗耗尽：无 CL 请求视为到齐。
                    return Ok(buf);
                }
                return Err(MinReadError::Timeout);
            }
            Ok(Err(e)) => return Err(MinReadError::Io(e)),
            Ok(Ok(0)) => {
                return if no_cl_drain {
                    // HTTP/1.0：EOF 是合法 body 终止符。
                    Ok(buf)
                } else {
                    Err(MinReadError::Eof)
                };
            }
            Ok(Ok(k)) => buf.extend_from_slice(&chunk[..k]),
        }
    }
}

/// header 区终点（`\r\n\r\n` 之后）。
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// 从 header 区解析 Content-Length（ASCII 大小写不敏感，首个命中生效；
/// 非法数值 = None = 无 CL 兜底形态）。
fn content_length(header_block: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(header_block);
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            return value.trim().parse::<usize>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests;
