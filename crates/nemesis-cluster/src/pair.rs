//! cluster pair 配对后端（发现②，2026-09-15）。
//!
//! 消灭「手工抄写对端 ID」这个事故环节：用户只给一个对端可达地址，系统
//! 一次 RPC `get_info` 拉取对端**真实 ID**（与 announce/RPC 自报同源
//! `node_id` 字段——单一真相源），代写 peers.toml（表键=字面真实 id），
//! 写后回读断言（最终防线：任何环节出错，错误配置不落盘生效，命令当场
//! 红）。CLI（`nemesisbot cluster pair`）与 WSAPI（`cluster.pair`）共用
//! 本模块——同一后端函数，行为零分叉。
//!
//! 入参端口双形态自动探测：`host:port` 先按字面端口发 RPC，失败试
//! `port+10000`（静态 peers 的 UDP→RPC 端口约定，gateway 装载推导）；
//! 两者都失败才报错，且**零写入**。

use std::path::Path;
use std::time::Duration;

/// 配对成功结果。
#[derive(Debug, Clone)]
pub struct PairOutcome {
    /// 对端真实 ID（peers.toml 表键，字面写入）。
    pub peer_id: String,
    /// 对端显示名（可能为空）。
    pub name: String,
    /// 探测成功的 RPC 地址（host:port，即用户给的可达地址+探测端口）。
    pub rpc_address: String,
    /// 写盘的 UDP 地址（host:udp_port，供 gateway 静态装载推导 rpc+10000）。
    pub udp_address: String,
    /// 对端自报全量地址列表（host 形态；运行期 announce 会继续刷新）。
    pub addresses: Vec<String>,
    /// 探测成功的 RPC 端口。
    pub rpc_port: u16,
    /// true=字面端口直接命中 RPC；false=按 +10000 约定推导命中。
    pub literal_port_was_rpc: bool,
}

/// 配对失败（所有失败形态都不产生任何配置写入——写入前失败零接触，
/// 写入后断言失败回滚）。
#[derive(Debug)]
pub enum PairError {
    /// 双形态探测都不可达：携带尝试过的地址与最后一次底层错误。
    Unreachable {
        tried: Vec<String>,
        last_error: String,
    },
    /// 入参形态非法（缺端口/端口为 0 等）。
    BadAddress(String),
    /// 对端响应缺 node_id 或为空（不可能用于表键）。
    MissingIdentity,
    /// 写后回读断言失败：已回滚本次写入。
    AssertFailed { detail: String },
    /// 本地文件 IO 错误。
    Io(String),
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairError::Unreachable { tried, last_error } => write!(
                f,
                "无法连接对端（试过 {}）——未写入任何配置；最后错误：{}",
                tried.join(", "),
                last_error
            ),
            PairError::BadAddress(msg) => write!(f, "地址形态非法：{}", msg),
            PairError::MissingIdentity => {
                write!(
                    f,
                    "对端 get_info 响应缺少 node_id，无法配对；未写入任何配置"
                )
            }
            PairError::AssertFailed { detail } => {
                write!(f, "写后回读断言失败（已回滚）：{}", detail)
            }
            PairError::Io(e) => write!(f, "本地文件错误：{}", e),
        }
    }
}

impl std::error::Error for PairError {}

