//! 协议循环单测（进程内驱动 GuestAgent，不 spawn 二进制——二进制级全链
//! 测试在 tests/full_chain.rs）。

use std::path::PathBuf;

use super::*;

fn agent_with_tmp() -> (GuestAgent, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    (GuestAgent::new(dir.path().to_path_buf()), dir)
}

fn v2_line(tool: &str, args: &str) -> String {
    nemesis_exec_protocol::build_v2_request_line(
        tool,
        args,
        &serde_json::json!({"session_id": "t"}),
        "c-test-1",
    )
    .expect("build v2 line")
}

#[tokio::test]
async fn v2_request_gets_v2_response_with_call_id_echo() {
    let (agent, _dir) = agent_with_tmp();
    let resp = agent.serve_line(&v2_line("list_dir", "{}")).await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(
        parsed.ok,
        "list_dir on empty workspace should succeed: {resp}"
    );
    assert_eq!(parsed.v, PROTOCOL_V2);
    assert_eq!(parsed.call_id, "c-test-1");
}

#[tokio::test]
async fn v1_request_loud_rejected() {
    let (agent, _dir) = agent_with_tmp();
    // v1 形态（无 v 无 call_id）。
    let line = r#"{"tool":"list_dir","args":"{}","context":{}}"#;
    let resp = agent.serve_line(line).await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(!parsed.ok);
    assert!(parsed.error.contains("requires v2"), "{resp}");
    assert_eq!(parsed.call_id, "", "v1 请求无 call_id 可回显");
}

#[tokio::test]
async fn bad_json_line_gets_v1_form_error() {
    let (agent, _dir) = agent_with_tmp();
    let resp = agent.serve_line("not-json").await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(!parsed.ok);
    assert!(parsed.error.contains("bad request line"), "{resp}");
}

#[tokio::test]
async fn unknown_tool_loud_error_lists_subset() {
    let (agent, _dir) = agent_with_tmp();
    let resp = agent.serve_line(&v2_line("git", "{}")).await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(!parsed.ok);
    assert!(parsed.error.contains("unknown tool: git"), "{resp}");
    assert!(parsed.error.contains("exec"), "{resp}");
}

