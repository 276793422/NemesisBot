//! MQTT 通道（P28，Wave 3 能力扩展）：基于 rumqttc 的双向通道。
//!
//! - **入站**：订阅 config 指定的 topic filter 列表（支持 `+`/`#` 通配，默认
//!   QoS 1），payload（JSON 信封或纯文本）转 `InboundMessage`，具体 topic 作为
//!   chat_id/session_key；
//! - **出站**：publish 到回包 topic。回包 topic 解析优先级（`resolve_reply_topic`）：
//!   ① 入站信封学习到的 `reply_topic`（会话级最具体）→ ② config topic 映射表
//!   （chat_id 具体化后匹配订阅 filter 的 `reply_topic`）→ ③ 全局默认回包 topic；
//!   三者皆缺位 = 诚实报错不乱发。
//!
//! 设计要点：
//! - **QoS 1（AtLeastOnce）为默认**（P28 钦定），0/2 可配，>2 在构造期拒绝；
//! - **重连语义**：事件循环 poll 出错只记日志 + 退避重试——rumqttc 的 EventLoop
//!   在后续 poll 时自动重连（`network=None` 分支），broker 后起不影响常驻进程
//!   （IoT 场景刚需）；每次收到 ConnAck（含重连成功）都重新订阅——clean_session
//!   =true 时 broker 不保留订阅关系；
//! - **防回环**：部署纪律上回包 topic 必须与订阅 filter 不同；兜底——出站恒包
//!   JSON 信封（`{"from":"nemesisbot","content":...}`），入站侧遇到带该标记的
//!   信封直接跳过（订阅 filter 覆盖到回包 topic / 双 bot 共享 broker 时不自对话）；
//! - **凭据纪律**：username/password 只进 config 结构体，绝不硬编码；装配层
//!   （gateway，B3 先例）可对字段做 vault:/env:/yaml: 引用解析；
//! - **凭据为空 = 匿名连接**（LAN broker 常态），不隐式填充。
//!
//! 已知边界（诚实声明）：
//! - v1 仅纯 TCP 传输：rumqttc 以 `default-features = false` 接入，避免其
//!   `use-rustls` 默认链引入 aws-lc-rs（Windows/CI 构建依赖重，workspace 现网
//!   rustls 走 ring）；TLS 挂账；
//! - 入站 QoS 1 ACK 由 rumqttc 自动完成（manual_acks=false）＝「收到即 ACK」，
//!   agent 处理失败不重投（传输层 at-least-once，处理层 at-most-once）；
//! - 停止走取消信号 + 任务退出即断 TCP（clean_session 下不追求优雅 DISCONNECT）；
//! - 真实 broker 端到端手测挂账（无凭证）；契约测试用 tokio TCP mock 最小
//!   broker（rumqttc 无内建 mock 模式），见 `mqtt/tests.rs`。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// 入站信封学习表容量上限（防 topic 空间膨胀；超出后停止学习，已有条目保留）。
const MAX_LEARNED_REPLY_TOPICS: usize = 1024;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// MQTT 通道配置。
///
/// 由 gateway 装配层从 `config.json` channels 段构造后传入（`ChannelInitConfig.mqtt`）；
/// username/password 建议走装配层 vault/env 引用，config 明文仅限内网实验。
#[derive(Debug, Clone, Default)]
pub struct MqttChannelConfig {
    /// broker 地址（默认 127.0.0.1）。
    pub broker_host: String,
    /// broker 端口（默认 1883）。
    pub broker_port: u16,
    /// MQTT client id；空 = 自动生成 `nemesisbot-<pid>`（同 broker 内须唯一）。
    pub client_id: String,
    /// 用户名；空 = 匿名。支持装配层 vault:/env:/yaml: 引用。
    pub username: String,
    /// 密码；空 = 无。只进 config 结构，绝不硬编码。
    pub password: String,
    /// keep-alive 秒数（默认 30；0 = 关闭保活）。
    pub keep_alive_secs: u64,
    /// clean session（默认 true；false 时 broker 保留订阅与未达消息）。
    pub clean_session: bool,
    /// 断线重连退避秒数（默认 5，下限 1）。
    pub reconnect_delay_secs: u64,
    /// 订阅与出站统一 QoS（默认 1 = AtLeastOnce；0/1/2，>2 构造期拒绝）。
    pub qos: u8,
    /// 订阅 + 回包映射表。
    pub topics: Vec<MqttTopicMapping>,
    /// 全局默认回包 topic；空 = 无默认（映射/学习都缺位时出站报错）。
    pub default_reply_topic: String,
    /// sender 白名单（入站信封 sender_id 或固定 "mqtt"）；空 = 全放行。
    pub allow_from: Vec<String>,
}

