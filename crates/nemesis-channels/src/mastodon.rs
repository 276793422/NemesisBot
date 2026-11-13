//! Mastodon channel (REST + SSE streaming, OAuth token).
//!
//! Uses Mastodon REST API for posting statuses and SSE streaming for
//! receiving notifications.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};
use nemesis_types::utils::split_message;

use crate::base::{BaseChannel, Channel};

/// Mastodon channel configuration.
/// SEC-002（BUG-20261111-068）：OAuth access token 脱敏。
#[derive(Clone)]
pub struct MastodonConfig {
    /// Server URL (e.g. "https://mastodon.social").
    pub server: String,
    /// OAuth access token.
    pub access_token: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

crate::debug_redact::impl_redacted_debug!(MastodonConfig {
    plain { server, allow_from }
    secret { access_token }
});

/// Mastodon notification.
#[derive(Debug, Deserialize)]
pub struct MastodonNotification {
    pub id: String,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub account: MastodonAccount,
    pub status: Option<MastodonStatus>,
}

/// Mastodon account.
#[derive(Debug, Deserialize)]
pub struct MastodonAccount {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
}

/// Mastodon status (post).
#[derive(Debug, Deserialize)]
pub struct MastodonStatus {
    pub id: String,
    pub content: String,
    pub in_reply_to_id: Option<String>,
}

/// Post status request.
#[derive(Serialize)]
struct PostStatusRequest {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    in_reply_to_id: Option<String>,
}

/// Post status response.
#[derive(Debug, Deserialize)]
struct PostStatusResponse {
    id: String,
}

/// Mastodon channel using REST API and SSE streaming.
pub struct MastodonChannel {
    base: BaseChannel,
    config: MastodonConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    seen_notifications: std::sync::Arc<parking_lot::RwLock<HashMap<String, bool>>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl MastodonChannel {
    /// Creates a new `MastodonChannel`.
    pub fn new(
        config: MastodonConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<Self> {
        if config.server.is_empty() || config.access_token.is_empty() {
            return Err(NemesisError::Channel(
                "mastodon server and access_token are required".to_string(),
            ));
        }

        let server = config.server.trim_end_matches('/').to_string();

        Ok(Self {
            base: BaseChannel::new("mastodon"),
            config: MastodonConfig { server, ..config },
            http: crate::base::outbound_http_client()?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            seen_notifications: std::sync::Arc::new(parking_lot::RwLock::new(HashMap::new())),
            bus_sender,
        })
    }

    /// Returns the streaming URL for notifications.
    pub fn streaming_url(&self) -> String {
        format!("{}/api/v1/streaming/user", self.config.server)
    }

    /// Returns the notifications API URL.
    pub fn notifications_url(&self) -> String {
        format!("{}/api/v1/notifications", self.config.server)
    }

    /// Verifies credentials.
    pub async fn verify_credentials(&self) -> Result<MastodonAccount> {
        let url = format!("{}/api/v1/accounts/verify_credentials", self.config.server);

        let resp = self
            .http
            .get(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.access_token),
            )
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("mastodon verify failed: {e}")))?;

        if !resp.status().is_success() {
            let body = nemesis_utils::http_cap::read_error_body(resp).await;
            return Err(NemesisError::Channel(format!(
                "mastodon verify error: {body}"
            )));
        }

        resp.json()
            .await
            .map_err(|e| NemesisError::Channel(format!("mastodon verify parse failed: {e}")))
    }

    /// Processes a notification and extracts content.
    pub fn process_notification(
        &self,
        notification: &MastodonNotification,
    ) -> Option<(String, String, String)> {
        if notification.notification_type != "mention" {
            return None;
        }

        let sender_id = &notification.account.username;
        let status = notification.status.as_ref()?;

        // Strip HTML tags from content
        let content = strip_html_tags(&status.content);

        let chat_id = status.id.clone();

        Some((sender_id.clone(), chat_id, content))
    }

