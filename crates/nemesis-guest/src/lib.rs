//! P5 guest agent 协议循环库（桌面级 VM 第 1 步，2026-10-17）。
//!
//! VM 内执行体的**协议侧**实现：消费 [`nemesis_exec_protocol`] 的 wire
//! 结构与传输原语，把「一行 v2 请求」变成「一行 v2 响应」（call_id 恒
//! 回显）。工具面是宿主 MOVE_TOOLS 的**子集**（exec + 基础文件工具），
//! 语义对齐宿主同源实现（结果/错误文案同形），让 gateway 侧对「本地
//! executor 子进程」与「VM 内 guest」说同一种话。
//!
//! **协议版本纪律**：guest 只说 v2（VM 通道恒 v2——协议 crate 文档的
//! 分工）。v1 请求 loud 拒绝（带 call_id 回显时错误可关联，半填形态
//! 回空 call_id 错误行）。这与 nemesisbot exec_worker 的「v1/v2 双解析」
//! 不同：exec_worker 是同一二进制自 spawn 自（无版本混布风险，双解析
//! 为测试平滑性），guest 是独立生命周期的新二进制，没有历史包袱。
//!
//! **双栖形态**（同一二进制，`main.rs` 分派）：
//! - 宿主直跑（stdio 循环）：跨平台冒烟 / 集成测试用，无需 VM；
//! - `--listen ADDR:PORT`（VM 内 TCP 服务）：第 2 步 NAT-TCP 通道的
//!   guest 端。accept 循环 = 每连接独立 serve 到 EOF，串行应答（协议
//!   本身严格逐请求应答）。
//!
//! **不进本 crate 的东西**（诚实边界）：
//! - 幂等 journal（P3 persistent 语义）——VM 侧持久化归第 3 步
//!   savevm/wake 一并设计；
//! - workspace 边界的 nemesis-path 归一化（8.3 短名等 Windows 语义）——
//!   guest 栖息地是 musl Linux，std canonicalize 语义即正确语义；
//! - edit_file / multiedit / grep / git 工具——往后续补（子集起步）。

use std::path::{Path, PathBuf};

use nemesis_exec_protocol::{
    ExecutorRequestOwned, ExecutorResponseWire, ExecutorStream, PROTOCOL_V2,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub mod tools;
// B9-1/B9-2 内部方法（bulk_write/bulk_read/bulk_read_chunk/hash_tree/
// bulk_delete）：gateway 同步引擎专用通道（internal_call），**不进 agent
// 工具面**——宿主侧 VM_GUEST_TOOLS 子集闸不认识它们。
pub(crate) mod bulk;

/// guest 工作区边界（`NEMESISBOT_GUEST_WORKSPACE`，缺省 `/workspace`）。
/// VM 内的根 = 挂载共享子树的挂载点（第 2 步接真实 virtiofs/9p 路径）。
pub fn default_workspace() -> PathBuf {
    std::env::var("NEMESISBOT_GUEST_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/workspace"))
}

/// guest agent：协议循环 + 工具派发（不可变，跨连接共享；`Clone` 供
/// accept 循环每连接克隆）。
///
/// **首帧 token 挑战鉴权**（P5 第 2 步，NAT-TCP 通道的访问闸）：配置了
/// token（[`GuestAgent::with_token`]，VM 内经 launcher 从 SMBIOS DMI
/// serial 注入）时，连接的**第一行**必须是鉴权帧 `{"v":2,"auth":"<token>"}`
/// ——对则回 ok 行、错则回错误行并断开。顶层 `"auth"` 键是协议保留键
/// （工具请求永远不带它）。未配置 token = 闸关：容忍鉴权帧（回 ok 继续跑
/// ——宿主侧恒发鉴权帧，裸跑形态不该因此不可用），无鉴权帧照常派发。
#[derive(Clone)]
pub struct GuestAgent {
    workspace: PathBuf,
    token: Option<String>,
    /// bulk 传输状态表（Arc 共享——accept 每连接克隆 agent 后仍指向同一张
    /// 表：一次 bulk_write 的分块可能落在不同连接上，序列跨连接可达）。
    transfers: bulk::TransferMap,
}