/// 单次 `get_info` 探测超时。
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 配对主流程（§2.3 四步 + §2.4-C 回读断言）。
///
/// `peers_path`：目标 peers.toml（调用方按自己的路径真相源解析——CLI 用
/// `<home>/cluster/peers.toml`，WSAPI 用 cluster 实例的 static_config_path）。
/// `token`：RPC 鉴权 token（AEAD）；与对端 `config.cluster.json` 的 token
/// 一致才通——两侧同源约定，与集群运行时同一把钥匙。
///
/// 一次性调用（自带 tokio runtime 也可在已有 runtime 内 await）。
pub async fn pair_with_peer(
    peers_path: &Path,
    token: Option<&str>,
    address: &str,
) -> Result<PairOutcome, PairError> {
    // -- 入参校验 ----------------------------------------------------------
    let address = address.trim();
    let (host, port) = parse_host_port_strict(address)?;
    if host.is_empty() {
        return Err(PairError::BadAddress(format!(
            "「{address}」缺主机名；形态：host:port（UDP 或 RPC 端口皆可，自动探测）"
        )));
    }

    // -- 双形态探测（字面 → +10000），全败零写入 ---------------------------
    let mut tried = Vec::new();
    let mut last_error = String::new();
    let mut probed: Option<(u16, bool, serde_json::Value)> = None;
    for (rpc_port, literal) in [(port, true), (port.saturating_add(10000), false)] {
        let addr = format!("{}:{}", host, rpc_port);
        // 总超时闸：连接 5s + 帧往返，防半开连接挂死。
        let attempt = tokio::time::timeout(PROBE_TIMEOUT, probe_get_info(&addr, token)).await;
        match attempt {
            Ok(Ok(payload)) => {
                probed = Some((rpc_port, literal, payload));
                break;
            }
            Ok(Err(e)) => {
                tried.push(addr);
                last_error = e;
            }
            Err(_) => {
                tried.push(addr);
                last_error = format!("timeout ({}s)", PROBE_TIMEOUT.as_secs());
            }
        }
    }
    let (rpc_port, literal_was_rpc, payload) =
        probed.ok_or(PairError::Unreachable { tried, last_error })?;

    // -- 拉取身份（单一真相源：对端 Cluster 实例 node_id）-------------------
    let peer_id = payload
        .get("node_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if peer_id.is_empty() {
        return Err(PairError::MissingIdentity);
    }
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let role = payload
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("worker");
    let category = payload
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let self_addresses: Vec<String> = payload
        .get("addresses")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .filter(|s| !s.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();

    // -- 写盘（表键=字面真实 id；地址=探测成功的可达地址换算 UDP 形态）------
    // 探测成功的 host 一定从本机可达（刚用它往返过 RPC）——静态兜底地址
    // 用它，不用对端自报 primary（多网卡下可能是本机不可达网段）。
    let rpc_address = format!("{}:{}", host, rpc_port);
    let udp_address = crate::cluster_config::rpc_to_udp_address(&rpc_address);

    // 记录写前内容用于回滚（文件不存在 = 写后失败则删文件）。
    let pre_content = std::fs::read_to_string(peers_path).ok();

    crate::cluster_config::append_peer_to_file_with_name(
        peers_path,
        &peer_id,
        &udp_address,
        role,
        category,
        if name.is_empty() { None } else { Some(&name) },
        rpc_port,
    )
    .map_err(|e| PairError::Io(e.to_string()))?;

    // -- 写后回读断言（最终防线）-------------------------------------------
    if let Err(detail) = assert_pair_written(peers_path, &peer_id, &udp_address, &name, rpc_port) {
        // 回滚：恢复写前内容；原本无文件则删除半成品。
        let rollback = if let Some(content) = &pre_content {
            std::fs::write(peers_path, content).is_err()
        } else {
            std::fs::remove_file(peers_path).is_err()
        };
        return Err(PairError::AssertFailed {
            detail: if rollback {
                format!(
                    "{}（回滚亦失败，请人工检查 {}）",
                    detail,
                    peers_path.display()
                )
            } else {
                detail
            },
        });
    }

    Ok(PairOutcome {
        peer_id,
        name,
        rpc_address,
        udp_address,
        addresses: self_addresses,
        rpc_port,
        literal_port_was_rpc: literal_was_rpc,
    })
}

/// 写后回读断言：重新解析 peers.toml，表键必须**字面等于**拉取的 peer_id，
/// 地址必须等于写盘值，name（若写入）必须一致，rpc_port（探测值 >0 时）
/// 必须一致。任一不过 = 写盘链路某环节出错（序列化/sanitize 残留/并发改写）。
fn assert_pair_written(
    peers_path: &Path,
    peer_id: &str,
    udp_address: &str,
    name: &str,
    rpc_port: u16,
) -> Result<(), String> {
    let content = std::fs::read_to_string(peers_path).map_err(|e| format!("回读失败：{}", e))?;
    let doc: toml::Value = content
        .parse()
        .map_err(|e| format!("回读解析失败：{}", e))?;
    let entry = doc
        .get("peers")
        .and_then(|v| v.get(peer_id))
        .ok_or_else(|| {
            format!(
                "表键 [peers.{}] 未在文件中出现（写入后丢失或被改写）",
                peer_id
            )
        })?;

    let written_addr = entry.get("address").and_then(|v| v.as_str()).unwrap_or("");
    if written_addr != udp_address {
        return Err(format!(
            "表键 [peers.{peer_id}] 地址不匹配：写入 {udp_address}，回读 {written_addr}"
        ));
    }
    if !name.is_empty() {
        let written_name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if written_name != name {
            return Err(format!(
                "表键 [peers.{peer_id}] name 不匹配：写入 {name}，回读 {written_name}"
            ));
        }
    }
    if rpc_port > 0 {
        let written_rpc = entry.get("rpc_port").and_then(|v| v.as_integer());
        if written_rpc != Some(rpc_port as i64) {
            return Err(format!(
                "表键 [peers.{peer_id}] rpc_port 不匹配：写入 {rpc_port}，回读 {written_rpc:?}"
            ));
        }
    }
    Ok(())
}

/// 严格 `host:port` 解析（IPv6 `[::1]:21949` 形态兼容）。
fn parse_host_port_strict(address: &str) -> Result<(String, u16), PairError> {
    let (host, port_str) = address.rsplit_once(':').ok_or_else(|| {
        PairError::BadAddress(format!(
            "「{address}」缺端口；形态：host:port（UDP 或 RPC 端口皆可，自动探测）"
        ))
    })?;
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port: u16 = port_str
        .parse()
        .map_err(|_| PairError::BadAddress(format!("「{address}」端口不是合法 u16")))?;
    if port == 0 {
        return Err(PairError::BadAddress("端口不能为 0".into()));
    }
    Ok((host, port))
}

/// 一次性 RPC `get_info` 探测：拨号 → 组帧（可选 AEAD）→ 发收 → 解析 payload。
///
/// 与 `RpcClient::try_connect_and_send` 同一 wire 协议（WireMessage JSON +
/// 4 字节长度前缀 + token 派生 AES-256-GCM），但不依赖 registry/resolver
/// ——pair 运行在集群未启动的 CLI 进程里。调用方负责总超时（外层
/// `tokio::time::timeout`），防半开连接挂死。
async fn probe_get_info(addr: &str, token: Option<&str>) -> Result<serde_json::Value, String> {
    let stream = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| format!("dial timeout to {}", addr))?
        .map_err(|e| format!("connect to {}: {}", addr, e))?;
    let std_stream = stream
        .into_std()
        .map_err(|e| format!("stream conversion: {}", e))?;
    std_stream
        .set_nonblocking(false)
        .map_err(|e| format!("set blocking: {}", e))?;

    let cipher_key = token
        .filter(|t| !t.is_empty())
        .map(crate::transport::frame::derive_key);

    let request_id = uuid::Uuid::new_v4().to_string();
    let mut wire = crate::transport::conn::WireMessage::new_request(
        "pair",
        "",
        "get_info",
        serde_json::json!({}),
    );
    // new_request 生成自己的 id；统一覆盖成可追踪的请求 id。
    wire.id = request_id;

    let json_bytes = serde_json::to_vec(&wire).map_err(|e| format!("serialize: {}", e))?;
    let wire_bytes = match &cipher_key {
        Some(key) => crate::transport::frame::encrypt_frame(&json_bytes, key)
            .map_err(|e| format!("encrypt request: {}", e))?,
        None => json_bytes,
    };

    // spawn_blocking 要求 'static：addr 落盘为 owned String 随闭包走。
    let addr_owned = addr.to_string();
    tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let addr = addr_owned.as_str();
        let mut conn = crate::transport::conn::Connection::new(std_stream);

        // Connection::send 自带 4 字节长度前缀（与服务端 AsyncFrameReader 对齐），
        // 与 RpcClient 同一字节形态。
        conn.send(&wire_bytes)
            .map_err(|e| format!("send to {}: {}", addr, e))?;

        let resp_data = conn
            .recv()
            .map_err(|e| format!("recv from {}: {}", addr, e))?;
        let plaintext = match &cipher_key {
            Some(key) => crate::transport::frame::decrypt_frame(&resp_data, key)
                .map_err(|e| format!("decrypt response from {}: {}", addr, e))?,
            None => resp_data,
        };
        let response = crate::rpc_types::Frame::decode_response(&plaintext)
            .map_err(|e| format!("decode response: {}", e))?;
        if let Some(err) = response.error {
            return Err(format!("remote error: {}", err));
        }
        Ok(response.result.unwrap_or_else(|| serde_json::json!({})))
    })
    .await
    .map_err(|e| format!("blocking task join: {}", e))?
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
