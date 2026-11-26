//! BUG-8（2026-11-25 远程 home 取证）回归：RequestLogger 写盘 hang 曾在
//! `session_dir` 互斥锁内做磁盘 I/O——一次写盘故障（D 盘渐进故障现场）
//! 把锁永久带死，此后**所有**日志调用（含只读的 `session_dir()`）全部
//! 阻塞，agent loop 整体停摆；且失败完全静默（`let _ =` 无计数无告警）。
//!
//! 修复语义在此钉死：
//! 1. 锁内只 clone 目录路径，写盘移出锁（异步上下文经 spawn_blocking
//!    fire-and-forget，sync 上下文保持同步写）；
//! 2. 失败记账：连续计数 + 累计总数 + 限频 WARN；成功清零；
//! 3. 失败后首次成功发恢复 WARN（was_failing 标记）；
//! 4. 写失败不 panic、不阻塞后续调用与只读口。

use super::*;

/// 失败记账语义：连续计数累加、成功清零、was_failing 恢复标记、总数累计。
#[test]
fn write_stats_failure_accounting_and_recovery() {
    let stats = WriteStats::default();
    let err = "Failed to create file: Access denied".to_string();

    record_write_result(&stats, Some(&err), Path::new("C:\\fake\\00.request.md"));
    record_write_result(&stats, Some(&err), Path::new("C:\\fake\\01.AI.Request.md"));
    assert_eq!(stats.consecutive_failures.load(Ordering::Relaxed), 2);
    assert_eq!(stats.failures_total.load(Ordering::Relaxed), 2);
    assert!(
        stats.was_failing.load(Ordering::Relaxed),
        "失败必须置恢复标记"
    );

    // 成功：连续清零、总数保留、was_failing 清（恢复 WARN 发过后）。
    record_write_result(&stats, None, Path::new("C:\\fake\\02.AI.Response.md"));
    assert_eq!(stats.consecutive_failures.load(Ordering::Relaxed), 0);
    assert_eq!(stats.failures_total.load(Ordering::Relaxed), 2);
    assert!(!stats.was_failing.load(Ordering::Relaxed));

    // 恢复后再失败：总数续计（3）。
    record_write_result(&stats, Some(&err), Path::new("C:\\fake\\03.Local.md"));
    assert_eq!(stats.failures_total.load(Ordering::Relaxed), 3);
    assert_eq!(stats.consecutive_failures.load(Ordering::Relaxed), 1);
}

/// 同步路径（无 tokio runtime）写失败：不 panic、session 目录路径目标文件
/// 被目录占位时写失败记账生效；占位移除后下一写恢复落盘（自愈语义）。
#[test]
fn sync_write_failure_recovers_after_blocker_removed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Full,
        log_dir: tmp.path().to_string_lossy().to_string(),
        save_raw: false,
    };
    let logger = RequestLogger::new(config, Path::new("."));
    logger.create_session().expect("session");
    let dir = logger.session_dir().expect("session dir");

    logger.log_user_request(&UserRequestInfo {
        timestamp: Local::now(),
        channel: "web".into(),
        sender_id: "u1".into(),
        chat_id: "c1".into(),
        content: "hello".into(),
    });
    // 直接断言文件已落盘。
    let files: Vec<_> = std::fs::read_dir(&dir)
        .expect("session dir readable")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        files.iter().any(|f| f.ends_with(".request.md")),
        "user request 文件必须落盘: {files:?}"
    );

    // 占位下一个 index 的目标文件名（同名目录 → File::create 必败）。
    let blocker = dir.join(format!(
        "{:02}.AI.Request.md",
        logger.file_index.load(Ordering::Relaxed)
    ));
    std::fs::create_dir(&blocker).expect("blocker dir");

    // 写失败路径：不 panic；无 runtime 上下文走同步分支 + 记账。
    logger.log_llm_request(&LLMRequestInfo {
        round: 1,
        timestamp: Local::now(),
        model: "test-model".into(),
        provider_name: "p".into(),
        api_key: "k".into(),
        api_base: "http://x".into(),
        messages_count: 1,
        tools_count: 0,
        messages: Vec::new(),
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });
    assert!(
        logger
            .write_stats
            .consecutive_failures
            .load(Ordering::Relaxed)
            >= 1,
        "写失败必须记账（连续计数 ≥1）"
    );
    // 只读口不被拖死（BUG-8 现场：锁内写盘 hang 把只读口一起带死）。
    assert_eq!(logger.session_dir(), Some(dir.clone()));

    // 占位移除 → 下一写恢复落盘 + 连续计数清零。
    std::fs::remove_dir(&blocker).expect("blocker removed");
    logger.log_llm_request(&LLMRequestInfo {
        round: 2,
        timestamp: Local::now(),
        model: "test-model".into(),
        provider_name: "p".into(),
        api_key: "k".into(),
        api_base: "http://x".into(),
        messages_count: 1,
        tools_count: 0,
        messages: Vec::new(),
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });
    assert_eq!(
        logger
            .write_stats
            .consecutive_failures
            .load(Ordering::Relaxed),
        0,
        "恢复成功必须清零连续计数"
    );
    let recovered = dir.join(format!(
        "{:02}.AI.Request.md",
        logger.file_index.load(Ordering::Relaxed) - 1
    ));
    assert!(
        recovered.exists(),
        "恢复写必须落盘: {}",
        recovered.display()
    );
}