impl GuestAgent {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            token: None,
            transfers: bulk::new_transfer_map(),
        }
    }

    /// 启用首帧 token 挑战（NAT-TCP 通道的宿主→guest 访问闸；stdio 直跑
    /// 形态不设——同进程自 spawn 无威胁面）。
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// 服务一行请求 → 一行响应（恒带换行）。协议主体（stdio / TCP 两栖共用）。
    pub async fn serve_line(&self, line: &str) -> String {
        // 行级解析失败（坏 JSON）：call_id 提不出来，回 v1 形态错误行
        // （与 exec_worker 同形——传输层损坏行的语义归 gateway 侧判定）。
        let req: ExecutorRequestOwned = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return serialize(&ExecutorResponseWire::error(&format!(
                    "bad request line: {e}"
                )));
            }
        };

        // v2-only 闸（本 crate 的协议版本纪律，见模块文档）。
        let ExecutorRequestOwned {
            v,
            call_id,
            tool,
            args,
            context: _,
        } = req;
        if v != PROTOCOL_V2 || call_id.is_empty() {
            let mut resp = ExecutorResponseWire::error(&format!(
                "guest agent requires v2 protocol (v={v}, call_id empty={}) — \
                 VM 通道恒走 v2，请用 build_v2_request_line 产出请求",
                call_id.is_empty()
            ));
            resp.call_id = call_id;
            return serialize(&resp);
        }

        let resp = match tools::dispatch(&tool, &args, &self.workspace, &self.transfers).await {
            Ok(result) => ExecutorResponseWire {
                v: PROTOCOL_V2,
                call_id,
                ok: true,
                result,
                error: String::new(),
            },
            Err(error) => ExecutorResponseWire {
                v: PROTOCOL_V2,
                call_id,
                ok: false,
                result: String::new(),
                error,
            },
        };
        serialize(&resp)
    }

    /// 驱动一条双工流直到 EOF（逐行 serve；连接关闭 = 正常收尾）。
    /// 首行走鉴权闸（见结构体文档），后续行走协议派发。
    pub async fn serve_stream<S: ExecutorStream>(&self, stream: S) -> std::io::Result<()> {
        // BufReader 活过整个循环：内部缓冲跨请求保留（每请求新建 reader 会
        // 把缓冲里已收到的后续请求字节随旧 reader 一起丢弃——exec_worker
        // 同款注释的教训，多行一次到达时请求会被吞）。
        let mut reader = BufReader::new(stream);
        if !self.auth_gate(&mut reader).await? {
            return Ok(()); // 鉴权失败：错误行已回、连接关闭（诚实拒绝）
        }
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            if line.is_empty() {
                continue; // 空行静默跳过（TCP 粘包拆包容错）
            }
            let resp_line = self.serve_line(&line).await;
            reader.get_mut().write_all(resp_line.as_bytes()).await?;
            reader.get_mut().flush().await?;
        }
    }

    /// 首帧鉴权闸。返回 false = 拒绝并已回错误行（调用方直接收尾）。
    async fn auth_gate<S: ExecutorStream>(
        &self,
        reader: &mut BufReader<S>,
    ) -> std::io::Result<bool> {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => return Ok(false), // 连了就断：无事可服务
            Ok(_) => {}
            Err(e) => return Err(e),
        }
        let line = line.trim_end_matches(['\n', '\r']).to_string();

        // 是否鉴权帧：顶层 "auth" 键（协议保留键——工具请求不带它）。
        let is_auth_frame = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("auth").cloned())
            .is_some();

        if is_auth_frame {
            let accepted = match &self.token {
                None => true, // 闸关：容忍鉴权帧（宿主恒发，裸跑不该不可用）
                Some(expected) => serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v["auth"].as_str().map(|s| s.to_string()))
                    .map(|got| ct_eq(&got, expected))
                    .unwrap_or(false),
            };
            let resp = if accepted {
                serialize(&ExecutorResponseWire {
                    v: PROTOCOL_V2,
                    call_id: String::new(),
                    ok: true,
                    result: "authorized".to_string(),
                    error: String::new(),
                })
            } else {
                serialize(&ExecutorResponseWire {
                    v: PROTOCOL_V2,
                    call_id: String::new(),
                    ok: false,
                    result: String::new(),
                    error: "auth failed: token mismatch".to_string(),
                })
            };
            reader.get_mut().write_all(resp.as_bytes()).await?;
            reader.get_mut().flush().await?;
            return Ok(accepted);
        }

        // 首帧不是鉴权帧：要求鉴权 = 拒绝；未要求 = 首帧当请求照常派发。
        if self.token.is_some() {
            let resp = serialize(&ExecutorResponseWire {
                v: PROTOCOL_V2,
                call_id: String::new(),
                ok: false,
                result: String::new(),
                error: "auth required: first frame must be {\"v\":2,\"auth\":\"<token>\"}"
                    .to_string(),
            });
            reader.get_mut().write_all(resp.as_bytes()).await?;
            reader.get_mut().flush().await?;
            return Ok(false);
        }
        let resp_line = self.serve_line(&line).await;
        reader.get_mut().write_all(resp_line.as_bytes()).await?;
        reader.get_mut().flush().await?;
        Ok(true)
    }

    /// TCP accept 循环（`--listen` 形态）：每连接独立任务 serve 到 EOF。
    /// 连接间天然并发（NAT-TCP hostfwd 每调用一连接的形态下即 per-call
    /// 隔离），连接内严格串行应答；每连接过首帧鉴权闸（token 随 agent 克隆）。
    pub async fn serve_tcp(self, listener: tokio::net::TcpListener) -> std::io::Result<()> {
        loop {
            let (stream, _peer) = listener.accept().await?;
            let agent = self.clone();
            tokio::spawn(async move {
                if let Err(e) = agent.serve_stream(stream).await {
                    eprintln!("[guest] connection error: {e}");
                }
            });
        }
    }
}