/// 订阅 topic filter → 回包 topic 映射（topic 映射表条目）。
#[derive(Debug, Clone, Default)]
pub struct MqttTopicMapping {
    /// 订阅的 topic filter（支持 `+`/`#` 通配，MQTT 3.1.1 §4.7 语义）。
    pub topic: String,
    /// 出站回包 topic；空 = 此映射不承接回包（回落学习条目/默认回包）。
    pub reply_topic: String,
}

impl MqttChannelConfig {
    /// QoS 配置值 → rumqttc QoS（构造期已保证 ≤2，此处兜底归一到 AtLeastOnce）。
    fn qos(&self) -> QoS {
        match self.qos {
            0 => QoS::AtMostOnce,
            2 => QoS::ExactlyOnce,
            _ => QoS::AtLeastOnce,
        }
    }
}

// ---------------------------------------------------------------------------
// 入站 payload 解析（JSON 信封 / 纯文本回退）
// ---------------------------------------------------------------------------

/// 入站 payload 解析结果。
#[derive(Debug, Clone, PartialEq)]
enum ParsedPayload {
    /// 正常消息（JSON 信封字段，或整段原文作纯文本回退）。
    Message {
        content: String,
        sender_id: Option<String>,
        reply_topic: Option<String>,
    },
    /// 跳过（防回环标记命中，或空 payload）。
    Skip,
}

/// 入站 payload 解析（单一真相源，事件循环与单测共用）。
///
/// 规则：
/// 1. 空 payload → [`ParsedPayload::Skip`]；
/// 2. 首字符为 `{` 且可解析为 JSON：
///    - `from == "nemesisbot"` → [`ParsedPayload::Skip`]（防回环兜底）；
///    - 有字符串 `content` 字段 → 信封模式（可选 `sender_id`/`reply_topic`）；
///    - 无 `content` 字段 → 整段原文当纯文本（诚实回退，不静默丢）；
/// 3. 其余（含非 UTF-8）→ `from_utf8_lossy` 后 trim 整段作纯文本。
fn parse_payload(raw: &[u8]) -> ParsedPayload {
    if raw.is_empty() {
        return ParsedPayload::Skip;
    }
    let text = String::from_utf8_lossy(raw);
    let trimmed = text.trim();
    if trimmed.starts_with('{')
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        // 防回环：本 bot 发出的信封直接跳过
        if v.get("from").and_then(|f| f.as_str()) == Some("nemesisbot") {
            return ParsedPayload::Skip;
        }
        if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
            return ParsedPayload::Message {
                content: content.to_string(),
                sender_id: v
                    .get("sender_id")
                    .and_then(|s| s.as_str())
                    .map(str::to_string),
                reply_topic: v
                    .get("reply_topic")
                    .and_then(|t| t.as_str())
                    .map(str::to_string),
            };
        }
        // JSON 但缺 content 字段 → 落到纯文本回退（整段原文）
    }
    ParsedPayload::Message {
        content: trimmed.to_string(),
        sender_id: None,
        reply_topic: None,
    }
}

// ---------------------------------------------------------------------------
// topic filter 匹配
// ---------------------------------------------------------------------------

/// MQTT topic filter（订阅侧，可含通配符）与具体 topic 的匹配判定。
///
/// 规则（MQTT 3.1.1 §4.7）：
/// - `+` 匹配恰好一层（不跨层）；
/// - `#` 匹配剩余全部层级，必须为最后一段；父级 filter `a/#` 也匹配 `a` 本身。
fn mqtt_topic_matches(filter: &str, topic: &str) -> bool {
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();
    let mut i = 0;
    while i < f.len() {
        match f[i] {
            "#" => return i == f.len() - 1,
            "+" => {
                if i >= t.len() {
                    return false;
                }
            }
            level => {
                if i >= t.len() || t[i] != level {
                    return false;
                }
            }
        }
        i += 1;
    }
    f.len() == t.len()
}