/// 异步路径（tokio runtime）：spawn_blocking fire-and-forget 投递后文件
/// 最终落盘（不等写完成返回——调用方零阻塞语义）。
#[tokio::test]
async fn async_write_lands_via_spawn_blocking() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Full,
        log_dir: tmp.path().to_string_lossy().to_string(),
        save_raw: false,
    };
    let logger = RequestLogger::new(config, Path::new("."));
    logger.create_session().expect("session");
    let dir = logger.session_dir().expect("session dir");

    logger.log_final_response(&FinalResponseInfo {
        timestamp: Local::now(),
        total_duration_ms: 1234,
        llm_rounds: 2,
        content: "final answer".into(),
        channel: "web".into(),
        chat_id: "c1".into(),
    });

    // fire-and-forget：返回即 Ok；文件在 blocking 线程落盘——轮询等待。
    let want = dir.join(format!(
        "{:02}.response.md",
        logger.file_index.load(Ordering::Relaxed) - 1
    ));
    let mut landed = false;
    for _ in 0..100 {
        if want.exists() {
            landed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(landed, "spawn_blocking 异步写必须落盘: {}", want.display());
    let body = std::fs::read_to_string(&want).expect("read back");
    assert!(body.contains("final answer"), "内容完整: {body:?}");
    assert_eq!(
        logger
            .write_stats
            .consecutive_failures
            .load(Ordering::Relaxed),
        0,
        "成功写零失败记账"
    );
}

/// 饱和闸（BUG-8 复核补强）：在飞写达到上限 = 后续写入丢弃 + 失败记账 +
/// 在飞计数原位归还（不泄漏）；释放槽位后下一写恢复落盘 + 连续计数清零。
/// 语义目标：挂死写堆积最多占满本 logger 的配额，不再无界压进 tokio
/// blocking 池饿死无关任务。
#[test]
fn in_flight_saturation_drops_and_releases() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = LoggingConfig {
        enabled: true,
        detail_level: DetailLevel::Full,
        log_dir: tmp.path().to_string_lossy().to_string(),
        save_raw: false,
    };
    let logger = RequestLogger::new(config, Path::new("."));
    logger.create_session().expect("session");
    let st = &logger.write_stats;

    // 人为填满在飞计数（模拟挂死写堆积；不真挂线程）。
    st.in_flight.store(MAX_IN_FLIGHT_WRITES, Ordering::SeqCst);

    logger.log_llm_request(&LLMRequestInfo {
        round: 1,
        timestamp: Local::now(),
        model: "test-model".into(),
        provider_name: "p".into(),
        api_key: "k".into(),
        api_base: "http://x".into(),
        messages_count: 1,
        tools_count: 0,
        messages: Vec::new(),
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });
    assert_eq!(
        st.in_flight.load(Ordering::SeqCst),
        MAX_IN_FLIGHT_WRITES,
        "丢弃路径必须原位归还，在飞计数不泄漏"
    );
    assert_eq!(
        st.consecutive_failures.load(Ordering::Relaxed),
        1,
        "饱和丢弃必须记账为失败"
    );
    assert!(st.was_failing.load(Ordering::Relaxed), "置恢复标记");

    // 释放一个槽（模拟挂死解除）→ 下一写恢复落盘 + 连续计数清零。
    st.in_flight.fetch_sub(1, Ordering::SeqCst);
    logger.log_llm_request(&LLMRequestInfo {
        round: 2,
        timestamp: Local::now(),
        model: "test-model".into(),
        provider_name: "p".into(),
        api_key: "k".into(),
        api_base: "http://x".into(),
        messages_count: 1,
        tools_count: 0,
        messages: Vec::new(),
        http_headers: Vec::new(),
        config: std::collections::HashMap::new(),
        fallback_attempts: Vec::new(),
    });
    assert_eq!(
        st.consecutive_failures.load(Ordering::Relaxed),
        0,
        "成功写清零"
    );
    assert_eq!(
        st.in_flight.load(Ordering::SeqCst),
        MAX_IN_FLIGHT_WRITES - 1,
        "完成路径归还槽位"
    );
}
