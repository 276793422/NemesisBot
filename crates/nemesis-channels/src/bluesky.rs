//! Bluesky channel (AT Protocol REST, createSession, notifications).
//!
//! Uses the AT Protocol REST API with polling for notifications and
//! createSession for authentication.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Bluesky channel configuration.
#[derive(Debug, Clone)]
pub struct BlueskyConfig {
    /// Server URL (e.g. "https://bsky.social").
    pub server: String,
    /// Handle (e.g. "nemesisbot.bsky.social").
    pub handle: String,
    /// App password.
    pub password: String,
    /// DID (optional, auto-resolved).
    pub did: Option<String>,
    /// Notification poll interval in seconds (default: 10).
    pub poll_interval: u64,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// Session response.
#[derive(Debug, Deserialize)]
pub struct BlueskySessionResponse {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: Option<String>,
    pub active: Option<bool>,
}

/// Notifications response.
#[derive(Debug, Deserialize)]
pub struct BlueskyNotificationsResponse {
    pub notifications: Vec<BlueskyNotification>,
    pub cursor: Option<String>,
}

/// A single notification.
#[derive(Debug, Deserialize)]
pub struct BlueskyNotification {
    pub id: String,
    pub reason: String,
    pub author: BlueskyActor,
    pub record: Option<serde_json::Value>,
    pub is_read: Option<bool>,
    pub indexed_at: Option<String>,
}

/// Bluesky actor.
#[derive(Debug, Deserialize)]
pub struct BlueskyActor {
    pub did: String,
    pub handle: String,
    pub display_name: Option<String>,
}

/// Post record.
#[derive(Debug, Deserialize)]
pub struct BlueskyPostRecord {
    #[serde(rename = "$type")]
    pub record_type: Option<String>,
    pub text: Option<String>,
    pub created_at: Option<String>,
}

/// Create record request.
#[derive(Serialize)]
struct CreateRecordRequest {
    repo: String,
    collection: String,
    record: serde_json::Value,
}

/// Create record response.
#[derive(Debug, Deserialize)]
struct CreateRecordResponse {
    uri: String,
    cid: Option<String>,
}

/// Get record response.
#[derive(Debug, Deserialize)]
struct GetRecordResponse {
    uri: String,
    cid: String,
}

/// Bluesky channel using AT Protocol REST API.
pub struct BlueskyChannel {
    base: BaseChannel,
    config: BlueskyConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    access_token: Arc<parking_lot::RwLock<String>>,
    did: Arc<parking_lot::RwLock<String>>,
    seen_notifs: parking_lot::RwLock<HashMap<String, bool>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl BlueskyChannel {
    /// Creates a new `BlueskyChannel`.
    pub fn new(config: BlueskyConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.server.is_empty() || config.handle.is_empty() || config.password.is_empty() {
            return Err(NemesisError::Channel(
                "bluesky server, handle, and password are required".to_string(),
            ));
        }

        let server = config.server.trim_end_matches('/').to_string();
        let poll_interval = if config.poll_interval == 0 {
            10
        } else {
            config.poll_interval
        };

        Ok(Self {
            base: BaseChannel::new("bluesky"),
            config: BlueskyConfig {
                server,
                poll_interval,
                ..config
            },
            http: reqwest::Client::new(),
            running: Arc::new(parking_lot::RwLock::new(false)),
            access_token: Arc::new(parking_lot::RwLock::new(String::new())),
            did: Arc::new(parking_lot::RwLock::new(String::new())),
            seen_notifs: parking_lot::RwLock::new(HashMap::new()),
            bus_sender,
        })
    }

    /// Creates a session and stores the access token.
    pub async fn create_session(&self) -> Result<(String, String)> {
        let url = format!(
            "{}/xrpc/com.atproto.server.createSession",
            self.config.server
        );

        let payload = serde_json::json!({
            "identifier": self.config.handle,
            "password": self.config.password,
        });

        let resp = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("bluesky session failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "bluesky session error: {body}"
            )));
        }

