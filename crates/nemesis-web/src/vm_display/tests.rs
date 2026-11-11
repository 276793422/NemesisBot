//! [`vm_display`] 单测（feature = vm-display 门控）。测试走模块级静态槽
//! （[`super::set_endpoint`] / manager），与 pty tests 同款纪律：不安装
//! 进程级 config store——config 闸（`read_display_config`）不在单测覆盖
//! 面（它读 `load_live()` 全局，会劫持同 binary 其他测试的家目录隔离），
//! 闸语义由 handler 闸链的结构位置保证（升级入口第一行）。

use super::*;

/// 长度前缀帧编解码往返（上行 cap 边界内）。
#[tokio::test]
async fn frame_roundtrip() {
    let (mut client, mut server) = tokio::io::duplex(64);
    let payload = b"RFB-handshake-bytes".to_vec();
    write_frame(&mut client, &payload).await.expect("write ok");
    let got = read_frame(&mut server, MAX_UPSTREAM_FRAME)
        .await
        .expect("read ok");
    assert_eq!(got, payload);
}

/// 超长帧拒绝（cap 闸诚实拒绝而非静默截断/无限分配）。
#[tokio::test]
async fn frame_over_cap_rejected() {
    let (mut client, mut server) = tokio::io::duplex(64);
    write_frame(&mut client, &[0u8; 32])
        .await
        .expect("write ok");
    let err = read_frame(&mut server, 8)
        .await
        .expect_err("超 cap 必须拒绝");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("exceeds cap"));
}

/// 连接中断（对端关流）→ read_frame Err（EOF），泵任务自然退出。
#[tokio::test]
async fn frame_read_eof_on_close() {
    let (client, mut server) = tokio::io::duplex(64);
    drop(client); // 对端关闭
    let r = read_frame(&mut server, MAX_UPSTREAM_FRAME).await;
    assert!(r.is_err(), "对端关闭后 read_frame 应 Err");
}

/// 端点槽：未装载 = None（501 闸的数据面）；装载后可见；重复装载不同
/// 端口 = first-wins 拒绝（插件热重载不做运行时换端的契约锚）。
#[test]
fn endpoint_slot_semantics() {
    // 注意：OnceLock 进程级——本测试依赖「其他测试未先占槽」。为避免与
    // 并行测试互踩，只断言「装载动作的幂等/拒绝语义」在**未占槽时**成立；
    // 若 CI 里真有并行测试先占，则走已占分支同样断言 first-wins。
    let probe = VmDisplayEndpoint {
        listen_port: 59999,
        plugin_abi_version: 1,
    };
    match set_endpoint(probe.clone()) {
        Ok(()) => {
            let got = endpoint().expect("装载后可见");
            assert_eq!(got.listen_port, 59999);
            assert_eq!(got.plugin_abi_version, 1);
            // second set with different port must be rejected
            let err = set_endpoint(VmDisplayEndpoint {
                listen_port: 59998,
                plugin_abi_version: 1,
            })
            .expect_err("first-wins：换端口装载必须拒绝");
            assert!(err.contains("已被占用"), "{err}");
            assert_eq!(endpoint().unwrap().listen_port, 59999, "旧端点保持");
        }
        Err(_) => {
            // 已被其他测试占槽：first-wins 生效即通过。
            assert!(endpoint().is_some());
        }
    }
}

/// 会话管理器：注册/摘牌/满员/kill-all 全生命周期（独立实例，不占模块级
/// manager——那个被 handler 用，测行为等价面在真实例上即可）。
#[test]
fn manager_register_full_and_kill_all() {
    let mgr = VmDisplayManager::new();
    for i in 0..MAX_DISPLAY_SESSIONS {
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let id = mgr
            .register(tx, MAX_DISPLAY_SESSIONS)
            .unwrap_or_else(|| panic!("第 {i} 个会话应可注册"));
        assert_ne!(id, 0, "会话 id 从 1 起");
    }
    assert_eq!(mgr.session_count(), MAX_DISPLAY_SESSIONS);
    // 满员：第 5 个拒绝。
    let (tx, _rx) = tokio::sync::watch::channel(false);
    assert!(
        mgr.register(tx, MAX_DISPLAY_SESSIONS).is_none(),
        "满员必须拒绝"
    );
    // kill-all 清空（kill 通知语义由会话循环内的 watch 消费，这里只验
    // 注册表面）。
    mgr.kill_all();
    assert_eq!(mgr.session_count(), 0, "kill-all 后清空");
    // 摘牌语义。
    let (tx2, _rx2) = tokio::sync::watch::channel(false);
    let id2 = mgr.register(tx2, MAX_DISPLAY_SESSIONS).expect("重新可注册");
    mgr.unregister(id2);
    assert_eq!(mgr.session_count(), 0);
}

/// estop 带外杀链回归锁（PTY BUG-20261031-234 同款失败形态）：会话任务
/// 停在不可自醒的 await 点（客户端 TCP 背压下的 socket.send——watch 通知
/// 观察不到）时，kill_all 必须经带外 abort 终止任务，不依赖任务自己
/// poll 到 kill 通知；注册表同步清空。修复前 track/abort 链不存在，
/// 本测试无法编译（poll-only 形态不可写）。
#[tokio::test]
async fn estop_kill_all_aborts_stalled_session_task() {
    let mgr = VmDisplayManager::new();
    let (tx, _rx) = tokio::sync::watch::channel(false);
    // 会话任务替身：永久 pending、不 select kill watch——等价于主循环
    // 卡死在 socket.send（任何 watch 通知都观察不到的病态）。
    let task = tokio::spawn(std::future::pending::<()>());
    mgr.register(tx, MAX_DISPLAY_SESSIONS).expect("注册");
    mgr.track(task);
    assert_eq!(mgr.session_count(), 1);
    assert_eq!(mgr.tracked_len(), 1);
    let aborted = mgr.kill_all();
    assert_eq!(mgr.session_count(), 0, "注册表同步清空");
    assert_eq!(aborted.len(), 1, "带外句柄随 kill_all 交还");
    for h in aborted {
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), h)
            .await
            .expect("abort 后 2s 内必须终止");
        assert!(
            matches!(joined, Err(ref e) if e.is_cancelled()),
            "pending 任务只能被 abort 终止（Cancelled）"
        );
    }
}

/// track 修剪：已完结句柄不留尸（正常退出的会话不随 gateway 寿命累积）。
#[tokio::test]
async fn track_prunes_finished_handles() {
    let mgr = VmDisplayManager::new();
    let first = tokio::spawn(async {});
    while !first.is_finished() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    mgr.track(first);
    assert_eq!(mgr.tracked_len(), 1);
    // 第二次 track 修剪已完结的 first，只留新句柄（新句柄即使瞬间跑完
    // 也是 push 后才可能被下一轮修剪，计数恒 1）。
    mgr.track(tokio::spawn(async {}));
    assert_eq!(mgr.tracked_len(), 1);
}
