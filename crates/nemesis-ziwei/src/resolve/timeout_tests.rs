//! BUG-20261116-022 回归锁：resolve_locates 分块扫描的块读必须 CMD_TIMEOUT
//! 包装（session 层 read_mem/poll_events/dispatch 入口同纪律）。修复前块读
//! 裸 await：目标停摆（挂起/死锁）时 read_frame 永挂，解析任务持 link 互斥
//! 锁不放——poll_loop 连锁挂、普通 detach（无超时预算）永挂、estop 30s 预算
//! 内卸载失败（payload 带钩子常驻目标进程）。修复后：块读超时 → 诚实 note
//! + 止损返回 + 链路按 pipe.rs 交换窗纪律恒 LinkUnreliable。

use std::time::Duration;

use nemesis_ziwei_profile::{LocateKind, LocateSpec};

use crate::pipe::{PipeError, create_server, finish_handshake, read_mem};
use crate::resolve::resolve_locates;

/// fake payload 完成握手后只读帧永不应答（目标挂起/死锁形态）：
/// resolve_locates 必须在预算内止损返回（修复前：首块读裸 await 永挂，
/// 外层 30s 预算打穿 = 红证）。
#[tokio::test]
async fn resolve_chunk_read_timeout_stalls_scan_and_poisons_link() {
    let name = format!(
        r"\\.\pipe\nemesis-resolve-tmo-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let server = create_server(&name).unwrap();
    let token = "1234567890abcdef1234567890abcdef";
    let client = tokio::spawn(async move {
        let mut stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&name)
            .unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let hello = format!(
            "{{\"proto\":1,\"token\":\"{token}\",\"arch\":\"x64\",\"pid\":{}}}",
            std::process::id()
        );
        stream.write_u32_le(hello.len() as u32).await.unwrap();
        stream.write_all(hello.as_bytes()).await.unwrap();
        // 只排干请求帧、永不应答——目标停摆形态。
        loop {
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut sink = vec![0u8; len];
            if stream.read_exact(&mut sink).await.is_err() {
                break;
            }
        }
    });
    let mut link = finish_handshake(server, token, std::process::id(), "x64")
        .await
        .unwrap();

    let locates = vec![(
        "probe_fn".to_string(),
        LocateSpec {
            kind: LocateKind::BytePattern,
            module: None,
            pattern: Some("48 89 5C 24".to_string()),
            rva: None,
        },
    )];

    let report = tokio::time::timeout(Duration::from_secs(30), async {
        resolve_locates(&mut link, std::process::id(), locates)
            .await
            .expect("resolve_locates 应止损返回 Ok（含诚实 note）")
    })
    .await
    .expect("resolve_locates 必须在预算内返回（块读 CMD_TIMEOUT 止损）——裸 await 永挂回归");

    assert!(
        report.addrs.is_empty(),
        "停摆目标上不得解析出地址（首块读即超时），got: {:?}",
        report.addrs
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("读取超时") && n.contains("解析中止")),
        "必须有诚实块读超时 note，got: {:?}",
        report.notes
    );
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("probe_fn") && n.contains("解析中止")),
        "未扫到的锚不得误报「锚未命中」（扫描中止须如实注明），got: {:?}",
        report.notes
    );

    // 交换窗纪律：超时取消滞留交换窗 → 同链路下一条命令恒 LinkUnreliable
    //（pipe.rs 第四次外评语义——绝不冒领迟到应答）。
    let next = read_mem(&mut link, 0x6000, 1).await;
    assert!(
        matches!(next, Err(PipeError::LinkUnreliable)),
        "超时止损后的链路必须诚实拒绝后续命令，got: {next:?}"
    );

    client.abort();
    let _ = client.await;
}
