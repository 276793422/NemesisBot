//! Channel trait and BaseChannel implementation.
//!
//! Defines the core `Channel` trait that all channel adapters must implement,
//! and provides `BaseChannel` with common functionality (name, enabled flag, stats,
//! allow-list filtering, and sync-target management).

use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{debug, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

/// Channel adapter trait for sending outbound messages.
///
/// Each channel implementation must provide a name, lifecycle hooks (start/stop),
/// and a `send` method for delivering outbound messages.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Returns the unique name of this channel (e.g. "rpc", "web", "websocket").
    fn name(&self) -> &str;

    /// Starts the channel, initializing any resources or connections.
    async fn start(&self) -> Result<()>;

    /// Stops the channel, releasing resources and closing connections.
    async fn stop(&self) -> Result<()>;

    /// Sends an outbound message through this channel.
    async fn send(&self, msg: OutboundMessage) -> Result<()>;

    /// Checks whether a sender is allowed to use this channel.
    ///
    /// Returns `true` if the allow-list is empty (all senders allowed),
    /// or if the sender matches an entry in the allow-list.
    fn is_allowed(&self, _sender_id: &str) -> bool {
        true
    }

    /// Returns whether the channel is currently running.
    ///
    /// A channel is running after `start()` succeeds and until `stop()` is called.
    /// Mirrors Go's `Channel.IsRunning()` interface method.
    fn is_running(&self) -> bool {
        false
    }

    /// Adds a channel as a sync target for broadcasting messages.
    ///
    /// Returns an error if the target name matches this channel's own name
    /// (self-sync prevention).
    ///
    /// 默认实现是**静默 no-op 存根**（`Ok(())`）：未 override 的通道上
    /// 此调用不落任何状态。出站镜像不依赖它（`ChannelManager`
    /// dispatch_outbound 集中触发，BUG-20261112-118）；只有带 BaseChannel
    /// 且有**入站镜像扇出**（读循环 `sync_to_targets`）的通道
    /// （external/websocket）必须 override 委托固有实现。
    fn add_sync_target(&self, _name: &str, _channel: Arc<dyn Channel>) -> Result<()> {
        Ok(())
    }

    /// Removes a previously added sync target by name.
    fn remove_sync_target(&self, _name: &str) {}
}

// ---------------------------------------------------------------------------
// URL 查询参数凭据脱敏（webhook 形态通道共用）
// ---------------------------------------------------------------------------

/// 脱敏字符串中 URL 查询参数里的凭据值。
///
/// 部分通道的官方 webhook 形态自带凭据（企业微信群机器人 `?key=`、
/// 钉钉 sessionWebhook `?access_token=` 等），reqwest 错误 Display 会内嵌
/// 完整 URL（`... for url (<url>)`），直接 `{e}` 格式化进日志/错误串会把
/// 凭据一并带上（BUG-20261109-006 同族横向修复）。本函数按已知凭据参数名
/// 掩掉值（`key=<redacted>`），非凭据参数原样保留诊断价值。
///
/// 只匹配以 `?` 或 `&` 起头的参数名；值截至 `&`/空白/引号/`)`/串尾。
// BUG-20261109-006 横向修复的先行 helper：单测已接（base/tests.rs），生产
// 调用点（企业微信/钉钉 webhook 错误串格式化）随 006 系列后续提交接线；
// lib 目标视角暂无调用方，allow 压 dead_code（接线后可摘）。
#[allow(dead_code)]
pub(crate) fn redact_url_secrets(msg: impl std::fmt::Display) -> String {
    // 前缀长的参数名在前（access_token= 先于 token=）；同名前缀残撞由
    // `?`/`&` 边界检查兜住（access_token= 中的 token= 前面是 `_`，不命中）。
    // ticket=（BUG-20261119-182）：钉钉 stream endpoint / 飞书 WS endpoint
    // 都是 `wss://…?ticket=<连接凭据>` 形态——凭据随 URL 进日志即泄露。
    const SECRET_PARAMS: [&str; 7] = [
        "access_token=",
        "webhook_key=",
        "ticket=",
        "key=",
        "token=",
        "sign=",
        "secret=",
    ];
    let mut out = msg.to_string();
    for name in SECRET_PARAMS {
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        loop {
            let Some(pos) = rest.find(name) else {
                result.push_str(rest);
                break;
            };
            if pos != 0 && !matches!(rest.as_bytes()[pos - 1], b'?' | b'&') {
                // 参数名边界不成立（子串残撞），原样带过继续找
                result.push_str(&rest[..pos + name.len()]);
                rest = &rest[pos + name.len()..];
                continue;
            }
            result.push_str(&rest[..pos]);
            result.push_str(name);
            rest = &rest[pos + name.len()..];
            let end = rest
                .find(|c: char| c == '&' || c == ')' || c.is_whitespace() || c == '"' || c == '\'')
                .unwrap_or(rest.len());
            result.push_str("<redacted>");
            rest = &rest[end..];
        }
        out = result;
    }
    out
}

