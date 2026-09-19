//! 反向桥帧协议（goal：节点显示名 + 反向桥与多设备汇聚，一期批次一）。
//!
//! 桥两端（中继服务端 ↔ 桥客户端）之间的全部通信都是 JSON 文本帧
//! （与集群 RPC 同风格；单用户面板流量量级足够，二进制帧优化待实测需要）。
//!
//! 帧分三类：
//! - **链路控制帧**：`bridge_hello`（接入鉴权，token 走 hello 帧不走 URL
//!   query——避免进访问日志）、`bridge_welcome`（服务端回执）、`heartbeat`/
//!   `pong`（30s 心跳 / 90s 无心跳判离线）、`bridge_close`（优雅关闭）
//! - **面板访问鉴权帧**：`access_check`（服务端→设备，转发浏览器提交的
//!   SHA-256 哈希；**密码原文不出设备本机、服务端零存储**）+ `access_result`
//!   回执
//! - **数据帧**：`conn_open`/`conn_data`/`conn_close`——一条 conn = 浏览器
//!   与设备 web server 之间的一次字节流搬运（普通 HTTP = 短 conn；WS 升级
//!   = open 后转长泵直至任一端关闭）
//!
//! 二/三期预留帧：`cluster_rpc`/`member_sync`——一期**收到即忽略 + WARN**，
//! 协议一次定对，后续扩展不破坏性改协议。
//!
//! **二期身份交换（2026-09-19 启用）**：`bridge_hello` 增补集群身份字段
//! （集群 node_id/显示名/role/category/tags/capabilities/node_type/
//! rpc_port/addresses，全部 `Option` + serde default——缺省 = 未启用集群
//! 身份（老版本/纯隧道设备），互连兼容）；`bridge_welcome` 增补
//! `hub_node_id`（hub 侧集群身份，`--relay` 纯中继为空）。桥链路身份
//! （`bridge-{hostname}`，conn 路由/顶替锚点）与集群身份（registry 锚点）
//! 是两个空间，hello 同时携带两者。
//!
//! **位置注记（对 goal 的偏离）**：goal 原文把协议类型放在
//! `nemesisbot/src/relay/protocol.rs`，但 nemesis-web 不能引用 bin crate
//! （nemesisbot）的类型，而路由/handler/状态页都在 nemesis-web——协议类型
//! 与服务端状态机整体放 `nemesis-web/src/relay/`，桥客户端（nemesisbot）
//! 引用 `nemesis_web::relay::protocol` 复用。

use serde::{Deserialize, Serialize};

/// 面板访问 token 校验回执等待超时（服务端侧；设备离线/不回 = fail）。
pub const ACCESS_CHECK_TIMEOUT_SECS: u64 = 5;

/// 服务端心跳判定：设备最近一次心跳/任意帧距今超过该秒数 → 判离线踢连接
/// （客户端 30s 一跳，3× 容错）。
pub const HEARTBEAT_TIMEOUT_SECS: u64 = 90;