#[tokio::test]
async fn exec_echo_roundtrip() {
    let (agent, _dir) = agent_with_tmp();
    let resp = agent
        .serve_line(&v2_line("exec", r#"{"command":"echo guest-smoke"}"#))
        .await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(parsed.ok, "{resp}");
    assert!(parsed.result.contains("guest-smoke"), "{resp}");
}

#[tokio::test]
async fn exec_nonzero_exit_is_ok_with_exit_code_text() {
    let (agent, _dir) = agent_with_tmp();
    // 宿主 ExecTool 语义：非零退出 = Ok 带 "Exit code: N" 文本。
    let cmd = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
    let resp = agent
        .serve_line(&v2_line("exec", &format!(r#"{{"command":"{cmd}"}}"#)))
        .await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(parsed.ok, "非零退出是 Ok 语义: {resp}");
    assert!(parsed.result.contains("Exit code: 3"), "{resp}");
}

// BUG-20261114-002 回归锁：Windows cmd /C 必须 raw_arg 逐字传命令串。
// `.arg()` 的 MSVCRT 转义把内嵌引号变 `\"`，cmd 不认 `\"`——带引号命令
// 被打挂（宿主 cc_hooks 实测先例；raw_arg 后 echo 原样输出带引号串）。
// Unix sh -c 无引号层，`.arg()` 本就逐字（测试挂 cfg(windows) 按
// Windows 形态测试标记约定，Linux 编译期消失）。
#[cfg(windows)]
#[tokio::test]
async fn exec_windows_cmd_preserves_embedded_quotes() {
    let (agent, _dir) = agent_with_tmp();
    let resp = agent
        .serve_line(&v2_line("exec", r#"{"command":"echo \"a b\""}"#))
        .await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(parsed.ok, "{resp}");
    assert!(
        parsed.result.contains("\"a b\""),
        "引号必须原样保留（raw_arg 逐字语义）: {resp}"
    );
}

#[tokio::test]
async fn write_read_delete_file_roundtrip() {
    let (agent, dir) = agent_with_tmp();
    async fn send(agent: &GuestAgent, tool: &str, args: &str) -> String {
        let line =
            nemesis_exec_protocol::build_v2_request_line(tool, args, &serde_json::json!({}), "c-t")
                .unwrap();
        agent.serve_line(&line).await
    }

    let resp = send(
        &agent,
        "write_file",
        r#"{"path":"a/b.txt","content":"hello guest"}"#,
    )
    .await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert!(p.ok, "{resp}");
    assert!(p.result.contains("wrote 11 bytes"), "{resp}");

    let resp = send(&agent, "read_file", r#"{"path":"a/b.txt"}"#).await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert!(p.ok && p.result == "hello guest", "{resp}");

    // 2026-11-06 append_file 并入 edit_file：追加走 action=append。
    let resp = send(
        &agent,
        "edit_file",
        r#"{"path":"a/b.txt","action":"append","content":"!"}"#,
    )
    .await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert!(p.ok && p.result.contains("Appended 1 bytes"), "{resp}");

    // action=edit（精确替换）guest 未实现 → 诚实拒绝（回归锁：防静默
    // 降级实现或误报 unknown tool）。
    let resp = send(
        &agent,
        "edit_file",
        r#"{"path":"a/b.txt","old_text":"guest","new_text":"world"}"#,
    )
    .await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert!(!p.ok, "{resp}");
    assert!(
        p.error.contains("not implemented in the guest subset"),
        "{resp}"
    );

    let resp = send(&agent, "read_file", r#"{"path":"a/b.txt"}"#).await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert_eq!(p.result, "hello guest!");

    // 边界外拒绝（绝对路径指到临时目录外）。
    let outside = if cfg!(windows) {
        r"C:\Windows\evil.txt"
    } else {
        "/etc/evil.txt"
    };
    let resp = send(
        &agent,
        "write_file",
        &serde_json::json!({"path": outside, "content": "x"}).to_string(),
    )
    .await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert!(!p.ok && p.error.contains("outside the workspace"), "{resp}");

    let resp = send(&agent, "delete_file", r#"{"path":"a/b.txt"}"#).await;
    let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
    assert!(p.ok && p.result.contains("deleted file"), "{resp}");
    assert!(!dir.path().join("a/b.txt").exists());
}

#[tokio::test]
async fn workspace_boundary_symlink_escape_denied() {
    // symlink 逃逸：workspace 内 symlink 指向外面 → 写穿透被拒。
    // Windows 上建 symlink 需要特权，skip（CI 本地等价覆盖在 Linux）。
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("esc")).expect("symlink");
        let agent = GuestAgent::new(dir.path().to_path_buf());
        let resp = agent
            .serve_line(&v2_line(
                "write_file",
                r#"{"path":"esc/evil.txt","content":"x"}"#,
            ))
            .await;
        let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
        assert!(!p.ok && p.error.contains("outside the workspace"), "{resp}");
    }
    #[cfg(windows)]
    {
        let (agent, _d) = agent_with_tmp();
        let resp = agent
            .serve_line(&v2_line(
                "exec",
                r#"{"command":"echo hi","cwd":"..\\\\.."}"#,
            ))
            .await;
        let p: ExecutorResponseWire = serde_json::from_str(resp.trim()).unwrap();
        assert!(!p.ok && p.error.contains("outside the workspace"), "{resp}");
    }
}

#[tokio::test]
async fn resolve_rejects_nonexistent_absolute_outside() {
    let ws = PathBuf::from("/nonexistent-workspace-anchor");
    // workspace 本身不存在：整个链不可解析 → 原样判前缀（拒绝绝对外部路径）。
    let r = resolve_in_workspace(&ws, "/etc/passwd");
    assert!(r.is_err());
}

// ---------------------------------------------------------------------------
// 首帧 token 挑战鉴权（第 2 步；duplex 流驱动 serve_stream——TCP 形态
// 的同语义进程内驱动，不占端口）
// ---------------------------------------------------------------------------

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// 起 agent serve_stream 于 duplex 一侧，返回对端 + task 句柄（服务端半
/// 随 serve_stream 返回而 drop——拒绝路径的对端 EOF 语义由此而来）。
async fn served(agent: GuestAgent) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<()>) {
    let (server_side, client_side) = tokio::io::duplex(4096);
    let handle = tokio::spawn(async move {
        let _ = agent.serve_stream(server_side).await;
    });
    (client_side, handle)
}

async fn read_resp(reader: &mut BufReader<tokio::io::DuplexStream>) -> ExecutorResponseWire {
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read resp line");
    serde_json::from_str(line.trim()).expect("parse resp")
}

#[tokio::test]
async fn auth_correct_token_then_tool_calls_flow() {
    let (agent, _dir) = agent_with_tmp();
    let agent = agent.with_token("tok-123");
    let (mut client, _task) = served(agent).await;

    client
        .write_all(b"{\"v\":2,\"auth\":\"tok-123\"}\n")
        .await
        .unwrap();
    let mut reader = BufReader::new(client);
    let resp = read_resp(&mut reader).await;
    assert!(resp.ok, "正确 token 应放行: {resp:?}");

    // 放行后照常协议派发。
    let req = v2_line("list_dir", "{}");
    reader.get_mut().write_all(req.as_bytes()).await.unwrap();
    let resp = read_resp(&mut reader).await;
    assert!(resp.ok, "鉴权后工具调用应正常: {resp:?}");
}

#[tokio::test]
async fn auth_wrong_token_rejected_and_connection_closed() {
    let (agent, _dir) = agent_with_tmp();
    let agent = agent.with_token("tok-123");
    let (mut client, _task) = served(agent).await;
    client
        .write_all(b"{\"v\":2,\"auth\":\"wrong\"}\n")
        .await
        .unwrap();
    let mut reader = BufReader::new(client);
    let resp = read_resp(&mut reader).await;
    assert!(
        !resp.ok && resp.error.contains("token mismatch"),
        "{resp:?}"
    );
    // 拒绝后连接关闭：read_to_end 到 EOF 收敛（服务端半随 serve_stream
    // 返回而 drop）。若连接不关，此读永远不返回——测试超时即失败面。
    let mut eof = Vec::new();
    reader
        .read_to_end(&mut eof)
        .await
        .expect("eof after reject");
}

#[tokio::test]
async fn auth_required_but_first_frame_not_auth_rejected() {
    let (agent, _dir) = agent_with_tmp();
    let agent = agent.with_token("tok-123");
    let (mut client, _task) = served(agent).await;
    // 首帧直接发工具请求（绕过鉴权）→ 拒绝。
    let req = v2_line("list_dir", "{}");
    client.write_all(req.as_bytes()).await.unwrap();
    let mut reader = BufReader::new(client);
    let resp = read_resp(&mut reader).await;
    assert!(!resp.ok && resp.error.contains("auth required"), "{resp:?}");
}

#[tokio::test]
async fn auth_gate_off_tolerates_auth_frame_and_requests() {
    let (agent, _dir) = agent_with_tmp();
    // 未配置 token（裸跑/宿主直跑形态）。
    let (mut client, _task) = served(agent).await;
    client
        .write_all(b"{\"v\":2,\"auth\":\"whatever\"}\n")
        .await
        .unwrap();
    let mut reader = BufReader::new(client);
    let resp = read_resp(&mut reader).await;
    assert!(resp.ok, "闸关时应容忍鉴权帧: {resp:?}");
    let req = v2_line("list_dir", "{}");
    reader.get_mut().write_all(req.as_bytes()).await.unwrap();
    let resp = read_resp(&mut reader).await;
    assert!(resp.ok, "闸关时工具调用照常: {resp:?}");
}

#[tokio::test]
async fn auth_gate_off_first_frame_as_request_still_works() {
    // 闸关且首帧是普通请求（老客户端形态）：不破兼容。
    let (agent, _dir) = agent_with_tmp();
    let (mut client, _task) = served(agent).await;
    let req = v2_line("list_dir", "{}");
    client.write_all(req.as_bytes()).await.unwrap();
    let mut reader = BufReader::new(client);
    let resp = read_resp(&mut reader).await;
    assert!(resp.ok, "{resp:?}");
}

// ---------------------------------------------------------------------------
// 巨帧闸（读侧无界 read_line OOM 防线的 guest 半边）
// ---------------------------------------------------------------------------

/// 跨 crate 锁：帧上限 ≥ bulk_write 最大合法请求行（8MiB raw base64 +
/// JSON 包裹开销）——正常大载荷通道不得被闸误伤（bulk 通道经同一
/// serve_stream 入口进 guest）。
#[test]
fn max_frame_covers_bulk_worst_case_line() {
    let worst = bulk::BULK_CHUNK_MAX_BYTES.div_ceil(3) * 4 + 4096;
    assert!(
        nemesis_exec_protocol::MAX_FRAME_BYTES >= worst,
        "MAX_FRAME_BYTES={} < bulk 最坏请求行 {worst}",
        nemesis_exec_protocol::MAX_FRAME_BYTES
    );
}

/// 巨帧（超限无换行）= 一行诚实错误 + 连接收尾（EOF 收敛）——不是无界
/// read_line 把 guest 读侧内存读爆。首帧即溢出走 auth_gate 同一闸。
#[tokio::test]
async fn serve_stream_rejects_oversized_first_frame_with_error_line_then_eof() {
    let (agent, _dir) = agent_with_tmp();
    let (server_side, mut client) = tokio::io::duplex(1024 * 1024);
    let handle = tokio::spawn(async move { agent.serve_stream(server_side).await });
    let chunk = vec![b'a'; 1024 * 1024];
    for _ in 0..=(nemesis_exec_protocol::MAX_FRAME_BYTES / chunk.len()) {
        // 溢出判定后 guest 收尾 drop，写侧断管是预期收尾。
        if client.write_all(&chunk).await.is_err() {
            break;
        }
    }
    let _ = client.flush().await;
    let mut reader = BufReader::new(client);
    let resp = read_resp(&mut reader).await;
    assert!(!resp.ok, "巨帧必须被拒: {resp:?}");
    assert!(
        resp.error.contains("MAX_FRAME_BYTES"),
        "错误面必须点名帧闸: {resp:?}"
    );
    // 连接收尾：EOF 收敛（读不到 EOF = 收尾失败，测试超时即失败面）。
    let mut eof = Vec::new();
    reader
        .read_to_end(&mut eof)
        .await
        .expect("eof after overflow");
    let _ = handle.await.unwrap();
}

/// 溢出发生在鉴权之后的请求循环同样被闸：小鉴权帧放行后，巨帧请求
/// 得到错误行并收尾。
#[tokio::test]
async fn serve_stream_rejects_oversized_request_after_auth() {
    let (agent, _dir) = agent_with_tmp();
    let (server_side, mut client) = tokio::io::duplex(1024 * 1024);
    let handle = tokio::spawn(async move { agent.serve_stream(server_side).await });
    client
        .write_all(b"{\"v\":2,\"auth\":\"whatever\"}\n")
        .await
        .unwrap();
    let chunk = vec![b'b'; 1024 * 1024];
    for _ in 0..=(nemesis_exec_protocol::MAX_FRAME_BYTES / chunk.len()) {
        if client.write_all(&chunk).await.is_err() {
            break;
        }
    }
    let _ = client.flush().await;
    let mut reader = BufReader::new(client);
    // 第一行 = 鉴权 ok。
    let resp = read_resp(&mut reader).await;
    assert!(resp.ok, "闸关时鉴权帧照常容忍: {resp:?}");
    // 第二行 = 巨帧错误行。
    let resp = read_resp(&mut reader).await;
    assert!(
        !resp.ok && resp.error.contains("MAX_FRAME_BYTES"),
        "{resp:?}"
    );
    let mut eof = Vec::new();
    reader
        .read_to_end(&mut eof)
        .await
        .expect("eof after overflow");
    let _ = handle.await.unwrap();
}

// ===========================================================================
// workspace 边界 `..` 逃逸矩阵（R837 实证固化）：防锚定子代理曾推断
// 「canonicalize 全链失败 -> 纯词法 fallback 放行 -> 写工具物化逃逸」，
// 实测否定——爬升循环在 ws/x/../.. 处 canonicalize 成功（内核弹升 ..
// 不需要中间组件存在），锚点重拼后词法判外拒绝；fallback 臂在本平台
// 语义下不可达。本矩阵钉住候选形态的拒绝行为与物理落盘真值。
// ===========================================================================
#[test]
fn resolve_dotdot_escape_matrix_denied_and_clamped() {
    let outer = tempfile::tempdir().expect("outer tempdir");
    let ws = outer.path().join("ws");
    std::fs::create_dir_all(&ws).expect("mkdir ws");

    // 逃逸候选：resolve 必须 DENIED（锚点重拼后词法判外）。
    let denied_cases = [
        "a/../../p1.txt",      // 单不存在组件 + 单弹升
        "s/a/../../../p2.txt", // 双不存在组件 + 双弹升
        "../p3.txt",           // 直接弹升
    ];
    for path in denied_cases {
        let err = resolve_in_workspace(&ws, path).expect_err(path);
        assert!(err.contains("outside the workspace"), "{path}: {err}");
    }
    // 绝对路径形态（ws 前缀 + 不存在组件 + 弹升逃出 ws）。
    let abs_case = format!("{}/a/../../p4.txt", ws.display());
    let err = resolve_in_workspace(&ws, &abs_case).expect_err(&abs_case);
    assert!(err.contains("outside the workspace"), "{abs_case}: {err}");

    // 钳制形态：canonicalize 锚点吸收弹升 -> 返回 ws 内真实路径
    // （物理安全方向）。断言写工具完整行为（resolve -> create_dir_all
    // -> 落盘）后文件物理位置留在 ws 内。
    std::fs::create_dir_all(ws.join("exist/sub")).expect("mkdir exist/sub");
    let ws_canon = ws.canonicalize().expect("canon ws");
    for path in ["exist/../p5.txt", "exist/sub/../../p6.txt"] {
        let p = resolve_in_workspace(&ws, path).expect(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&p, b"x").expect("write");
        assert!(
            p.canonicalize()
                .expect("canon landed")
                .starts_with(&ws_canon),
            "钳制形态 {path} 物理落盘应留在 ws 内: {p:?}"
        );
    }

    // 物理真值清点：ws 外（outer 根）零落盘。
    let mut entries: Vec<String> = std::fs::read_dir(outer.path())
        .expect("read outer")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["ws".to_string()], "ws 外零外泄: {entries:?}");
}

// 显式 env 透传（模块文档「差异」节声明的语义；此前实现缺失此消费点，
// 模型带 env 调 guest exec 时变量被静默丢弃）。
#[tokio::test]
async fn exec_explicit_env_is_passed_through() {
    let (agent, _dir) = agent_with_tmp();
    let args = if cfg!(windows) {
        r#"{"command":"echo %GUEST_PROBE_VAR%","env":{"GUEST_PROBE_VAR":"val-42"}}"#
    } else {
        r#"{"command":"echo $GUEST_PROBE_VAR","env":{"GUEST_PROBE_VAR":"val-42"}}"#
    };
    let resp = agent.serve_line(&v2_line("exec", args)).await;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim()).expect("parse resp");
    assert!(parsed.ok, "{resp}");
    assert!(parsed.result.contains("val-42"), "env 未透传: {resp}");
}

// accept 瞬态错误分类：连接级抖动与资源耗尽族重试，其余传播退出。
#[test]
fn transient_accept_errors_classified() {
    use std::io::Error;
    let mk = |k: std::io::ErrorKind| Error::new(k, "test");
    assert!(super::transient_accept_error(&mk(
        std::io::ErrorKind::ConnectionAborted
    )));
    assert!(super::transient_accept_error(&mk(
        std::io::ErrorKind::ConnectionReset
    )));
    assert!(super::transient_accept_error(&mk(
        std::io::ErrorKind::Interrupted
    )));
    // listener 本体坏（kind 不在瞬态表）-> 传播。
    assert!(!super::transient_accept_error(&mk(
        std::io::ErrorKind::BrokenPipe
    )));
    // 资源耗尽族按 raw OS 错误码判定（musl/glibc 同值）。
    #[cfg(unix)]
    {
        for code in [24, 23, 12, 105] {
            assert!(
                super::transient_accept_error(&Error::from_raw_os_error(code)),
                "code {code} 应瞬态"
            );
        }
        assert!(super::resource_exhausted_accept(&Error::from_raw_os_error(
            24
        )));
        assert!(!super::resource_exhausted_accept(
            &Error::from_raw_os_error(13)
        ));
    }
}