// ---------------------------------------------------------------------------
// VoiceTranscriber trait (dependency injection for voice transcription)
// ---------------------------------------------------------------------------

/// Trait for voice transcription services.
///
/// Mirrors Go's `voice.GroqTranscriber` but decoupled from the voice module
/// to avoid circular dependencies. Implementations (e.g., `nemesis-voice::Transcriber`)
/// are injected into channels via `set_transcriber()`.
pub trait VoiceTranscriber: Send + Sync {
    /// Check if the transcriber is available (e.g., has API key configured).
    fn is_available(&self) -> bool;

    /// Transcribe an audio file at the given path.
    ///
    /// Returns the transcribed text on success, or an error description on failure.
    fn transcribe(
        &self,
        file_path: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send + '_>,
    >;
}

/// Statistics for a channel instance.
#[derive(Debug, Default)]
pub struct ChannelStats {
    /// Number of messages sent (outbound).
    pub messages_sent: AtomicU64,
    /// Number of messages received (inbound).
    pub messages_received: AtomicU64,
}

/// Base channel providing common functionality.
///
/// Wraps name, enabled flag, message counters, allow-list, and sync targets.
/// Concrete channel implementations can embed this and delegate common operations.
pub struct BaseChannel {
    name: String,
    enabled: parking_lot::RwLock<bool>,
    running: parking_lot::RwLock<bool>,
    stats: Arc<ChannelStats>,
    /// Allow-list for sender filtering. If empty, all senders are allowed.
    allow_list: Vec<String>,
    /// Sync targets for broadcasting messages to other channels.
    sync_targets: parking_lot::RwLock<HashMap<String, Arc<dyn Channel>>>,
}

impl fmt::Debug for BaseChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BaseChannel")
            .field("name", &self.name)
            .field("enabled", &self.is_enabled())
            .field("running", &self.is_running())
            .field("allow_list", &self.allow_list)
            .field(
                "sync_targets",
                &self.sync_targets.read().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Clone for BaseChannel {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            enabled: parking_lot::RwLock::new(*self.enabled.read()),
            running: parking_lot::RwLock::new(*self.running.read()),
            stats: Arc::clone(&self.stats),
            allow_list: self.allow_list.clone(),
            sync_targets: parking_lot::RwLock::new(self.sync_targets.read().clone()),
        }
    }
}

/// Splits a compound sender ID (e.g. `"123456|username"`) into its ID and username parts.
///
/// Returns `(id_part, user_part)`. If there is no `|` separator, `user_part` is empty.
fn split_sender_id(sender_id: &str) -> (&str, &str) {
    match sender_id.find('|') {
        Some(idx) if idx > 0 => (&sender_id[..idx], &sender_id[idx + 1..]),
        _ => (sender_id, ""),
    }
}

