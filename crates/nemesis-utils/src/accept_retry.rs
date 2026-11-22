//! TCP accept 循环的韧性退避助手（单一真相源）。
//!
//! 家族问题：`listener.accept()` 的 Err 臂「log + continue」无退避——
//! Windows accept 有 WSAECONNRESET 怪癖（客户端在 accept 前连接+重置即
//! 返回 Err，std/tokio 文档均载），端口扫描器 / 健康检查 / 故障客户端的
//! 连接重置风暴会让 accept 循环进入热自旋（CPU 烧满 + 日志洪泛，每秒
//! 百万级 warn）。Linux 侧 fd 耗尽族（EMFILE/ENFILE）同样永久 Err。
//!
//! 正解先例：`nemesis-guest::serve_tcp`（瞬态/资源耗尽分诊 + 差异化
//! 退避 + 致命错误传播）。本模块把分诊与退避时长收敛为共享 API，供
//! 全部 accept 循环消费（通道 webhook/WS 监听、cluster RPC server、
//! desktop 子窗口 WS、oauth 回调、maixcam TCP）。分类判定错误语义
//! 只此一份——各消费点按自身循环策略决定 continue/传播，但退避时长
//! 与「瞬态 vs 耗尽 vs 未知」的裁决不再各自为政。

use std::time::Duration;

/// accept 瞬态错误判定：连接级抖动（连接在 accept 前被对端重置/中止、
/// 被信号打断）——退避后重试即可恢复。
pub fn transient_accept_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted
    ) || resource_exhausted_accept(e)
}

/// accept 的资源耗尽类错误（EMFILE/ENFILE/ENOMEM/ENOBUFS）：Rust 的
/// `ErrorKind` 对 fd 耗尽族无稳定映射（落 Uncategorized），按 raw OS
/// 错误码判定。非 unix 恒 false（Windows accept 无此错误族）。
#[cfg(unix)]
pub fn resource_exhausted_accept(e: &std::io::Error) -> bool {
    // musl/glibc 同值：EMFILE=24 ENFILE=23 ENOMEM=12 ENOBUFS=105。
    matches!(e.raw_os_error(), Some(24) | Some(23) | Some(12) | Some(105))
}

#[cfg(not(unix))]
pub fn resource_exhausted_accept(_e: &std::io::Error) -> bool {
    false
}

/// 连接级瞬态的退避：极短——正常抖动高频出现时压延迟不压吞吐。
pub const ACCEPT_BACKOFF_TRANSIENT: Duration = Duration::from_millis(10);

/// 资源耗尽族的退避：等 fd/内存释放（10ms 级重试在耗尽态只会空转打日志）。
pub const ACCEPT_BACKOFF_EXHAUSTED: Duration = Duration::from_millis(500);

/// 未知/致命错误（listener 本体损坏等）的退避：1s——继续跑保服务可恢复
/// （bind 成功后 accept 永久 Err 极罕见），但绝不热自旋。
pub const ACCEPT_BACKOFF_UNKNOWN: Duration = Duration::from_millis(1000);

/// accept 错误的续跑退避时长（家族分诊单一出口）。
pub fn accept_error_backoff(e: &std::io::Error) -> Duration {
    if resource_exhausted_accept(e) {
        ACCEPT_BACKOFF_EXHAUSTED
    } else if transient_accept_error(e) {
        ACCEPT_BACKOFF_TRANSIENT
    } else {
        ACCEPT_BACKOFF_UNKNOWN
    }
}

#[cfg(test)]
mod tests;
