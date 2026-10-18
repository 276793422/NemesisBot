//! QMP 客户端（QEMU Machine Protocol over TCP，P5 第 2 步）。
//!
//! 面向本仓 lifecycle 的最小 QMP 面：连接 → 握手（greeting +
//! `qmp_capabilities`）→ `execute`（带超时）→ `query-status` / `quit`。
//! 真实 QEMU 会在响应之间夹异步事件行（`{"event":...}`）——读取循环
//! 跳过事件行直到等到响应行（fake-qmp 不发事件，真实 QEMU 发；跳过
//! 逻辑用纯函数 [`classify_line`] 钉死，CI 可测）。
//!
//! 传输形态：`-qmp tcp:127.0.0.1:<port>,server=on,wait=off`——QMP 无鉴权
//! 机制，绑 127.0.0.1 是唯一的暴露面控制（本地其他进程可达 = 与 hostfwd
//! 同级的诚实边界；QMP 只承载生命周期控制，不承载工作负载字节）。

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// QMP 交互错误（诚实分类：连接失败 / 超时 / 协议形态错 / IO）。
#[derive(Debug)]
pub enum QmpError {
    /// 连接建立失败（QEMU 未起 / 端口错 / 端口被占）。
    Connect(String),
    /// 操作超时（QEMU 挂起 / 卡死——不挂死调用方）。
    Timeout,
    /// 协议形态错（QEMU 回了不认识的行 / 命令被拒）。
    Protocol(String),
    /// 底层 IO 错（连接被杀等——`Kill` QEMU 后的诚实错误面）。
    Io(std::io::Error),
}

impl std::fmt::Display for QmpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QmpError::Connect(m) => write!(f, "QMP connect failed: {m}"),
            QmpError::Timeout => write!(f, "QMP timeout (no response in budget)"),
            QmpError::Protocol(m) => write!(f, "QMP protocol error: {m}"),
            QmpError::Io(e) => write!(f, "QMP io error: {e}"),
        }
    }
}

impl From<std::io::Error> for QmpError {
    fn from(e: std::io::Error) -> Self {
        QmpError::Io(e)
    }
}

/// 单行分类结果（纯函数 [`classify_line`] 的产出）。
#[derive(Debug, Clone, PartialEq)]
pub enum QmpLine {
    /// 连接问候（`{"QMP":{...}}`，每连接一次）。
    Greeting,
    /// 异步事件（`{"event":...}`——等待响应时跳过）。
    Event,
    /// 命令响应：`Ok(return 值)` 或 `Err((error class, desc))`。
    Response(Result<serde_json::Value, (String, String)>),
}

/// 单行分类（纯函数，单测钉死；未知形态 = Err 带原文现场）。
pub fn classify_line(line: &str) -> Result<QmpLine, String> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("bad json line: {e} ({line})"))?;
    if v.get("QMP").is_some() {
        return Ok(QmpLine::Greeting);
    }
    if v.get("event").is_some() {
        return Ok(QmpLine::Event);
    }
    if let Some(ret) = v.get("return") {
        return Ok(QmpLine::Response(Ok(ret.clone())));
    }
    if let Some(err) = v.get("error") {
        let class = err["class"].as_str().unwrap_or("<no class>").to_string();
        let desc = err["desc"].as_str().unwrap_or("<no desc>").to_string();
        return Ok(QmpLine::Response(Err((class, desc))));
    }
    Err(format!("unrecognized QMP line: {line}"))
}

/// QMP 客户端（一条 TCP 连接一个实例；QMP server 每连接独立会话，多次
/// 控制操作各起新连接即可——[`crate::vm::lifecycle`] 的用法）。
pub struct QmpClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    write: tokio::net::tcp::OwnedWriteHalf,
}

