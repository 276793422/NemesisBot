//! Mastodon channel (REST + SSE streaming, OAuth token).
//!
//! Uses Mastodon REST API for posting statuses and SSE streaming for
//! receiving notifications.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Mastodon channel configuration.
#[derive(Debug, Clone)]
pub struct MastodonConfig {
    /// Server URL (e.g. "https://mastodon.social").
    pub server: String,
    /// OAuth access token.
    pub access_token: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

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
    seen_notifications: parking_lot::RwLock<HashMap<String, bool>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl MastodonChannel {
    /// Creates a new `MastodonChannel`.
    pub fn new(config: MastodonConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.server.is_empty() || config.access_token.is_empty() {
            return Err(NemesisError::Channel(
                "mastodon server and access_token are required".to_string(),
            ));
        }

        let server = config.server.trim_end_matches('/').to_string();

        Ok(Self {
            base: BaseChannel::new("mastodon"),
            config: MastodonConfig { server, ..config },
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            seen_notifications: parking_lot::RwLock::new(HashMap::new()),
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
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("mastodon verify failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
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
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("mastodon post failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
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

    async fn start(&self) -> Result<()> {
        info!("starting Mastodon channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let http = self.http.clone();
        let server = self.config.server.clone();
        let access_token = self.config.access_token.clone();
        let running = self.running.clone();
        let seen = self.seen_notifications.clone();

        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_secs(30);
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);
            let mut last_notification_id: Option<String> = None;

            loop {
                if !*running.read() {
                    break;
                }

                tokio::time::sleep(poll_interval).await;

                if !*running.read() {
                    break;
                }

                // Build notifications URL
                let mut url = format!(
                    "{}/api/v1/notifications?types[]=mention&limit=30",
                    server
                );
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
                        warn!("Mastodon: notification poll error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    warn!("Mastodon: notification poll returned {}", resp.status());
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
                if let Some(newest) = notifications.first() {
                    if let Some(nid) = newest["id"].as_str() {
                        last_notification_id = Some(nid.to_string());
                    }
                }

                for notif in &notifications {
                    let notif_type = notif["type"].as_str().unwrap_or("");
                    if notif_type != "mention" {
                        continue;
                    }

                    let notif_id = notif["id"].as_str().unwrap_or("").to_string();

                    // Dedup check
                    {
                        let mut map = seen.write();
                        if map.contains_key(&notif_id) {
                            continue;
                        }
                        map.insert(notif_id.clone(), true);
                        if map.len() > 10000 {
                            let keys: Vec<String> = map.keys().take(5000).cloned().collect();
                            for key in keys {
                                map.remove(&key);
                            }
                        }
                    }

                    let status = match notif.get("status") {
                        Some(s) => s,
                        None => continue,
                    };

                    let account = match notif.get("account") {
                        Some(a) => a,
                        None => continue,
                    };

                    let content_html = status["content"].as_str().unwrap_or("");
                    let content = strip_html_tags(content_html);
                    if content.is_empty() {
                        continue;
                    }

                    let sender_id = account["username"].as_str().unwrap_or("unknown");
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
                    };

                    let _ = bus.send(inbound);
                }
            }

            info!("Mastodon polling loop stopped");
        });

        info!("Mastodon channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Mastodon channel");
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

        self.post_status(&msg.content, in_reply_to).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bus() -> broadcast::Sender<InboundMessage> {
        let (tx, _) = broadcast::channel(256);
        tx
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
        assert_eq!(
            strip_html_tags("<a href=\"url\">link</a>"),
            "link"
        );
    }

    #[test]
    fn test_strip_html_entities() {
        assert_eq!(strip_html_tags("a &amp; b"), "a & b");
        assert_eq!(strip_html_tags("&lt;tag&gt;"), "<tag>");
    }

    #[tokio::test]
    async fn test_mastodon_channel_new_validates() {
        let config = MastodonConfig {
            server: String::new(),
            access_token: String::new(),
            allow_from: Vec::new(),
        };
        assert!(MastodonChannel::new(config, test_bus()).is_err());
    }

    #[tokio::test]
    async fn test_mastodon_channel_lifecycle() {
        let config = MastodonConfig {
            server: "https://mastodon.social".to_string(),
            access_token: "token".to_string(),
            allow_from: Vec::new(),
        };
        let ch = MastodonChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "mastodon");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_seen_notification_tracking() {
        let config = MastodonConfig {
            server: "https://mastodon.social".to_string(),
            access_token: "token".to_string(),
            allow_from: Vec::new(),
        };
        let ch = MastodonChannel::new(config, test_bus()).unwrap();

        assert!(!ch.is_seen("notif-1"));
        ch.mark_seen("notif-1");
        assert!(ch.is_seen("notif-1"));
    }

    #[test]
    fn test_notifications_url() {
        let config = MastodonConfig {
            server: "https://mastodon.social".to_string(),
            access_token: "token".to_string(),
            allow_from: Vec::new(),
        };
        let ch = MastodonChannel::new(config, test_bus()).unwrap();
        assert_eq!(
            ch.notifications_url(),
            "https://mastodon.social/api/v1/notifications"
        );
    }

    // ---- New tests ----

    #[test]
    fn test_strip_html_complex() {
        assert_eq!(strip_html_tags("<div><p>para1</p><p>para2</p></div>"), "para1para2");
        assert_eq!(strip_html_tags("no html here"), "no html here");
        assert_eq!(strip_html_tags(""), "");
        assert_eq!(strip_html_tags("&nbsp;"), "\u{a0}");
    }

    #[test]
    fn test_mastodon_config_with_allow_from() {
        let config = MastodonConfig {
            server: "https://m.social".into(),
            access_token: "tok".into(),
            allow_from: vec!["@user@m.social".into()],
        };
        assert_eq!(config.allow_from.len(), 1);
    }

    #[test]
    fn test_seen_notification_multiple() {
        let config = MastodonConfig {
            server: "https://mastodon.social".into(),
            access_token: "token".into(),
            allow_from: Vec::new(),
        };
        let ch = MastodonChannel::new(config, test_bus()).unwrap();
        for i in 0..20 {
            assert!(!ch.is_seen(&format!("n-{}", i)));
            ch.mark_seen(&format!("n-{}", i));
            assert!(ch.is_seen(&format!("n-{}", i)));
        }
    }

    #[test]
    fn test_status_url() {
        let config = MastodonConfig {
            server: "https://mastodon.social".into(),
            access_token: "token".into(),
            allow_from: Vec::new(),
        };
        let ch = MastodonChannel::new(config, test_bus()).unwrap();
        let url = ch.status_url("12345");
        assert!(url.contains("/api/v1/statuses/12345"));
    }

    #[tokio::test]
    async fn test_mastodon_double_stop() {
        let config = MastodonConfig {
            server: "https://mastodon.social".into(),
            access_token: "token".into(),
            allow_from: Vec::new(),
        };
        let ch = MastodonChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();
        ch.stop().await.unwrap();
        ch.stop().await.unwrap();
    }
}
