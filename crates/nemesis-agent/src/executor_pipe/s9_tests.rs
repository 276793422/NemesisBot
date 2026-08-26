//! S9 覆盖率批次：executor_pipe.rs 剩余未覆盖行。
//! - 63-66/68：connect_client 的 Err 重试臂 + 截止到点返回 + 重试 sleep。
//!   连接一个必然不存在的管道名 → ERROR_FILE_NOT_FOUND → 每轮 sleep 20ms
//!   直到 10s deadline 返回 Err。
//!
//! ⚠️ 测试硬成本 ~10s（deadline 写死 10s；tokio start_paused 用不了——
//! std::time::Instant 是真实时钟，暂停时间会死循环）。只此一个。

#[cfg(windows)]
#[tokio::test]
async fn connect_client_to_missing_pipe_times_out_with_error() {
    use super::*;
    // unique_pipe_id 保证不与任何真实管道撞名
    let name = pipe_name(&format!("s9_missing_{}", unique_pipe_id()));
    let started = std::time::Instant::now();
    let res = connect_client(&name).await;
    let elapsed = started.elapsed();
    let err = res.expect_err("pipe must not exist");
    // Windows: 打开不存在的管道 → os error 2 (ERROR_FILE_NOT_FOUND)
    assert_eq!(err.raw_os_error(), Some(2), "got: {}", err);
    assert!(
        elapsed >= std::time::Duration::from_secs(9),
        "must have retried until the 10s deadline, elapsed {:?}",
        elapsed
    );
}