        let session: BlueskySessionResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("bluesky session parse failed: {e}")))?;

        *self.access_token.write() = session.access_jwt.clone();
        let did = self.config.did.clone().unwrap_or(session.did.clone());
        *self.did.write() = did.clone();

        Ok((did, session.handle))
    }

    /// Resolves a record's CID.
    pub async fn resolve_record_cid(&self, at_uri: &str) -> Result<String> {
        let uri = at_uri.strip_prefix("at://").unwrap_or(at_uri);
        let parts: Vec<&str> = uri.splitn(3, '/').collect();
        if parts.len() < 3 {
            return Err(NemesisError::Channel(format!(
                "invalid AT URI: {at_uri}"
            )));
        }

        let url = format!(
            "{}/xrpc/com.atproto.repo.getRecord?repo={}&collection={}&rkey={}",
            self.config.server, parts[0], parts[1], parts[2]
        );

        let token = self.access_token.read().clone();
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("getRecord failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "getRecord error: {body}"
            )));
        }

        let result: GetRecordResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("getRecord parse failed: {e}")))?;

        Ok(result.cid)
    }

    /// Posts a reply.
    pub async fn post_reply(&self, parent_uri: &str, content: &str) -> Result<String> {
        let parent_cid = self.resolve_record_cid(parent_uri).await?;
        let did = self.did.read().clone();
        let token = self.access_token.read().clone();

        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);

        let record = serde_json::json!({
            "$type": "app.bsky.feed.post",
            "text": content,
            "createdAt": now,
            "reply": {
                "root": { "uri": parent_uri, "cid": parent_cid },
                "parent": { "uri": parent_uri, "cid": parent_cid },
            }
        });

        let request = CreateRecordRequest {
            repo: did,
            collection: "app.bsky.feed.post".to_string(),
            record,
        };

        let url = format!(
            "{}/xrpc/com.atproto.repo.createRecord",
            self.config.server
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&request)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("createRecord failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "createRecord error: {body}"
            )));
        }

        let result: CreateRecordResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("createRecord parse failed: {e}")))?;

        Ok(result.uri)
    }

    /// Builds the AT URI for a notification.
    pub fn build_post_uri(did: &str, rkey: &str) -> String {
        format!("at://{did}/app.bsky.feed.post/{rkey}")
    }

    /// Marks a notification as seen.
    pub fn mark_seen(&self, notif_id: &str) {
        let mut map = self.seen_notifs.write();
        map.insert(notif_id.to_string(), true);
        if map.len() > 500 {
            *map = HashMap::new();
            map.insert(notif_id.to_string(), true);
        }
    }

    /// Checks if a notification has been seen.
    pub fn is_seen(&self, notif_id: &str) -> bool {
        self.seen_notifs.read().contains_key(notif_id)
    }
}

