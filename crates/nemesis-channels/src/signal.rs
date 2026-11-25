//! Signal channel (signal-cli-rest-api REST, envelope processing).
//!
//! Uses the signal-cli-rest-api for sending and receiving Signal messages
//! via long polling on the /v1/receive endpoint.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Signal channel configuration.
#[derive(Debug, Clone)]
pub struct SignalConfig {
    /// signal-cli-rest-api base URL.
    pub api_url: String,
    /// Signal phone number (with country code).
    pub phone_number: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// Signal envelope (incoming message).
#[derive(Debug, Deserialize)]
pub struct SignalEnvelope {
    pub envelope: Option<SignalEnvelopeInner>,
}

/// Inner envelope data.
///
/// BUG-20261125-025：wire 字段名权威真相源 = signal-cli-rest-api swagger
/// `receive.MessageEnvelope`（camelCase：`sourceNumber`/`sourceUuid`/
/// `dataMessage`/`syncMessage`...）。本结构体曾用 snake_case 且无
/// `rename_all`——serde 精确匹配字段名，线上全部 camelCase 键静默忽略：
/// `data_message` 恒 None → receive loop 每条消息 `continue` → 通道 100%
/// 静默丢消息；`source_number` 恒 None → sender 恒 "unknown"（allow_from
/// 配置时所有真实消息被过滤 / 未配置时跨发送者会话键塌缩）。
/// 测试 fixture 自建 snake_case JSON 与线上格式脱锚 = 假绿实证。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalEnvelopeInner {
    pub source: Option<String>,
    pub source_number: Option<String>,
    pub source_uuid: Option<String>,
    pub timestamp: Option<i64>,
    pub data_message: Option<SignalDataMessage>,
    pub sync_message: Option<serde_json::Value>,
}

/// Signal data message.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalDataMessage {
    pub timestamp: Option<i64>,
    pub message: Option<String>,
    pub group_info: Option<SignalGroupInfo>,
}

/// Signal group info.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalGroupInfo {
    pub group_id: Option<String>,
    pub group_name: Option<String>,
}

/// Sender 身份提取单一真相源：优先手机号（sourceNumber），回落 source，
/// 再回落 sourceUuid（隐藏号码账户只有 UUID 形态身份），全缺 = "unknown"。
fn envelope_sender(inner: &SignalEnvelopeInner) -> String {
    inner
        .source_number
        .as_deref()
        .or(inner.source.as_deref())
        .or(inner.source_uuid.as_deref())
        .unwrap_or("unknown")
        .to_string()
}

/// Processes a received envelope → (sender, chat_id, content, timestamp)。
///
/// 模块级自由函数：receive loop 与测试共用同一提取逻辑（此前 loop 内联
/// 副本与 pub 方法双路径，测试锚不到生产路径——025 假绿的结构性成因之一）。
pub fn process_envelope(envelope: &SignalEnvelopeInner) -> Option<(String, String, String, i64)> {
    let data_msg = envelope.data_message.as_ref()?;
    let content = data_msg.message.as_deref().unwrap_or("");
    if content.is_empty() {
        return None;
    }

    let sender = envelope_sender(envelope);

    let chat_id = if let Some(ref group_info) = data_msg.group_info {
        group_info
            .group_id
            .clone()
            .unwrap_or_else(|| sender.clone())
    } else {
        sender.clone()
    };

    Some((
        sender,
        chat_id,
        content.to_string(),
        data_msg.timestamp.unwrap_or(0),
    ))
}

/// Signal send request.
#[derive(Serialize)]
struct SignalSendRequest {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
}