/// 通道 send 的 panic 隔离包裹。
///
/// dispatch 环是单任务串行派发（`ChannelManager::dispatch_loop`）：任一
/// 通道 send future 内的 panic（三方库解析、毒化锁 unwrap、越界——26
/// 通道 × 三方依赖面非全仓自审）会沿 unwind 炸掉整个 dispatch loop
/// task——出站 mpsc 无接收者静默堆积，gateway 其余功能照常，全部通道
/// 出站停摆且零错误零日志。对齐既有长环隔离纪律（heartbeat 逐拍
/// `catch_unwind` / cron hook / agent loop 工具执行 future 同款）：panic
/// 映射为 `Err(..)` 返回，消费方走既有 Err 臂记账（send_errors），环
/// 存活继续派发后续消息。
///
/// 诚实边界：`iotsmall` profile 是 `panic=abort`，`catch_unwind` 接不住
/// abort（进程死即任务死，与工具隔离同一取舍）；send 半程副作用（消息
/// 可能已送达外部服务）与超时放弃 future 的孤儿语义同级，错误串注明
/// 「送达状态未知」。
pub(crate) async fn send_panic_isolated(
    channel_name: &str,
    ch: Arc<dyn Channel>,
    msg: OutboundMessage,
) -> Result<()> {
    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(ch.send(msg))).await {
        Ok(result) => result,
        Err(payload) => {
            let detail = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "opaque panic".to_string()
            };
            warn!(
                channel = %channel_name,
                "[Channel] send PANIC caught, dispatch loop survives: {detail}"
            );
            Err(NemesisError::Channel(format!(
                "{channel_name} send panicked: {detail} (delivery state unknown, partial side effects possible)"
            )))
        }
    }
}

/// Sends a single sync mirror message to one target channel.
///
/// Single truth source for mirror construction (BUG-20261112-118): stamp
/// `meta.sync_via` with the source channel name (the receiving side's loop
/// guard), use `"web:broadcast"` as chat ID for the `"web"` target, and
/// apply a 3-second send timeout. Both mirror origins share this path:
/// channel-level `sync_to_targets` (inbound-origin fan-outs from
/// external/websocket read loops) and the manager-level centralized trigger
/// in `ChannelManager::dispatch_outbound` (outbound-origin fan-outs).
pub(crate) async fn send_sync_mirror(
    source_name: &str,
    target_name: &str,
    target_ch: Arc<dyn Channel>,
    content: &str,
) {
    let mut chat_id = String::new();
    if target_name == "web" {
        chat_id = "web:broadcast".to_string();
    }

    let mut msg = OutboundMessage::new(target_name, &chat_id, content);
    // 单一盖章点：接收侧（manager dispatch_outbound 的集中触发）见到
    // 该标记不再二次扩散——互配 `sync_to` 的两通道会无限乒乓，每轮都是
    // 一次真实外部投递的消息风暴（BUG-57 回环防护语义，随出站触发点
    // 迁到 manager 层保持不变）。
    msg.meta.sync_via = Some(source_name.to_string());

    debug!(
        channel = %source_name,
        target = %target_name,
        content_len = content.len(),
        chat_id = %chat_id,
        "[Channel] sending to sync target"
    );

    // panic 隔离在 timeout 内层：send future 的 panic 不穿透 timeout
    // 炸调用方任务（镜像扇出跑在 manager dispatch 环内，同一条命）。
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        send_panic_isolated(target_name, target_ch, msg),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            warn!(
                channel = %source_name,
                target = %target_name,
                error = %e,
                "[Channel] failed to sync to target"
            );
        }
        Err(_) => {
            warn!(
                channel = %source_name,
                target = %target_name,
                "[Channel] sync to target timed out"
            );
        }
    }
}

