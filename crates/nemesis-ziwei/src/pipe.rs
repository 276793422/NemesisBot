//! pipe —— bot 侧具名管道 SERVER + 帧协议客户端（T1.2/T1.5）。
//!
//! 职责反转（决议：bot 是 server，payload 是 client）——bot 无法预知目标内
//! LoadLibrary 时机，server 侧被动等连接 + 首帧握手最稳。
//!
//! 安全面（总控 §7）：
//! - **PID 验证**：`GetNamedPipeClientProcessId` == 目标 PID（防外来进程抢连——
//!   管道名含 pid+随机段，但本机任何进程都能试探打开，PID 闸是硬校验）；
//! - **令牌**：注入参数块带一次性 32 hex 令牌，握手首帧回带，不匹配即断；
//! - **协议版本**：`proto` 必须 == [`PROTO_VERSION`]；
//! - **架构**：握手 `arch` 必须与预期一致（防 x86 DLL 注进 x64 目标的错配残留）。
//!
//! 帧协议与 payload 同源：`[u32 LE 长度][JSON UTF-8]`，值域不含转义字符。

use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::time::timeout;

pub use crate::dispatch::PipeError;
use crate::dispatch::{json_get_str, json_get_u64};

/// 管道协议版本（与 payload / 注入器三方同源）。
pub const PROTO_VERSION: u32 = 1;
/// 单帧上限（4MB，与 payload 同源）。
pub const MAX_FRAME: u32 = 4 << 20;
/// 握手等待窗口。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// 握手信息（payload 首帧回带）。
#[derive(Debug, Clone)]
pub struct Handshake {
    pub proto: u32,
    pub token: String,
    pub arch: String,
    pub pid: u32,
}

/// 已完成握手的管道链路（bot ↔ payload 双向帧通道）。
pub struct PipeLink {
    stream: tokio::net::windows::named_pipe::NamedPipeServer,
    pub handshake: Handshake,
}

/// 建管道 server（第一步——必须在注入**之前**落定：payload 连回重试窗口
/// 只有 10s，注入起线程后 server 才建会撞窗口尾部）。
pub fn create_server(
    pipe_name: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, PipeError> {
    ServerOptions::new()
        .first_pipe_instance(true)
        .create(pipe_name)
        .map_err(PipeError::Io)
}

impl PipeLink {
    /// 建 server → 等连接 → PID 验证 → 读握手帧 → 令牌/版本/架构校验。
    pub async fn connect(
        pipe_name: &str,
        expect_token: &str,
        expect_pid: u32,
        expect_arch: &str,
    ) -> Result<PipeLink, PipeError> {
        let server = create_server(pipe_name)?;
        finish_handshake(server, expect_token, expect_pid, expect_arch).await
    }
}

/// 已建 server 上完成连接等待 + 握手校验（session::attach 的 join! 并行臂）。
pub async fn finish_handshake(
    server: tokio::net::windows::named_pipe::NamedPipeServer,
    expect_token: &str,
    expect_pid: u32,
    expect_arch: &str,
) -> Result<PipeLink, PipeError> {
    timeout(HANDSHAKE_TIMEOUT, server.connect())
        .await
        .map_err(|_| PipeError::ConnectTimeout(HANDSHAKE_TIMEOUT))?
        .map_err(PipeError::Io)?;

    // 硬校验：客户端进程必须是目标进程（win32 查询，非 payload 自报）。
    verify_client_pid(server.as_raw_handle(), expect_pid)?;

    // 读握手首帧（窗口内）
    let mut link = PipeLink {
        stream: server,
        handshake: Handshake {
            proto: 0,
            token: String::new(),
            arch: String::new(),
            pid: 0,
        },
    };
    let frame = timeout(HANDSHAKE_TIMEOUT, link.read_frame())
        .await
        .map_err(|_| PipeError::HandshakeTimeout)??;
    let text = String::from_utf8_lossy(&frame).to_string();
    let hs = &mut link.handshake;
    hs.proto = json_get_u64(&text, "proto").unwrap_or(0) as u32;
    hs.token = json_get_str(&text, "token").unwrap_or_default().to_string();
    hs.arch = json_get_str(&text, "arch").unwrap_or_default().to_string();
    hs.pid = json_get_u64(&text, "pid").unwrap_or(0) as u32;

    if hs.token != expect_token {
        return Err(PipeError::TokenMismatch);
    }
    if hs.proto != PROTO_VERSION {
        return Err(PipeError::ProtoMismatch {
            got: hs.proto,
            expect: PROTO_VERSION,
        });
    }
    if hs.pid != expect_pid {
        return Err(PipeError::PidMismatch {
            got: hs.pid,
            expect: expect_pid,
        });
    }
    if hs.arch != expect_arch {
        return Err(PipeError::ArchMismatch {
            got: hs.arch.clone(),
            expect: expect_arch.to_string(),
        });
    }
    Ok(link)
}

impl PipeLink {
    /// 读一帧（长度前缀）。
    pub async fn read_frame(&mut self) -> Result<Vec<u8>, PipeError> {
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => PipeError::Closed,
                _ => PipeError::Io(e),
            })?;
        let len = u32::from_le_bytes(len_buf);
        if len > MAX_FRAME {
            return Err(PipeError::FrameTooBig(len));
        }
        let mut payload = vec![0u8; len as usize];
        if len > 0 {
            self.stream.read_exact(&mut payload).await?;
        }
        Ok(payload)
    }

    /// 写一帧。
    pub async fn write_frame(&mut self, payload: &[u8]) -> Result<(), PipeError> {
        if payload.len() > MAX_FRAME as usize {
            return Err(PipeError::FrameTooBig(payload.len() as u32));
        }
        self.stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .await?;
        self.stream.write_all(payload).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// 请求-响应往返（JSON 文本帧；单请求串行——payload 命令循环也是串行）。
    pub async fn roundtrip(&mut self, req: &str) -> Result<String, PipeError> {
        self.write_frame(req.as_bytes()).await?;
        let frame = self.read_frame().await?;
        Ok(String::from_utf8_lossy(&frame).to_string())
    }
}