impl QmpClient {
    /// 连接 + 完整握手（greeting → `qmp_capabilities` → 应答）。
    /// `timeout` 约束连接与握手每一读（每操作超时，非总预算——文档见上）。
    pub async fn connect(addr: SocketAddr, timeout: Duration) -> Result<Self, QmpError> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| QmpError::Timeout)?
            .map_err(|e| QmpError::Connect(format!("{addr}: {e}")))?;
        let (read, write) = stream.into_split();
        let mut this = Self {
            reader: BufReader::new(read),
            write,
        };
        // greeting（容忍事件行插队——真实 QEMU 握手期不发，分类统一处理）。
        let greeting = this.read_until_response(timeout).await?;
        match greeting {
            Some(QmpLine::Greeting) => {}
            Some(_) => {
                return Err(QmpError::Protocol(
                    "expected QMP greeting, got response".into(),
                ));
            }
            None => return Err(QmpError::Protocol("connection closed at greeting".into())),
        }
        this.execute_raw("qmp_capabilities", None, timeout)
            .await?
            .map_err(|(c, d)| QmpError::Protocol(format!("qmp_capabilities failed: {c}: {d}")))?;
        Ok(this)
    }

    /// 读行直到拿到响应行（跳过事件行）。`None` = 对端关闭。
    async fn read_until_response(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<QmpLine>, QmpError> {
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(timeout, self.reader.read_line(&mut line))
                .await
                .map_err(|_| QmpError::Timeout)?
                .map_err(QmpError::Io)?;
            if n == 0 {
                return Ok(None);
            }
            let classified =
                classify_line(line.trim_end_matches(['\n', '\r'])).map_err(QmpError::Protocol)?;
            match classified {
                QmpLine::Response(r) => {
                    return Ok(Some(QmpLine::Response(r)));
                }
                QmpLine::Greeting => return Ok(Some(QmpLine::Greeting)),
                QmpLine::Event => {
                    tracing::debug!("[qmp] event skipped");
                }
            }
        }
    }

    /// 执行 QMP 命令（带参数），返回 `return` 值；命令被拒 = Err 带
    /// class+desc。`timeout` 约束写与读。
    pub async fn execute(
        &mut self,
        cmd: &str,
        args: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value, QmpError> {
        self.execute_raw(cmd, args, timeout)
            .await?
            .map_err(|(c, d)| QmpError::Protocol(format!("{cmd} rejected: {c}: {d}")))
    }

    /// 原样执行：Ok = 响应行（return 或 error 都算「拿到了响应」）。
    async fn execute_raw(
        &mut self,
        cmd: &str,
        args: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<Result<serde_json::Value, (String, String)>, QmpError> {
        let mut msg = serde_json::json!({ "execute": cmd });
        if let Some(a) = args {
            msg["arguments"] = a;
        }
        let mut line = msg.to_string();
        line.push('\n');
        tokio::time::timeout(timeout, self.write.write_all(line.as_bytes()))
            .await
            .map_err(|_| QmpError::Timeout)?
            .map_err(QmpError::Io)?;
        tokio::time::timeout(timeout, self.write.flush())
            .await
            .map_err(|_| QmpError::Timeout)?
            .map_err(QmpError::Io)?;

        match self.read_until_response(timeout).await? {
            Some(QmpLine::Response(r)) => Ok(r),
            _ => Err(QmpError::Protocol(format!(
                "{cmd}: connection closed before response"
            ))),
        }
    }

    /// `query-status` → 客户机状态串（"running" / "paused" / ...）。
    pub async fn query_status(&mut self, timeout: Duration) -> Result<String, QmpError> {
        let ret = self.execute("query-status", None, timeout).await?;
        Ok(ret["status"].as_str().unwrap_or("<no status>").to_string())
    }

    /// HMP 桥（`human-monitor-command`）：savevm/loadvm 只有 HMP 形态，
    /// QMP 侧经此执行。**HMP 失败不回 error 对象**——错误以文本出现在
    /// `return` 字符串里（首非空行 `Error` 开头），这里诚实转
    /// [`QmpError::Protocol`] 并带全文现场（判定纯函数
    /// [`hmp_output_is_error`]，单测钉死）。
    pub async fn human_monitor(
        &mut self,
        cmdline: &str,
        timeout: Duration,
    ) -> Result<String, QmpError> {
        let ret = self
            .execute(
                "human-monitor-command",
                Some(serde_json::json!({ "command-line": cmdline })),
                timeout,
            )
            .await?;
        let output = ret.as_str().unwrap_or_default().to_string();
        if hmp_output_is_error(&output) {
            return Err(QmpError::Protocol(format!(
                "HMP `{cmdline}` failed: {output}"
            )));
        }
        Ok(output)
    }

    /// `migrate` 到 `uri`（P5 sleep 备选线载体：`file:<path>`）。返回 =
    /// 迁移任务已被受理（非完成）——完成轮询用 [`Self::query_migrate`]。
    pub async fn migrate(&mut self, uri: &str, timeout: Duration) -> Result<(), QmpError> {
        self.execute("migrate", Some(serde_json::json!({ "uri": uri })), timeout)
            .await?;
        Ok(())
    }

    /// `query-migrate` → 迁移任务状态串（"active" / "completed" /
    /// "failed" / ...）。
    pub async fn query_migrate(&mut self, timeout: Duration) -> Result<String, QmpError> {
        let ret = self.execute("query-migrate", None, timeout).await?;
        Ok(ret["status"].as_str().unwrap_or("<no status>").to_string())
    }

    /// `quit`：请求 QEMU 干净退出（lifecycle stop 的优雅路径）。
    pub async fn quit(&mut self, timeout: Duration) -> Result<(), QmpError> {
        self.execute("quit", None, timeout).await?;
        Ok(())
    }
}

/// HMP 输出的失败判定（纯函数，单测钉死）：HMP 把错误写进 monitor 输出
/// 文本（典型形态 `Error: <原因>`，QEMU 多版本一致），首非空行 `Error:`
/// 开头即失败。诚实边界：判定面只服务本仓固定 HMP 命令
/// （savevm/loadvm/info migrate），不试图通解任意 HMP 文本协议；
/// `Error:` 带冒号而非裸 `Error` 前缀——`Errors occurred`（复数名词）
/// 不是错误行，裸前缀会误伤。
pub fn hmp_output_is_error(output: &str) -> bool {
    output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .is_some_and(|first| first.starts_with("Error:"))
}

#[cfg(test)]
mod tests;