impl BaseChannel {
    /// Creates a new `BaseChannel` with the given name and an empty allow-list.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: parking_lot::RwLock::new(true),
            running: parking_lot::RwLock::new(false),
            stats: Arc::new(ChannelStats::default()),
            allow_list: Vec::new(),
            sync_targets: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Creates a new `BaseChannel` with the given name and allow-list.
    pub fn with_allow_list(name: impl Into<String>, allow_list: Vec<String>) -> Self {
        Self {
            name: name.into(),
            enabled: parking_lot::RwLock::new(true),
            running: parking_lot::RwLock::new(false),
            stats: Arc::new(ChannelStats::default()),
            allow_list,
            sync_targets: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Returns the channel name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether the channel is enabled.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Sets the enabled flag.
    pub fn set_enabled(&self, value: bool) {
        *self.enabled.write() = value;
    }

    /// Returns whether the channel is currently running.
    ///
    /// Mirrors Go's `BaseChannel.IsRunning()`. A channel is running
    /// after `start()` succeeds and until `stop()` is called.
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }

    /// Sets the running flag.
    ///
    /// Mirrors Go's `BaseChannel.setRunning()`. Should be called by
    /// concrete channel implementations in their `start()` and `stop()` methods.
    pub fn set_running(&self, value: bool) {
        *self.running.write() = value;
    }

    /// Increments the sent message counter.
    pub fn record_sent(&self) {
        self.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the received message counter.
    pub fn record_received(&self) {
        self.stats.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the number of messages sent.
    pub fn messages_sent(&self) -> u64 {
        self.stats.messages_sent.load(Ordering::Relaxed)
    }

    /// Returns the number of messages received.
    pub fn messages_received(&self) -> u64 {
        self.stats.messages_received.load(Ordering::Relaxed)
    }

    /// Checks whether a sender is allowed based on the allow-list.
    ///
    /// If the allow-list is empty, all senders are allowed. Otherwise, the sender
    /// ID is checked against each allowed entry using compound-ID matching that
    /// supports formats like `"123456|username"` and `"@username"`.
    pub fn is_allowed(&self, sender_id: &str) -> bool {
        if self.allow_list.is_empty() {
            return true;
        }

        let (id_part, user_part) = split_sender_id(sender_id);

        for allowed in &self.allow_list {
            // Strip leading '@' from allowed value for username matching
            let trimmed = allowed.trim_start_matches('@');
            let (allowed_id, allowed_user) = split_sender_id(trimmed);

            if sender_id == allowed
                || id_part == allowed
                || sender_id == trimmed
                || id_part == trimmed
                || id_part == allowed_id
                || (!allowed_user.is_empty() && sender_id == allowed_user)
                || (!user_part.is_empty()
                    && (user_part == allowed || user_part == trimmed || user_part == allowed_user))
            {
                return true;
            }
        }

        false
    }

    /// Adds a channel as a sync target.
    ///
    /// Returns an error if attempting to add a target with the same name as
    /// this channel (self-sync prevention).
    pub fn add_sync_target(&self, name: &str, channel: Arc<dyn Channel>) -> Result<()> {
        if name == self.name {
            return Err(NemesisError::Channel(
                "channel cannot sync to itself".to_string(),
            ));
        }
        self.sync_targets.write().insert(name.to_string(), channel);
        debug!(target = %name, "[Channel] sync target added");
        Ok(())
    }

    /// Removes a sync target by name.
    ///
    /// No-op if the target was not previously added.
    pub fn remove_sync_target(&self, name: &str) {
        self.sync_targets.write().remove(name);
        debug!(target = %name, "[Channel] sync target removed");
    }

    /// Sends a message to all configured sync targets.
    ///
    /// Skips any target whose name matches this channel's name (double-check).
    /// Each send is delegated to [`send_sync_mirror`] (stamp + web:broadcast
    /// + 3s timeout, single truth source).
    ///
    /// 入站镜像起点（external/websocket 读循环）经此扇出；出站镜像的扇出
    /// 已集中到 `ChannelManager::dispatch_outbound`（send 成功后触发），
    /// 不再依赖各通道 `send()` 自行扩散。
    pub async fn sync_to_targets(&self, content: &str) {
        // Collect targets into a Vec within a block scope so the RwLockReadGuard
        // is dropped before any.await. parking_lot::RwLockReadGuard is !Send,
        // which would make the future !Send if held across an await point.
        let entries: Vec<(String, Arc<dyn Channel>)> = {
            let targets = self.sync_targets.read();
            if targets.is_empty() {
                return;
            }
            targets
                .iter()
                .filter(|(target_name, _)| *target_name != &self.name)
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        debug!(
            channel = %self.name,
            content_len = content.len(),
            num_targets = entries.len(),
            "[Channel] syncing to targets"
        );

        for (target_name, target_ch) in entries {
            send_sync_mirror(&self.name, &target_name, target_ch, content).await;
        }
    }

    /// Handles an inbound message after checking the allow-list.
    ///
    /// If the sender is not allowed, the message is silently dropped.
    /// Otherwise, the message is published to the inbound bus (when a bus
    /// reference is available via the concrete channel implementation).
    ///
    /// Returns `true` if the message was accepted, `false` if blocked.
    pub fn handle_message(&self, sender_id: &str) -> bool {
        if !self.is_allowed(sender_id) {
            return false;
        }
        self.record_received();
        true
    }
}

// ---------------------------------------------------------------------------
// 对端 IP 准入判定（单一真相源）
// ---------------------------------------------------------------------------

/// 对端 IP 准入判定（maixcam BUG-20261110-054 接线；websocket
/// BUG-20261118-155 同族共用——两处定义漂移过一次后收编单点）。
/// `allow_from` 空 = 放行所有（兼容既有部署）；非空 = 对端 IP 必须命中
/// 白名单。条目支持精确 IP（`192.168.1.50`）与 IPv4 前缀通配
/// （`192.168.1.*`，尾段通配）。适用于「服务端监听通道的客户端无
/// 可预知应用层身份」的场景（maixcam 设备无握手 / websocket client_id
/// 进程内生成不可预知）——此时 allow_from 的真实语义是对端网络准入。
pub(crate) fn peer_allowed(allow_from: &[String], peer_ip: &str) -> bool {
    if allow_from.is_empty() {
        return true;
    }
    let peer_ip = peer_ip.trim();
    for entry in allow_from {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some(prefix) = entry.strip_suffix('*') {
            // BUG-20261121-022 段边界闸：无分隔符尾的手滑写法（`192.168.1*`
            // 漏尾点）用字符 starts_with 会把 `192.168.11.x` / `192.168.100.x`
            // 误放进白名单——`1` 是 `11`/`100` 的字符串前缀，段边界撞名，
            // 对端网络准入假放行。要求 prefix 以分隔符结尾（契约形态
            // `192.168.1.*` / IPv6 hextet `fd00::*`）或 peer_ip 在边界处
            // 恰是 `.`/`:`（`192.168.1*` 照常放行 `192.168.1.50`、拒绝
            // `192.168.11.5`；IPv6 `fd00:a*` 放行 `fd00:a::1`、拒绝
            // `fd00:aa::1`）。收紧方向：契约形态行为不变，只可能更严。
            let boundary_ok = prefix.ends_with('.')
                || prefix.ends_with(':')
                || peer_ip.as_bytes().get(prefix.len()) == Some(&b'.')
                || peer_ip.as_bytes().get(prefix.len()) == Some(&b':');
            if peer_ip.starts_with(prefix) && boundary_ok {
                return true;
            }
        } else if peer_ip == entry {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 出站 HTTP client 单一构造点
// ---------------------------------------------------------------------------

/// 出站 HTTP builder 统一定义：通道出站 client 必须带
/// 总超时。reqwest 默认无总超时——通道远端半死（TCP 已建立但响应挂起）时
/// `channel.send()` 永久 pending，而 `ChannelManager` 的 dispatch loop 是
/// 单任务串行 await，一条通道挂起 = **全部**通道出站停摆且零错误零日志。
/// 35s 对齐 telegram 先例（消息投递类请求；long-polling 型通道
/// matrix/signal 的 sync/receive 走各自 120s builder，不经此处）。
/// 需要 proxy 注入的通道拿 builder 自己续链，无特殊需求直接用
/// [`outbound_http_client`]。
// 同上：default 构建形态无消费通道（telegram/whatsapp 在全 feature 下续链）。
#[allow(dead_code)]
pub(crate) fn outbound_http_builder() -> reqwest::ClientBuilder {
    // 凭据随重定向跨主机泄漏防线：自定义凭据头（dingtalk token 头等）与
    // URL path token（telegram /bot<token>/）不在 reqwest 跨主机剥离名单，
    // 302 会带去第三方主机。单一真相源 nemesis-utils::http_redirect
    // （同源跟随/跨源终停；共享面单点 = 全通道受益）。
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(35))
        .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
}

/// [`outbound_http_builder`] 的直用形态（无 proxy 注入需求的通道）。
// default features（web/webhook/rpc）不编译任何消费通道——全 feature 构建下
// 11 通道使用，dead_code 只在 default 构建形态出现。
#[allow(dead_code)]
pub(crate) fn outbound_http_client() -> Result<reqwest::Client> {
    outbound_http_builder()
        .build()
        .map_err(|e| NemesisError::Channel(format!("HTTP client creation failed: {e}")))
}

// ---------------------------------------------------------------------------
// 出站 WebSocket 拨号单一构造点
// ---------------------------------------------------------------------------

/// 出站 WS 握手超时帽（BUG-20261116-041）。15s 对齐 cdp.rs 先例
/// （`connect_async` 无内建超时——对端接受 TCP 后不回 HTTP 101 时
/// `connect_async` 永久挂起：重连任务停在 await 内，退避/取消检查全失
/// 效，通道静默停摆零错误零日志。裸 TCP connect 有 OS SYN 超时兜底，
/// WS 握手没有——握手是 HTTP-over-TCP，纯等对端字节）。
// default 构建形态无消费通道（7 个 WS 通道默认关）——同
// outbound_http_builder 房规，dead_code 只在 default 构建出现。
#[allow(dead_code)]
pub(crate) const WS_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// [`ws_connect_bounded`] 的显式超时形态（测试用小超时；生产走 15s 帽）。
#[allow(dead_code)]
pub(crate) async fn ws_connect_bounded_with_timeout<R>(
    target: R,
    timeout: std::time::Duration,
) -> std::result::Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    String,
>
where
    R: tokio_tungstenite::tungstenite::client::IntoClientRequest + Unpin,
{
    match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(target)).await {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(e)) => Err(format!("websocket connect failed: {e}")),
        Err(_) => Err(format!(
            "websocket handshake timeout ({timeout:?}): server accepted TCP but never sent \
             HTTP 101 (half-open?)"
        )),
    }
}

/// 出站 WebSocket 拨号单一构造点：带握手超时帽的 `connect_async`。
///
/// 通道重连环统一走此入口（discord/slack/qq/nostr/mattermost/feishu/
/// onebot）；超时 Err 与 connect Err 同形态（String），消费方既有退避
/// 重试臂无需区分。取消安全：超时丢 future 即关 socket，握手未完成前
/// 无共享流状态。
#[allow(dead_code)]
pub(crate) async fn ws_connect_bounded<R>(
    target: R,
) -> std::result::Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    String,
>
where
    R: tokio_tungstenite::tungstenite::client::IntoClientRequest + Unpin,
{
    ws_connect_bounded_with_timeout(target, WS_HANDSHAKE_TIMEOUT).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
