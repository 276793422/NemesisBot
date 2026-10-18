//! fake QMP server（P5 第 1 步 mock 层，2026-10-17）。
//!
//! QEMU QMP socket 的**确定性桩**：同一 JSON 行协议（greeting →
//! qmp_capabilities → execute/response），可编程应答行为——savevm/migrate/
//! query-status 桩应答 + 错误注入 + 挂起（hang）注入 + 断连（reset）注入 +
//! 延迟注入 + 已收命令记录。第 2 步起 nemesis-sandbox vm 的生命周期代码全部对它开发与
//! CI（云端 runner 无嵌套虚拟化——VM 相关测试必须能用纯用户态 mock 跑）。
//!
//! 不是真 QEMU：不发事件、不模拟机器状态机（query-status 恒回配置态）。
//! 诚实边界——它验证「宿主侧代码对 QMP 协议的使用正确性」，不验证
//! QEMU 行为本身（真启动裁决归第 2 步真机验证）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// 单命令的应答行为（可编程桩核心）。
#[derive(Clone, Debug)]
pub enum Behavior {
    /// `{"return": <value>}`（`Value::Null` → `{}`）。
    Return(serde_json::Value),
    /// `{"error": {"class": ..., "desc": ...}}`。
    Error { class: String, desc: String },
    /// 永不应答（连接保持打开）——客户端超时路径测试。
    Hang,
    /// 收到命令后直接断连不回包——建模 QEMU `quit` 停机拆除竞速（SHUTDOWN
    /// 事件后 close 带未读尾部 `\n` → 内核 abort 路径，quit 回包被 RST 取
    /// 代；2026-10-18 远程 KVM savevm 误报失败根因的 CI 形态）。
    Reset,
    /// 延迟后执行内层行为——超时/并发路径测试。
    Delay(Duration, Box<Behavior>),
}

impl Behavior {
    pub fn ok() -> Self {
        Behavior::Return(serde_json::json!({}))
    }
    pub fn err(class: &str, desc: &str) -> Self {
        Behavior::Error {
            class: class.to_string(),
            desc: desc.to_string(),
        }
    }
}

/// 面板默认应答表（QEMU 口径的桩形态——真实 QEMU 对这些命令返回同形）。
fn default_responses() -> HashMap<String, Behavior> {
    let mut m = HashMap::new();
    m.insert("qmp_capabilities".into(), Behavior::ok());
    m.insert(
        "query-status".into(),
        Behavior::Return(serde_json::json!({
            "status": "running", "singlestep": false, "running": true
        })),
    );
    m.insert("savevm".into(), Behavior::ok());
    m.insert("loadvm".into(), Behavior::ok());
    // HMP 桥（savevm/loadvm 只有 HMP 形态，真 QEMU 经 human-monitor-command
    // 执行；成功时 return = HMP 输出文本，savevm 成功输出为空串）。
    m.insert(
        "human-monitor-command".into(),
        Behavior::Return(serde_json::Value::String(String::new())),
    );
    m.insert("stop".into(), Behavior::ok());
    m.insert("cont".into(), Behavior::ok());
    m.insert("migrate".into(), Behavior::Return(serde_json::json!({})));
    m.insert(
        "query-migrate".into(),
        Behavior::Return(serde_json::json!({"status": "completed"})),
    );
    m.insert("migrate-set-capabilities".into(), Behavior::ok());
    m.insert("quit".into(), Behavior::ok());
    m
}

/// 已收命令记录 + 关停句柄。
#[derive(Clone)]
pub struct FakeQmpHandle {
    addr: std::net::SocketAddr,
    commands: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<tokio::sync::Notify>,
    /// accept 循环退出且 listener 已 drop 的信号（`notify_one` 存 permit——
    /// 关停先于等待发生也不丢）。
    closed: Arc<tokio::sync::Notify>,
    /// override 与 default 表合并后的可编程应答（运行时可改）。
    responses: Arc<Mutex<HashMap<String, Behavior>>>,
}

impl FakeQmpHandle {
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// 服务器已收到的 execute 命令名（按到达顺序；含 arguments 摘要）。
    pub fn commands(&self) -> Vec<String> {
        self.commands
            .lock()
            .expect("fake-qmp commands lock")
            .clone()
    }

    /// 运行时改某命令的应答（下一连接生效；测试内错误注入入口）。
    pub fn set_behavior(&self, command: &str, behavior: Behavior) {
        self.responses
            .lock()
            .expect("fake-qmp responses lock")
            .insert(command.to_string(), behavior);
    }

    /// 关停服务器（accept 循环退出 + **listener 真正关闭**后才返回——
    /// 后续 connect 必被拒绝，确定性；无 20ms 竞窗）。
    pub async fn shutdown(&self) {
        // notify_one 存 permit（notify_waiters 在 accept 循环正忙于
        // listener.accept() 分支时会丢通知——关停失效竞窗，实证于
        // dead_server 回归）：select! 下一拍 poll notified() 必消费。
        self.shutdown.notify_one();
        // 等 closed 信号（notify_one 存 permit，先通知后等待也不丢）。
        let _ = tokio::time::timeout(Duration::from_secs(2), self.closed.notified()).await;
    }
}