    /// Posts a status (reply).
    pub async fn post_status(&self, content: &str, in_reply_to: Option<&str>) -> Result<String> {
        let url = format!("{}/api/v1/statuses", self.config.server);

        let request = PostStatusRequest {
            status: content.to_string(),
            in_reply_to_id: in_reply_to.map(|s| s.to_string()),
        };

        let resp = self
            .http
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.access_token),
            )
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("mastodon post failed: {e}")))?;

        if !resp.status().is_success() {
            let body = nemesis_utils::http_cap::read_error_body(resp).await;
            return Err(NemesisError::Channel(format!(
                "mastodon post error: {body}"
            )));
        }

        let result: PostStatusResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("mastodon post parse failed: {e}")))?;

        Ok(result.id)
    }

    /// Marks a notification as read.
    pub fn mark_seen(&self, notification_id: &str) {
        let mut map = self.seen_notifications.write();
        map.insert(notification_id.to_string(), true);
        if map.len() > 10000 {
            let keys: Vec<String> = map.keys().take(5000).cloned().collect();
            for key in keys {
                map.remove(&key);
            }
        }
    }

    /// Checks if a notification has been seen.
    pub fn is_seen(&self, notification_id: &str) -> bool {
        self.seen_notifications.read().contains_key(notification_id)
    }

    /// 首轮基线：把窗口内全部 mention 通知 id 标记为已见（不发布）。
    /// 供 start() 轮询循环在首次成功拉取后调用。
    fn mark_mentions_seen(
        notifications: &[serde_json::Value],
        seen: &std::sync::Arc<parking_lot::RwLock<HashMap<String, bool>>>,
    ) {
        for notif in notifications {
            if notif["type"].as_str().unwrap_or("") != "mention" {
                continue;
            }
            if let Some(id) = notif["id"].as_str() {
                seen.write().insert(id.to_string(), true);
            }
        }
    }

    /// 处理单条通知：mention 类型闸 → allow_from 白名单闸 → 去重 → 内容
    /// 提取 → bus 发布。返回是否发布了入站消息（测试断言用）。
    ///
    /// - allow_from 判据与 signal/whatsapp/qq 同款家族惯例：空 = 全放行；
    ///   匹配对象为 sender_id（Mastodon username，不含域）。此前 config
    ///   `channels.mastodon.allow_from` 是声名未读取的死键——白名单写了
    ///   任何 mention 都照常入总线（BUG-20261113-149）。
    fn ingest_notification(
        notif: &serde_json::Value,
        seen: &std::sync::Arc<parking_lot::RwLock<HashMap<String, bool>>>,
        allow_from: &[String],
        bus: &broadcast::Sender<InboundMessage>,
    ) -> bool {
        if notif["type"].as_str().unwrap_or("") != "mention" {
            return false;
        }

        let account = match notif.get("account") {
            Some(a) => a,
            None => return false,
        };
        let status = match notif.get("status") {
            Some(s) => s,
            None => return false,
        };

        let sender_id = account["username"].as_str().unwrap_or("unknown");
        if !allow_from.is_empty() && !allow_from.iter().any(|a| a == sender_id) {
            return false;
        }

        let notif_id = notif["id"].as_str().unwrap_or("").to_string();

        // Dedup check
        {
            let mut map = seen.write();
            if map.contains_key(&notif_id) {
                return false;
            }
            map.insert(notif_id.clone(), true);
            if map.len() > 10000 {
                let keys: Vec<String> = map.keys().take(5000).cloned().collect();
                for key in keys {
                    map.remove(&key);
                }
            }
        }

        let content = strip_html_tags(status["content"].as_str().unwrap_or(""));
        if content.is_empty() {
            return false;
        }

        let chat_id = status["id"].as_str().unwrap_or("").to_string();

        let inbound = InboundMessage {
            channel: "mastodon".to_string(),
            sender_id: sender_id.to_string(),
            chat_id: chat_id.clone(),
            content,
            media: Vec::new(),
            session_key: chat_id,
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        };

        let _ = bus.send(inbound);
        true
    }
}

/// Strips HTML tags from a string.
pub fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .trim()
        .to_string()
}

#[async_trait]
impl Channel for MastodonChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!("[MastodonChannel] starting Mastodon channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let http = self.http.clone();
        let server = self.config.server.clone();
        let access_token = self.config.access_token.clone();
        let running = self.running.clone();
        let seen = self.seen_notifications.clone();
        let allow_from = self.config.allow_from.clone();

        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_secs(30);
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);
            let mut last_notification_id: Option<String> = None;
            // 首轮成功拉取前为 true：首个窗口只建基线不发布（见下方
            // mark_mentions_seen 注释）。
            let mut first_poll = true;

            loop {
                if !*running.read() {
                    break;
                }

                tokio::time::sleep(poll_interval).await;

                if !*running.read() {
                    break;
                }

                // Build notifications URL
                let mut url = format!("{}/api/v1/notifications?types[]=mention&limit=30", server);
                if let Some(ref sid) = last_notification_id {
                    url.push_str(&format!("&since_id={}", sid));
                }

                let resp = match http
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("[MastodonChannel] notification poll error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    warn!(
                        "[MastodonChannel] notification poll returned {}",
                        resp.status()
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }

                backoff = std::time::Duration::from_secs(1);

                let notifications: Vec<serde_json::Value> = match resp.json().await {
                    Ok(n) => n,
                    Err(_) => continue,
                };

                // Update last_notification_id from newest first
                if let Some(newest) = notifications.first()
                    && let Some(nid) = newest["id"].as_str()
                {
                    last_notification_id = Some(nid.to_string());
                }

                // 首轮基线（nostr since=now 同款家族语义）：进程首启/重启后的
                // 第一次成功拉取，把窗口内全部 mention 标记为已见、**不发布**。
                // 否则 seen 表与 since_id 都只在内存，每次重启都会把最近
                // ≤30 条历史 mention 当新消息回灌，bot 对早已回复过的旧帖
                // 逐条重发回复（垃圾刷屏）。离线期间的 mention 不补答。
                if first_poll {
                    Self::mark_mentions_seen(&notifications, &seen);
                    first_poll = false;
                    continue;
                }

                for notif in &notifications {
                    Self::ingest_notification(notif, &seen, &allow_from, &bus);
                }
            }

            info!("[MastodonChannel] polling loop stopped");
        });

        info!("[MastodonChannel] channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[MastodonChannel] stopping Mastodon channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "mastodon channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let in_reply_to = if msg.chat_id.is_empty() {
            None
        } else {
            Some(msg.chat_id.as_str())
        };

        // mastodon 硬限 500 字符（服务端 422 拒收整条），
        // 预算 480 留余量；长文拆成多条回复（同父）。
        let chunks = split_message(&msg.content, 480);
        for chunk in &chunks {
            self.post_status(chunk, in_reply_to).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