/// 常数时间字符串比较（token 比对用）。XOR 折叠全字节——长度不同立即
/// false（长度本身不是秘密；NAT 127.0.0.1 域的威胁面 = 本地其他进程，
/// 不是远程计时侧信道，常数时间是廉价的正确习惯而非硬需求）。
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn serialize(resp: &ExecutorResponseWire) -> String {
    let mut line = serde_json::to_string(resp).unwrap_or_else(|_| {
        r#"{"ok":false,"result":"","error":"response serialize failed"}"#.into()
    });
    line.push('\n');
    line
}

/// 工作区边界判定（guest 栖息地 = musl Linux 的最小实现）：
/// 相对路径 join workspace；绝对路径向上找最深的已存在祖先做
/// canonicalize（写类工具目标尚不存在时也能判），余下后缀原样拼接，
/// 前缀不在 workspace 内即拒绝。
pub fn resolve_in_workspace(workspace: &Path, path: &str) -> Result<PathBuf, String> {
    let target = Path::new(path);
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        workspace.join(target)
    };

    // 找最深已存在祖先：canonicalize 需要路径存在，目标可能是待创建文件。
    let mut existing = joined.clone();
    let mut suffix = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(canon) => {
                let mut resolved = canon;
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return check_prefix(resolved, workspace, path);
            }
            Err(_) => {
                match (existing.parent(), existing.file_name()) {
                    (Some(p), Some(name)) => {
                        suffix.push(name.to_os_string());
                        existing = p.to_path_buf();
                    }
                    _ => {
                        // 整条链都不可解析（盘符级不存在等）：按原样判前缀。
                        return check_prefix(joined, workspace, path);
                    }
                }
            }
        }
    }
}

fn check_prefix(resolved: PathBuf, workspace: &Path, original: &str) -> Result<PathBuf, String> {
    let ws = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    if resolved.starts_with(&ws) {
        Ok(resolved)
    } else {
        Err(format!(
            "access denied: path '{original}' is outside the workspace"
        ))
    }
}

#[cfg(test)]
mod tests;