/// 桥帧（serde 内部 tag：`{"type":"bridge_hello",...}`；snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeFrame {
    // ---- 链路控制（设备 → 服务端）----
    /// 桥接入握手：携带接入门 token + 设备身份（node_id/显示名/版本——
    /// 显示名为二期身份交换预留，一期仅记录展示）。
    ///
    /// 二期增补集群身份字段（全部 `Option` + serde default，缺省 = 老版本/
    /// 未启用集群，服务端仅作隧道设备处理）。
    BridgeHello {
        token: String,
        node_id: String,
        name: String,
        version: String,
        /// 集群身份：registry 锚点 node_id（与桥链路 `node_id` 是两个空间）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cluster_node_id: Option<String>,
        /// 集群显示名（config.cluster.node_name 或 hostname 解析链产物）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cluster_name: Option<String>,
        /// 集群角色（worker/master/...）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// 集群类别。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
        /// 集群标签。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tags: Option<Vec<String>>,
        /// RPC 能力（工具名清单）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
        /// 节点类型（agent/node）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node_type: Option<String>,
        /// 本机 RPC server 监听端口（0 = 未启动，服务端不注册集群节点）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rpc_port: Option<u16>,
        /// 本机网卡地址清单（同网段直连仲裁用）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        addresses: Option<Vec<String>>,
    },
    /// 设备心跳（30s 一跳；服务端回 `Pong`）。
    Heartbeat,
    /// 设备主动关闭桥连接（如本机 Bot 正在关停）。
    BridgeClose { reason: String },

    // ---- 链路控制（服务端 → 设备）----
    /// hello 回执：`ok=false` = 拒绝接入（token 错 / 已关闭 / node_id 冲突），
    /// 服务端随后关连接；客户端据此诚实报错（区分「配对失败」与「网络断」）。
    /// `hub_node_id` = hub 侧集群身份（正常模式非空；`--relay` 纯中继为空
    /// ——设备据此感知 hub 是否集群节点，三期跨桥寻址用）。
    BridgeWelcome {
        ok: bool,
        reason: String,
        #[serde(default)]
        hub_node_id: String,
    },
    /// 心跳回执。
    Pong,

    // ---- 面板访问鉴权（服务端 → 设备 + 回执）----
    /// 面板访问 token 校验转发：`hash_hex` = 浏览器端 SHA-256(token) 的 hex。
    /// 设备比对 `SHA-256(access_token)` 后回 `AccessResult`。服务端全程
    /// 只见哈希、零存储。
    AccessCheck {
        request_id: String,
        node_id: String,
        hash_hex: String,
    },
    /// access_check 回执（设备 → 服务端）。
    AccessResult { request_id: String, ok: bool },

    // ---- 数据帧（双向）----
    /// 开一条 conn：`target` 一期恒 `"local"`（本机 web server；为二期
    /// 多目标预留）。conn_id 由**服务端**分配（全局原子递增）。
    ConnOpen { conn_id: u64, target: String },
    /// conn 字节搬运：`data_b64` = base64 字节；`seq` 单调递增（0 起，设备
    /// 侧按序写出）；`fin=true` = 该方向最后一包。下行（服务端 → 设备）
    /// fin 仅为信息性提示（请求完整性由 content-length 界定，设备侧**不**
    /// 半关闭写半——2026-09-19 真机实测 shutdown 使 hyper 于处理前断连）；
    /// 上行空载荷 + fin = EOF 约定（设备侧读到本机关连接时收口用）。
    ConnData {
        conn_id: u64,
        seq: u64,
        data_b64: String,
        fin: bool,
    },
    /// 关一条 conn（dial 失败回执 / 任一端正常关闭 / 异常终止都走这里；
    /// `reason` 供日志与错误页展示）。
    ConnClose { conn_id: u64, reason: String },

    // ---- 二/三期预留（一期收到即忽略 + WARN）----
    /// 三期：集群 RPC 帧经桥转发（`payload` 结构二三期细化）。
    ClusterRpc { payload: serde_json::Value },
    /// 三期：成员表广播（`payload` 结构三期细化）。
    MemberSync { payload: serde_json::Value },
}

/// 编码帧为 JSON 文本（ws Text 帧载荷）。
pub fn encode_frame(frame: &BridgeFrame) -> Result<String, serde_json::Error> {
    serde_json::to_string(frame)
}

/// 解码 JSON 文本为帧。
pub fn decode_frame(text: &str) -> Result<BridgeFrame, serde_json::Error> {
    serde_json::from_str(text)
}

