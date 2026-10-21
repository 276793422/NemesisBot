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
    let (mut client, mut server) = tokio::io::duplex(64);
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
