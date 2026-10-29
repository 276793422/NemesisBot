//! M1a broadcast 事件收尾排干——K2 NDJSON（`commands/run.rs`）与 ACP
//! update 泵收尾（`acp.rs`）共用的单一真相源。
//!
//! 任务完成后的残余事件排干必须**穿透 Lagged**：broadcast 环形缓冲被
//! 覆盖时 `try_recv()` 首次返回 `Err(Lagged(n))`，但游标已同时推进到最旧
//! 存活事件——尾部未覆盖事件仍然可读。`while let Ok` 形态会在首个
//! Lagged 处提前终止，把尾部事件（典型：配对的 `tool_end`）整体丢弃
//! ——K2 消费方看到 `tool_start` 无 `tool_end`，ACP 编辑器侧留下悬空
//! in_progress 工具卡。正确形态是 Lagged 继续排干，Empty/Closed 才终止。

/// 排干 receiver 中所有可读事件；`Lagged`（缓冲覆盖）不终止——游标已
/// 推进到最旧存活事件，继续读到 `Empty`/`Closed` 为止。
pub fn drain_residual<T: Clone>(rx: &mut tokio::sync::broadcast::Receiver<T>) -> Vec<T> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(ev) => out.push(ev),
            // 覆盖不终止：游标已推进到最旧存活事件，继续排干尾部。
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            // Empty（读尽）/ Closed（sender 全 drop 且已读尽）才终止。
            Err(_) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests;