/// Signal channel using signal-cli-rest-api.
pub struct SignalChannel {
    base: BaseChannel,
    config: SignalConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    /// 已处理时间戳去重表（Arc 槽位：spawn 闭包与实例方法共享同一张表）。
    /// BUG-20261125-038：signal-cli REST 长轮询 at-least-once（消费超时/
    /// 服务重启等场景重复投递同一 envelope），此前表存在但 receive loop
    /// 零调用（死代码，与 qq BUG-20261125-032 同构），重复消息原样喂 agent。
    processed_timestamps: Arc<parking_lot::RwLock<HashMap<i64, bool>>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl SignalChannel {
    /// Creates a new `SignalChannel`.
    pub fn new(
        config: SignalConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.api_url.is_empty() || config.phone_number.is_empty() {
            return Err(NemesisError::Channel(
                "signal api_url and phone_number are required".to_string(),
            ));
        }

        Ok(Self {
            base: BaseChannel::new("signal"),
            config: SignalConfig {
                // api_url 尾斜杠归一：config 反序列化直通，用户从 signal-cli
                // REST 文档/示例复制带尾斜杠 base（如 `http://127.0.0.1:8080/`）
                // 时 receive_url/send 的 `format!("{}/v1/...")` 产生
                // `//v1/receive/` 双斜杠——signal-cli-rest-api 服务路由精确
                // 匹配 404，收发全挂（BUG-20261109-034，与 discord/telegram/
                // qq 的 BUG-20261109-012 同族）。构造期写回 = 3 个拼接消费点
                // （receive_url + send 两分支）的单一归一漏斗。
                api_url: config.api_url.trim_end_matches('/').to_string(),
                ..config
            },
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                // 凭据随重定向跨主机泄漏防线（nemesis-utils::http_redirect
                // 同源跟随/跨源终停；signal 独立 builder 不走 base 共享面）。
                .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
                .build()
                .map_err(|e| NemesisError::Channel(format!("HTTP client creation failed: {e}")))?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            processed_timestamps: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            bus_sender,
        })
    }

    /// Returns the receive URL for long polling.
    pub fn receive_url(&self) -> String {
        format!(
            "{}/v1/receive/{}",
            self.config.api_url, self.config.phone_number
        )
    }

    /// Checks if a timestamp has already been processed.
    pub fn is_duplicate(&self, timestamp: i64) -> bool {
        Self::dedup_check(&self.processed_timestamps, timestamp)
    }

    /// 去重查表核心（静态形态：spawn 闭包持有的 Arc 槽位与实例方法同源）。
    /// 重复返回 `true`（is_duplicate 语义），首见登记后返回 `false`。
    fn dedup_check(map: &parking_lot::RwLock<HashMap<i64, bool>>, timestamp: i64) -> bool {
        let mut m = map.write();
        if m.contains_key(&timestamp) {
            return true;
        }
        m.insert(timestamp, true);

        // Simple cleanup: limit map size
        if m.len() > 10000 {
            let keys: Vec<i64> = m.keys().take(5000).copied().collect();
            for key in keys {
                m.remove(&key);
            }
        }

        false
    }

    /// 分派前去重判定（BUG-20261125-038 接线点）：timestamp 非零查表——首见
    /// 放行并登记、重复丢弃；timestamp 为 0 恒放行——process_envelope 对缺失
    /// timestamp 落 0（unwrap_or(0)），无键可查；若把 0 也登记，所有缺时间戳
    /// 消息会被折叠成一条（每批只活一条），与 qq 空 msg_id 恒放行同语义。
    fn should_publish(map: &parking_lot::RwLock<HashMap<i64, bool>>, timestamp: i64) -> bool {
        timestamp == 0 || !Self::dedup_check(map, timestamp)
    }

    /// Sends a message to a number or group.
    pub async fn send_signal_message(&self, to: &str, content: &str, is_group: bool) -> Result<()> {
        // URL 单一形态：is_group 只切换 payload 的 group/number 字段，
        // 端点相同（原 if/else 两分支字节相同，clippy identical-blocks）。
        let url = format!(
            "{}/v2/send/{}",
            self.config.api_url, self.config.phone_number
        );

        let mut request = SignalSendRequest {
            message: content.to_string(),
            group: None,
        };

        if is_group {
            request.group = Some(to.to_string());
        }

        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "message": content,
                if is_group { "group" } else { "number" }: to,
            }))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("signal send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = nemesis_utils::http_cap::read_error_body(resp).await;
            return Err(NemesisError::Channel(format!("signal send error: {body}")));
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        // BUG-20261125-019（BUG-20261121-050 同构家族横向扫漏网补齐）：
        // 幂等双启守卫 + base running 旗标接线——无守卫的重复 start 会
        // spawn 多个并行 receive loop 每条消息 N 倍喂 agent；trait
        // is_running()（读 base 旗标）恒 false 使 manager 运行中守卫
        // （start_all / any_channel_running / status 上报三消费点）失真。
        if self.base.is_running() {
            info!("[SignalChannel] already running, skipping duplicate start");
            return Ok(());
        }
        info!("[SignalChannel] starting Signal channel");
        *self.running.write() = true;
        self.base.set_running(true);
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let http = self.http.clone();
        let receive_url = self.receive_url();
        let running = self.running.clone();
        let allow_from = self.config.allow_from.clone();
        let processed_timestamps = self.processed_timestamps.clone();

        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_secs(5);
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            loop {
                if !*running.read() {
                    break;
                }

                tokio::time::sleep(poll_interval).await;

                if !*running.read() {
                    break;
                }

                let resp = match http.get(&receive_url).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("[SignalChannel] poll error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    warn!("[SignalChannel] poll returned {}", resp.status());
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }

                backoff = std::time::Duration::from_secs(1);

                let envelopes: Vec<SignalEnvelope> = match resp.json().await {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                for envelope in envelopes {
                    let inner = match envelope.envelope {
                        Some(e) => e,
                        None => continue,
                    };

                    // BUG-20261125-025：提取逻辑与测试共用模块级自由函数
                    // （单一真相源；此前此处内联副本与 pub 方法双路径漂移，
                    // 测试锚不到生产路径）。
                    let (sender, chat_id, content, timestamp) = match process_envelope(&inner) {
                        Some(t) => t,
                        None => continue,
                    };

                    // 去重接线（BUG-20261125-038）：长轮询重复投递同一
                    // envelope（同 timestamp）在此拦截
                    if !Self::should_publish(&processed_timestamps, timestamp) {
                        debug!(timestamp = %timestamp, "[SignalChannel] duplicate message dropped");
                        continue;
                    }

                    // Filter by allow_from
                    if !allow_from.is_empty()
                        && !crate::base::allow_list_contains(&allow_from, &sender)
                    {
                        continue;
                    }

                    let inbound = InboundMessage::user_ingress(
                        "signal".to_string(),
                        sender,
                        chat_id.clone(),
                        content,
                    )
                    .with_session_key(chat_id)
                    .with_metadata_map({
                        let mut m = std::collections::HashMap::new();
                        m.insert("timestamp".to_string(), timestamp.to_string());
                        m
                    });

                    if let Err(e) = bus.send(inbound) {
                        warn!("[SignalChannel] failed to publish inbound message to bus: {e}");
                    }
                }
            }

            info!("[SignalChannel] receive loop stopped");
        });

        info!("[SignalChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[SignalChannel] stopping Signal channel");
        *self.running.write() = false;
        // BUG-20261125-019：对称清 base 旗标（契约面见 start 内注释）。
        self.base.set_running(false);
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "signal channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let is_group = msg.chat_id.len() > 20; // heuristic: group IDs are longer
        self.send_signal_message(&msg.chat_id, &msg.content, is_group)
            .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
