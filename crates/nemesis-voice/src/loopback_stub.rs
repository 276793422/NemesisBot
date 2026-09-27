//! loopback（远端参考采集线程）的诚实降级 stub（`voice-capture` feature 未启用时编译）。
//!
//! 真实现在同目录 `loopback.rs`（wasapi loopback 采集扬声器输出作 AEC far-end 参考）。
//! 调用方（nemesis-web voice.rs）只在 AEC 初始化成功后启动 loopback——AEC stub 恒失败，
//! 本 stub 正常路径不可达；万一被调到也诚实记日志、no-op 返回（签名返回 `()`）。

pub fn start_loopback() {
    tracing::debug!("voice-capture feature 未编译：loopback 远端参考不可用（no-op）");
}

pub fn stop_loopback() {}