/// 启动 fake QMP（bind 127.0.0.1:0 临时端口）。返回句柄（含实际地址）。
pub async fn spawn() -> std::io::Result<FakeQmpHandle> {
    spawn_on("127.0.0.1:0").await
}

/// 启动 fake QMP（显式 bind 地址）。
pub async fn spawn_on(bind: &str) -> std::io::Result<FakeQmpHandle> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let commands = Arc::new(Mutex::new(Vec::new()));
    let responses = Arc::new(Mutex::new(default_responses()));
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let closed = Arc::new(tokio::sync::Notify::new());

    let accept_shutdown = shutdown.clone();
    let accept_closed = closed.clone();
    let accept_commands = commands.clone();
    let accept_responses = responses.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = accept_shutdown.notified() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let commands = accept_commands.clone();
                    let responses = accept_responses.clone();
                    tokio::spawn(async move {
                        serve_connection(stream, commands, responses).await;
                    });
                }
            }
        }
        // listener 先 drop 再发信号——shutdown() 返回后端口确定拒绝连接。
        drop(listener);
        accept_closed.notify_one();
    });

    Ok(FakeQmpHandle {
        addr,
        commands,
        shutdown,
        closed,
        responses,
    })
}

/// QMP greeting（QEMU 同形：版本 + 空能力表）。
fn greeting() -> String {
    let g = serde_json::json!({
        "QMP": {
            "version": {
                "qemu": {"micro": 0, "minor": 15, "major": 8},
                "package": "fake-qmp (nemesisbot P5 test tool)"
            },
            "capabilities": []
        }
    });
    format!("{g}\n")
}

async fn serve_connection(
    stream: TcpStream,
    commands: Arc<Mutex<Vec<String>>>,
    responses: Arc<Mutex<HashMap<String, Behavior>>>,
) {
    let (reader_half, mut writer_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader_half);

    // 先发 greeting（QMP 协议：服务端连接即发）。
    if writer_half.write_all(greeting().as_bytes()).await.is_err() {
        return;
    }

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return, // 客户端断开 = 正常收尾
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            // QEMU 对坏 JSON 回 GenericError。
            let err = serde_json::json!({
                "error": {"class": "GenericError", "desc": "Invalid JSON syntax"}
            });
            let _ = writer_half.write_all(format!("{err}\n").as_bytes()).await;
            continue;
        };

        let Some(cmd) = msg
            .get("execute")
            .and_then(|v| v.as_str())
            .map(String::from)
        else {
            let err = serde_json::json!({
                "error": {"class": "GenericError", "desc": "Expected 'execute' in query"}
            });
            let _ = writer_half.write_all(format!("{err}\n").as_bytes()).await;
            continue;
        };

        commands
            .lock()
            .expect("fake-qmp commands lock")
            .push(cmd.clone());

        let behavior = responses
            .lock()
            .expect("fake-qmp responses lock")
            .get(&cmd)
            .cloned()
            .unwrap_or_else(|| {
                Behavior::err(
                    "CommandNotFound",
                    &format!("The command {cmd} has not been found"),
                )
            });

        let payload = match behavior {
            Behavior::Hang => {
                // 永不应答：等客户端断开（读半已在循环里，这里直接继续
                // 外层循环等下一行/EOF——挂起语义 = 不写响应）。
                continue;
            }
            Behavior::Reset => {
                // 断连不回包（建模 QEMU quit 拆除竞速）：return 丢弃
                // reader/writer 两半 → 客户端读到对端关闭。
                return;
            }
            Behavior::Delay(d, inner) => {
                tokio::time::sleep(d).await;
                *inner
            }
            other => other,
        };
        let resp = match payload {
            Behavior::Return(v) => {
                let v = if v.is_null() {
                    serde_json::json!({})
                } else {
                    v
                };
                serde_json::json!({"return": v})
            }
            Behavior::Error { class, desc } => {
                serde_json::json!({"error": {"class": class, "desc": desc}})
            }
            _ => serde_json::json!({"return": {}}),
        };
        if writer_half
            .write_all(format!("{resp}\n").as_bytes())
            .await
            .is_err()
        {
            return;
        }
    }
}

/// 阻塞式独立运行（bin 入口用；库消费方请用 [`spawn`]）。
pub async fn serve_forever(bind: &str) -> std::io::Result<()> {
    let handle = spawn_on(bind).await?;
    eprintln!("[fake-qmp] listening {}", handle.addr());
    // 常驻：等 shutdown 信号（bin 里永远等不到，靠进程退出）。
    loop {
        handle.shutdown.notified().await;
    }
}