/// read_mem：payload 侧 ReadProcessMemory(self)（ntdll SEH 守护）。
/// 地址以裸数字（hex）入帧——payload 的 json_get_u64 只认非引号数值。
pub async fn read_mem(link: &mut PipeLink, addr: u64, len: u32) -> Result<Vec<u8>, PipeError> {
    let req = format!("{{\"cmd\":\"read_mem\",\"addr\":{addr:#x},\"len\":{len}}}");
    let resp = link.roundtrip(&req).await?;
    crate::dispatch::parse_read_mem_resp(&resp)
}

// ---------------------------------------------------------------------------
// M4：call_fn / hook 订阅退订 / 事件拉取（命令循环仍严格请求-响应配对）
// ---------------------------------------------------------------------------

/// call_fn 约定值（payload CONV_* 同源；定义在平台中立 dispatch——re-export）。
pub use crate::dispatch::{CONV_CDECL, CONV_MSX64, CONV_STDCALL};

/// call_fn：payload 在目标内以专用线程调函数（单飞 + 看门狗 + VEH 救援）。
/// 注意：payload 应答前命令循环被占用——调用方 timeout 预算须大于
/// `timeout_ms`（session 层负责加余量）。
pub async fn call_fn(
    link: &mut PipeLink,
    addr: u64,
    conv: u32,
    args: [u64; 4],
    timeout_ms: u32,
) -> Result<crate::dispatch::CallFnOutcome, PipeError> {
    let req = format!(
        "{{\"cmd\":\"call_fn\",\"addr\":{addr:#x},\"conv\":{conv},\
         \"args\":[{},{},{},{}],\"timeout_ms\":{timeout_ms}}}",
        args[0], args[1], args[2], args[3]
    );
    let resp = link.roundtrip(&req).await?;
    crate::dispatch::parse_call_fn_resp(&resp)
}

/// hook_set：目标函数入口补丁（5×0x90 窃取窗口；payload 二次实核）。
pub async fn hook_set(link: &mut PipeLink, id: u32, addr: u64) -> Result<(), PipeError> {
    let req = format!("{{\"cmd\":\"hook_set\",\"id\":{id},\"addr\":{addr:#x}}}");
    let resp = link.roundtrip(&req).await?;
    if resp.contains("\"ok\":false") {
        return Err(PipeError::Payload(
            json_get_str(&resp, "error")
                .unwrap_or("hook_set 失败")
                .to_string(),
        ));
    }
    Ok(())
}

/// hook_unset：还原入口 5×0x90（stub 常驻不回收——payload 侧纪律）。
pub async fn hook_unset(link: &mut PipeLink, addr: u64) -> Result<(), PipeError> {
    let req = format!("{{\"cmd\":\"hook_unset\",\"addr\":{addr:#x}}}");
    let resp = link.roundtrip(&req).await?;
    if resp.contains("\"ok\":false") {
        return Err(PipeError::Payload(
            json_get_str(&resp, "error")
                .unwrap_or("hook_unset 失败")
                .to_string(),
        ));
    }
    Ok(())
}

/// poll_events：批量拉取 hook 事件（bot 主动拉——不引入异步插帧）。
pub async fn poll_events(
    link: &mut PipeLink,
    max: u32,
) -> Result<(Vec<crate::dispatch::HookEvent>, u64), PipeError> {
    let req = format!("{{\"cmd\":\"poll_events\",\"max\":{max}}}");
    let resp = link.roundtrip(&req).await?;
    crate::dispatch::parse_poll_events_resp(&resp)
}

/// win32 查询管道客户端 PID（不信任对端自报）。
fn verify_client_pid(
    handle: std::os::windows::io::RawHandle,
    expect: u32,
) -> Result<(), PipeError> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetNamedPipeClientProcessId(hPipe: *mut core::ffi::c_void, clientPid: *mut u32) -> i32;
    }
    let mut got: u32 = 0;
    let ok = unsafe { GetNamedPipeClientProcessId(handle as *mut _, &mut got) };
    if ok == 0 || got != expect {
        return Err(PipeError::PidMismatch { got, expect });
    }
    Ok(())
}