#[async_trait]
impl Channel for BlueskyChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting Bluesky channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let http = self.http.clone();
        let server = self.config.server.clone();
        let handle = self.config.handle.clone();
        let password = self.config.password.clone();
        let poll_interval = self.config.poll_interval;
        let running = self.running.clone();
        let access_token = self.access_token.clone();
        let did_arc = self.did.clone();
        let seen = self.seen_notifs.clone();

        tokio::spawn(async move {
            let poll_dur = std::time::Duration::from_secs(poll_interval);
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            // Create session first
            let session_url = format!("{}/xrpc/com.atproto.server.createSession", server);
            let session_body = serde_json::json!({
                "identifier": handle,
                "password": password,
            });

            loop {
                if !*running.read() {
                    break;
                }

                // Ensure we have a valid access token
                let token = access_token.read().clone();
                let own_did = did_arc.read().clone();
                if token.is_empty() || own_did.is_empty() {
                    match http.post(&session_url).json(&session_body).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            let body: serde_json::Value = resp.json().await.unwrap_or_default();
                            let tok = body["accessJwt"].as_str().unwrap_or("").to_string();
                            let did = body["did"].as_str().unwrap_or("").to_string();
                            if tok.is_empty() {
                                warn!("Bluesky: empty access token from session");
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(max_backoff);
                                continue;
                            }
                            *access_token.write() = tok.clone();
                            *did_arc.write() = did.clone();
                        }
                        Ok(resp) => {
                            warn!("Bluesky: session returned {}", resp.status());
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(max_backoff);
                            continue;
                        }
                        Err(e) => {
                            warn!("Bluesky: session error: {e}");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(max_backoff);
                            continue;
                        }
                    }
                    backoff = std::time::Duration::from_secs(1);
                    continue; // loop to get fresh token
                }

                tokio::time::sleep(poll_dur).await;

                if !*running.read() {
                    break;
                }

                // Poll notifications
                let url = format!(
                    "{}/xrpc/app.bsky.notification.listNotifications?limit=25",
                    server
                );

                let resp = match http
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Bluesky: notification poll error: {e}");
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    warn!("Bluesky: notification poll returned {}", resp.status());
                    if resp.status().as_u16() == 401 {
                        // Session expired, clear it
                        *access_token.write() = String::new();
                    }
                    continue;
                }

                backoff = std::time::Duration::from_secs(1);

                let body: serde_json::Value = match resp.json().await {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                let notifications = match body["notifications"].as_array() {
                    Some(arr) => arr,
                    None => continue,
                };

                for notif in notifications {
                    let reason = notif["reason"].as_str().unwrap_or("");
                    if reason != "mention" && reason != "reply" {
                        continue;
                    }

                    let author = match notif.get("author") {
                        Some(a) => a,
                        None => continue,
                    };

                    let author_did = author["did"].as_str().unwrap_or("");
                    if author_did == own_did {
                        continue;
                    }

                    let record = match notif.get("record") {
                        Some(r) => r,
                        None => continue,
                    };

                    let text = record["text"].as_str().unwrap_or("");
                    if text.is_empty() {
                        continue;
                    }

                    let notif_id = notif["uri"].as_str().unwrap_or("").to_string();

                    // Dedup
                    {
                        let mut map = seen.write();
                        if map.contains_key(&notif_id) {
                            continue;
                        }
                        map.insert(notif_id.clone(), true);
                        if map.len() > 500 {
                            *map = HashMap::new();
                            map.insert(notif_id, true);
                        }
                    }

                    let sender_handle = author["handle"].as_str().unwrap_or("unknown");
                    let inbound = InboundMessage {
                        channel: "bluesky".to_string(),
                        sender_id: sender_handle.to_string(),
                        chat_id: notif["uri"].as_str().unwrap_or("").to_string(),
                        content: text.to_string(),
                        media: Vec::new(),
                        session_key: sender_handle.to_string(),
                        correlation_id: String::new(),
                        metadata: std::collections::HashMap::new(),
                    };

                    let _ = bus.send(inbound);
                }

                // Mark notifications as seen
                let mark_url = format!("{}/xrpc/app.bsky.notification.updateSeen", server);
                let mark_body = serde_json::json!({
                    "seenAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                });
                let _ = http
                    .post(&mark_url)
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&mark_body)
                    .send()
                    .await;
            }

            info!("Bluesky polling loop stopped");
        });

        info!("Bluesky channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Bluesky channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "bluesky channel not running".to_string(),
            ));
        }

        if msg.chat_id.is_empty() {
            return Err(NemesisError::Channel(
                "no post URI specified in chat_id".to_string(),
            ));
        }

        self.base.record_sent();
        debug!(reply_to = %msg.chat_id, "Bluesky posting reply");
        self.post_reply(&msg.chat_id, &msg.content).await?;
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

    #[tokio::test]
    async fn test_bluesky_channel_new_validates() {
        let config = BlueskyConfig {
            server: String::new(),
            handle: String::new(),
            password: String::new(),
            did: None,
            poll_interval: 0,
            allow_from: Vec::new(),
        };
        assert!(BlueskyChannel::new(config, test_bus()).is_err());
    }

    #[tokio::test]
    async fn test_bluesky_channel_lifecycle() {
        let config = BlueskyConfig {
            server: "https://bsky.social".to_string(),
            handle: "test.bsky.social".to_string(),
            password: "password".to_string(),
            did: None,
            poll_interval: 10,
            allow_from: Vec::new(),
        };
        let ch = BlueskyChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "bluesky");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_build_post_uri() {
        let uri = BlueskyChannel::build_post_uri(
            "did:plc:abc123",
            "3k2la7bfx2x2y",
        );
        assert_eq!(uri, "at://did:plc:abc123/app.bsky.feed.post/3k2la7bfx2x2y");
    }

    #[test]
    fn test_seen_tracking() {
        let config = BlueskyConfig {
            server: "https://bsky.social".to_string(),
            handle: "test.bsky.social".to_string(),
            password: "password".to_string(),
            did: None,
            poll_interval: 10,
            allow_from: Vec::new(),
        };
        let ch = BlueskyChannel::new(config, test_bus()).unwrap();

        assert!(!ch.is_seen("notif-1"));
        ch.mark_seen("notif-1");
        assert!(ch.is_seen("notif-1"));
    }

    #[test]
    fn test_default_poll_interval() {
        let config = BlueskyConfig {
            server: "https://bsky.social".to_string(),
            handle: "test.bsky.social".to_string(),
            password: "password".to_string(),
            did: None,
            poll_interval: 0,
            allow_from: Vec::new(),
        };
        let ch = BlueskyChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.config.poll_interval, 10);
    }

    // ---- New tests ----

    #[test]
    fn test_bluesky_config_with_did() {
        let config = BlueskyConfig {
            server: "https://bsky.social".into(),
            handle: "user.bsky.social".into(),
            password: "pass".into(),
            did: Some("did:plc:abc".into()),
            poll_interval: 30,
            allow_from: vec!["did:plc:other".into()],
        };
        assert!(config.did.is_some());
        assert_eq!(config.poll_interval, 30);
    }

    #[test]
    fn test_seen_tracking_multiple() {
        let config = BlueskyConfig {
            server: "https://bsky.social".to_string(),
            handle: "test.bsky.social".to_string(),
            password: "password".to_string(),
            did: None,
            poll_interval: 10,
            allow_from: Vec::new(),
        };
        let ch = BlueskyChannel::new(config, test_bus()).unwrap();

        for i in 0..10 {
            assert!(!ch.is_seen(&format!("n-{}", i)));
            ch.mark_seen(&format!("n-{}", i));
            assert!(ch.is_seen(&format!("n-{}", i)));
        }
    }

    #[test]
    fn test_build_post_uri_various() {
        let uri = BlueskyChannel::build_post_uri("did:plc:test123", "abc");
        assert!(uri.starts_with("at://"));
        assert!(uri.contains("did:plc:test123"));
        assert!(uri.ends_with("/abc"));
    }

    #[tokio::test]
    async fn test_bluesky_double_stop() {
        let config = BlueskyConfig {
            server: "https://bsky.social".to_string(),
            handle: "test.bsky.social".to_string(),
            password: "password".to_string(),
            did: None,
            poll_interval: 10,
            allow_from: Vec::new(),
        };
        let ch = BlueskyChannel::new(config, test_bus()).unwrap();
        ch.start().await.unwrap();
        ch.stop().await.unwrap();
        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }
}