// ---------------------------------------------------------------------------
// MqttChannel
// ---------------------------------------------------------------------------

/// MQTT 客户端通道（长连接，rumqttc AsyncClient + EventLoop）。
pub struct MqttChannel {
    base: Arc<BaseChannel>,
    config: MqttChannelConfig,
    /// 消息总线入站 sender。
    bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    /// 出站客户端句柄：start() 装入，stop() 清空（Clone 廉价，锁内取即弃，
    /// 不跨 await 持锁）。
    client: parking_lot::Mutex<Option<AsyncClient>>,
    /// 入站信封学习到的 chat_id → reply_topic（Arc 共享给事件循环任务写入）。
    reply_topics: Arc<dashmap::DashMap<String, String>>,
    /// 事件循环任务句柄（stop 时优雅等待 + abort 兜底）。
    event_task: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 事件循环取消信号（stop 时触发）。
    cancel_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl std::fmt::Debug for MqttChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttChannel")
            .field("name", &self.base.name())
            .field("config", &self.config)
            .field("running", &self.base.is_running())
            .field("reply_topics", &self.reply_topics.iter().count())
            .field("client_installed", &self.client.lock().is_some())
            .finish()
    }
}

impl MqttChannel {
    /// 创建 MQTT 通道。构造期做 config 校验（快速失败，不注册半成品）。
    pub fn new(
        config: MqttChannelConfig,
        bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.broker_host.trim().is_empty() {
            return Err(NemesisError::Channel(
                "mqtt broker_host 不能为空".to_string(),
            ));
        }
        if config.qos > 2 {
            return Err(NemesisError::Channel(format!(
                "mqtt qos 必须为 0/1/2，收到 {}",
                config.qos
            )));
        }
        for mapping in &config.topics {
            if mapping.topic.trim().is_empty() {
                return Err(NemesisError::Channel(
                    "mqtt 订阅 topic filter 不能为空".to_string(),
                ));
            }
        }
        Ok(Self {
            base: Arc::new(BaseChannel::with_allow_list(
                "mqtt",
                config.allow_from.clone(),
            )),
            config,
            bus_sender,
            client: parking_lot::Mutex::new(None),
            reply_topics: Arc::new(dashmap::DashMap::new()),
            event_task: parking_lot::Mutex::new(None),
            cancel_tx: parking_lot::Mutex::new(None),
        })
    }

    /// 回包 topic 解析（优先级：学习条目 > 映射表 > 默认回包；None = 无处可发）。
    fn resolve_reply_topic(&self, chat_id: &str) -> Option<String> {
        // ① 入站信封学习到的 reply_topic（会话级最具体）
        if let Some(t) = self.reply_topics.get(chat_id)
            && !t.is_empty()
        {
            return Some(t.clone());
        }
        // ② config 映射表：chat_id（入站具体 topic）匹配订阅 filter
        for mapping in &self.config.topics {
            if !mapping.reply_topic.is_empty() && mqtt_topic_matches(&mapping.topic, chat_id) {
                return Some(mapping.reply_topic.clone());
            }
        }
        // ③ 全局默认回包 topic
        if !self.config.default_reply_topic.is_empty() {
            return Some(self.config.default_reply_topic.clone());
        }
        None
    }