/// 浏览器请求方向的重序列化：method/uri/headers/body → 原始 HTTP 报文字节。
///
/// 服务端把 axum 已解析的请求**重拼成 HTTP 报文**发给设备——桥语义是
/// 「搬运浏览器与 web server 之间的原始字节流」，axum 必然先解析（构造
/// axum Response 也必然要解析响应头），设备侧拿到的字节与浏览器发出的
/// 报文等价。hop-by-hop 头（connection/keep-alive/transfer-encoding）不透传
/// ——这些语义由每跳自行协商；`content-length` 按实际 body 重算。
pub fn serialize_http_request(
    method: &http::Method,
    uri: &http::Uri,
    headers: &http::HeaderMap,
    body: &[u8],
) -> Vec<u8> {
    let mut head = String::with_capacity(512);
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    head.push_str(method.as_str());
    head.push(' ');
    head.push_str(&path);
    head.push_str(" HTTP/1.1\r\n");
    // hop-by-hop 头不透传（这些语义由每跳自行协商）；`content-length` 按
    // 实际 body 重算。WS 升级请求的 connection/upgrade 头在末尾原样补回。
    const HOP_BY_HOP: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
    ];
    let mut saw_host = false;
    for (name, value) in headers.iter() {
        let name = name.as_str();
        let lower = name.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lower.as_str()) || lower == "content-length" {
            continue;
        }
        if lower == "host" {
            saw_host = true;
        }
        if let Ok(v) = value.to_str() {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
    }
    if !saw_host && let Some(host) = uri.host() {
        head.push_str("host: ");
        head.push_str(host);
        head.push_str("\r\n");
    }
    // WS 升级请求：connection/upgrade 头是升级语义的关键，原样补回。
    if let Some(conn) = headers.get(http::header::CONNECTION)
        && let Ok(v) = conn.to_str()
    {
        head.push_str("connection: ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if let Some(upg) = headers.get(http::header::UPGRADE)
        && let Ok(v) = upg.to_str()
    {
        head.push_str("upgrade: ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("content-length: ");
    head.push_str(&body.len().to_string());
    head.push_str("\r\n\r\n");

    let mut buf = Vec::with_capacity(head.len() + body.len());
    buf.extend_from_slice(head.as_bytes());
    buf.extend_from_slice(body);
    buf
}

/// 响应头解析：从头几包字节里切出 status line + headers。
/// 返回 (状态码, 头列表, 头部总字节数)。找不到 `\r\n\r\n` → None（继续等）。
pub fn parse_http_response_head(
    buf: &[u8],
) -> Option<(http::StatusCode, Vec<(String, String)>, usize)> {
    let head_end = find_subslice(buf, b"\r\n\r\n")? + 4;
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    // "HTTP/1.1 200 OK"
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next()?;
    let code: u16 = parts.next()?.parse().ok()?;
    let status = http::StatusCode::from_u16(code).ok()?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Some((status, headers, head_end))
}

/// 响应头里的 chunked 检测（chunked 响应服务端要解块后按 content-length
/// 语义重新输出——axum Response 不透传 chunked 编码）。
pub fn is_chunked(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(n, v)| {
        n.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    })
}

/// 在 buf 中找子串首次出现位置。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// chunked 编码解码器：设备 web server 若回 chunked 响应，服务端按块解码
/// 后以 content-length 语义流式输出（axum Response 不透传 chunked）。
pub struct ChunkedDecoder {
    buf: Vec<u8>,
    /// 当前块剩余字节（None = 正在等块头）。
    remaining_in_chunk: Option<usize>,
    finished: bool,
}

impl ChunkedDecoder {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            remaining_in_chunk: None,
            finished: false,
        }
    }

    /// 喂入新字节，返回可输出给客户端的已解码字节。
    /// 内部缓冲会吃掉块头/CRLF/尾块，输出只含净荷。
    pub fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while !self.finished {
            if let Some(remaining) = self.remaining_in_chunk {
                if remaining == 0 {
                    // 块结束：吃 CRLF
                    if self.buf.len() < 2 {
                        break;
                    }
                    let crlf = self.buf.drain(..2).collect::<Vec<_>>();
                    if &crlf != b"\r\n" {
                        // 协议错乱——按结束处理，剩余原样吐出。
                        out.extend_from_slice(&crlf);
                        self.buf.clear();
                        break;
                    }
                    self.remaining_in_chunk = None;
                    continue;
                }
                let take = remaining.min(self.buf.len());
                if take == 0 {
                    break;
                }
                out.extend_from_slice(&self.buf.drain(..take).collect::<Vec<_>>());
                self.remaining_in_chunk = Some(remaining - take);
            } else {
                // 等块头行
                let Some(pos) = find_subslice(&self.buf, b"\r\n") else {
                    break;
                };
                let line = String::from_utf8_lossy(&self.buf[..pos]).to_string();
                self.buf.drain(..pos + 2);
                let size_part = line.split(';').next().unwrap_or("").trim();
                let size = match usize::from_str_radix(size_part, 16) {
                    Ok(s) => s,
                    Err(_) => {
                        // 坏块头——按结束处理。
                        self.finished = true;
                        self.buf.clear();
                        break;
                    }
                };
                if size == 0 {
                    // 尾块：规范上后面还有 trailer，简化为直接结束（本机
                    // 回环 hyper 不发 trailer）。
                    self.finished = true;
                    self.buf.clear();
                    break;
                }
                self.remaining_in_chunk = Some(size);
            }
        }
        out
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        Self::new()
    }
}