    /// 事件循环主体（独立任务）。
    ///
    /// rumqttc 语义：`eventloop.poll()` 返回 Err 后不重建连接对，继续 poll 即自动
    /// 重连；每次 ConnAck（含重连成功）重新订阅全部 filter。取消信号经由 select
    /// 触发退出；重连退避 sleep 期间不响应取消（stop 的 2s 优雅窗 + abort 兜底
    /// 覆盖该窗口）。
    async fn run_event_loop(
        base: Arc<BaseChannel>,
        config: MqttChannelConfig,
        client: AsyncClient,
        mut eventloop: EventLoop,
        bus_sender: tokio::sync::broadcast::Sender<InboundMessage>,
        reply_topics: Arc<dashmap::DashMap<String, String>>,
        mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
    ) {
        let qos = config.qos();
        let reconnect_delay = Duration::from_secs(config.reconnect_delay_secs.max(1));

        loop {
            if !base.is_running() {
                break;
            }
            tokio::select! {
                _ = &mut cancel_rx => {
                    info!("[MqttChannel] 收到停止信号，事件循环退出");
                    break;
                }
                poll = eventloop.poll() => {
                    match poll {
                        Ok(Event::Incoming(Packet::ConnAck(_))) => {
                            info!(
                                topics = config.topics.len(),
                                "[MqttChannel] 已连接 broker，(重)订阅全部 filter"
                            );
                            for mapping in &config.topics {
                                if let Err(e) = client.subscribe(&mapping.topic, qos).await {
                                    warn!(topic = %mapping.topic, error = %e, "[MqttChannel] 订阅失败");
                                }
                            }
                        }
                        Ok(Event::Incoming(Packet::Publish(publish))) => {
                            Self::ingest_publish(&base, &reply_topics, &publish, &bus_sender);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!(
                                error = %e,
                                delay_secs = reconnect_delay.as_secs(),
                                "[MqttChannel] 连接断开，退避后自动重连"
                            );
                            tokio::time::sleep(reconnect_delay).await;
                        }
                    }
                }
            }
        }
        info!("[MqttChannel] 事件循环已退出");
    }

    /// 入站 PUBLISH 消化：payload 解析 → 白名单 → reply_topic 学习 → bus 发布。
    fn ingest_publish(
        base: &BaseChannel,
        reply_topics: &dashmap::DashMap<String, String>,
        publish: &Publish,
        bus_sender: &tokio::sync::broadcast::Sender<InboundMessage>,
    ) {
        let chat_id = publish.topic.clone();
        if chat_id.is_empty() {
            debug!("[MqttChannel] 收到空 topic publish，跳过");
            return;
        }
        match parse_payload(&publish.payload[..]) {
            ParsedPayload::Skip => {
                debug!(topic = %chat_id, "[MqttChannel] payload 跳过（空/防回环标记）");
            }
            ParsedPayload::Message {
                content,
                sender_id,
                reply_topic,
            } => {
                // 白名单：未配置 = 全放行；未配置 sender_id 的消息固定 "mqtt"
                let sender = sender_id.clone().unwrap_or_else(|| "mqtt".to_string());
                if !base.handle_message(&sender) {
                    warn!(
                        sender = %sender,
                        topic = %chat_id,
                        "[MqttChannel] 消息被 allow_from 白名单拦截"
                    );
                    return;
                }
                // 学习 reply_topic（容量上限防膨胀；出站解析优先级最高）
                if let Some(rt) = reply_topic
                    && !rt.is_empty()
                    && reply_topics.len() < MAX_LEARNED_REPLY_TOPICS
                {
                    reply_topics.insert(chat_id.clone(), rt);
                }
                let mut metadata = HashMap::new();
                metadata.insert("mqtt_topic".to_string(), chat_id.clone());
                metadata.insert("mqtt_qos".to_string(), (publish.qos as u8).to_string());
                let inbound = InboundMessage {
                    channel: base.name().to_string(),
                    sender_id: sender,
                    session_key: chat_id.clone(),
                    chat_id,
                    content,
                    media: Vec::new(),
                    correlation_id: String::new(),
                    metadata,
                    voice_playback: None,
                };
                if let Err(e) = bus_sender.send(inbound) {
                    warn!(error = %e, "[MqttChannel] 入站消息发布到总线失败");
                }
            }
        }
    }
}

#[async_trait]
impl Channel for MqttChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        // 幂等：已在运行直接返回
        if self.base.is_running() {
            return Ok(());
        }

        // client id：空 = nemesisbot-<pid>（同 broker 内进程级唯一）
        let client_id = if self.config.client_id.trim().is_empty() {
            format!("nemesisbot-{}", std::process::id())
        } else {
            self.config.client_id.clone()
        };
        let mut opts = MqttOptions::new(
            client_id,
            self.config.broker_host.clone(),
            self.config.broker_port,
        );
        opts.set_keep_alive(Duration::from_secs(self.config.keep_alive_secs));
        opts.set_clean_session(self.config.clean_session);
        // 凭据只来自 config 结构（装配层可注入 vault/env 解析结果），空 = 匿名
        if !self.config.username.is_empty() {
            opts.set_credentials(self.config.username.clone(), self.config.password.clone());
        }

        // rumqttc：请求通道容量 64（出站瞬时排队上限；QoS1 断线期间由
        // EventLoop pending 列表承接重发）
        let (client, eventloop) = AsyncClient::new(opts, 64);

        info!(
            broker = %self.config.broker_host,
            port = self.config.broker_port,
            qos = self.config.qos,
            topics = self.config.topics.len(),
            "[MqttChannel] starting（broker 未就绪时事件循环自动重试）"
        );

        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        *self.cancel_tx.lock() = Some(cancel_tx);

        // running 标志先置位再 spawn（事件循环以 is_running 为退出条件之一）
        self.base.set_running(true);
        *self.client.lock() = Some(client.clone());

        let task = tokio::spawn(Self::run_event_loop(
            Arc::clone(&self.base),
            self.config.clone(),
            client,
            eventloop,
            self.bus_sender.clone(),
            Arc::clone(&self.reply_topics),
            cancel_rx,
        ));
        *self.event_task.lock() = Some(task);

        info!("[MqttChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[MqttChannel] stopping");
        self.base.set_running(false);

        // 触发取消信号 → 事件循环 select 分支退出；重连退避 sleep 窗口由
        // 优雅等待 + abort 兜底覆盖。clean_session 下不追求优雅 DISCONNECT，
        // 任务退出即断 TCP。
        if let Some(tx) = self.cancel_tx.lock().take() {
            let _ = tx.send(());
        }
        // 先 take 出句柄再 await：parking_lot MutexGuard 非 Send，不能跨 await 存活
        // （async_trait 要求 future: Send）；guard 随 let 语句结束释放。
        let handle = self.event_task.lock().take();
        if let Some(handle) = handle {
            let abort_handle = handle.abort_handle();
            match tokio::time::timeout(Duration::from_secs(2), handle).await {
                Ok(_) => {}
                Err(_) => {
                    warn!("[MqttChannel] 事件循环 2s 内未退出，abort 兜底");
                    abort_handle.abort();
                }
            }
        }
        self.client.lock().take();
        self.reply_topics.clear();
        info!("[MqttChannel] stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !self.base.is_running() {
            return Err(NemesisError::Channel(
                "mqtt channel not running".to_string(),
            ));
        }
        // 回包 topic 解析：学习 > 映射 > 默认，皆缺位 = 诚实报错（不回声到订阅 topic）
        let topic = match self.resolve_reply_topic(&msg.chat_id) {
            Some(t) => t,
            None => {
                return Err(NemesisError::Channel(format!(
                    "mqtt 无法解析回包 topic（chat_id={:?} 无学习条目/映射/默认回包；\
                     为防回环不会向订阅 topic 回声）",
                    msg.chat_id
                )));
            }
        };
        let client = { self.client.lock().clone() }
            .ok_or_else(|| NemesisError::Channel("mqtt channel not running".to_string()))?;
        // 防回环信封：出站带 from="nemesisbot" 标记，让入站侧的 Skip 兜底真正
        // 生效（订阅 filter 覆盖到回包 topic / 双 bot 共享 broker 时，自己发的
        // 消息会被自己当纯文本入站 → 自对话放大循环）。2026-09-26 复查修复：
        // 此前出站发裸 content，入站 Skip 分支永不触发。
        let envelope = serde_json::json!({
            "from": "nemesisbot",
            "content": msg.content,
        })
        .to_string();
        client
            .publish(&topic, self.config.qos(), false, envelope)
            .await
            .map_err(|e| NemesisError::Channel(format!("mqtt publish 失败: {e}")))?;
        self.base.record_sent();
        debug!(
            topic = %topic,
            len = msg.content.len(),
            "[MqttChannel] outbound published"
        );
        // 出站同步镜像（websocket 先例：镜像到 web 等同步目标）
        self.base.sync_to_targets(&msg.content).await;
        Ok(())
    }

    fn add_sync_target(&self, name: &str, channel: Arc<dyn Channel>) -> Result<()> {
        self.base.add_sync_target(name, channel)
    }

    fn remove_sync_target(&self, name: &str) {
        self.base.remove_sync_target(name);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
