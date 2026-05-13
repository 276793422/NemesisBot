//! Matrix channel (/sync long polling, incremental sync, room events).
//!
//! Uses the Matrix Client-Server API with long-poll /sync for receiving
//! messages and /rooms/{roomId}/send/m.room.message for sending.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use nemesis_types::channel::{InboundMessage, OutboundMessage};
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Matrix channel configuration.
#[derive(Debug, Clone)]
pub struct MatrixConfig {
    /// Homeserver URL (e.g. "https://matrix.org").
    pub homeserver: String,
    /// User ID (e.g. "@bot:matrix.org").
    pub user_id: String,
    /// Access token.
    pub access_token: String,
    /// Default room ID.
    pub room_id: Option<String>,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

/// Matrix sync response.
#[derive(Debug, Deserialize)]
pub struct MatrixSyncResponse {
    pub next_batch: String,
    pub rooms: Option<MatrixRooms>,
}

/// Matrix rooms section.
#[derive(Debug, Deserialize)]
pub struct MatrixRooms {
    pub join: Option<std::collections::HashMap<String, MatrixJoinedRoom>>,
}

/// A joined room in sync response.
#[derive(Debug, Deserialize)]
pub struct MatrixJoinedRoom {
    pub timeline: Option<MatrixTimeline>,
}

/// Room timeline.
#[derive(Debug, Deserialize)]
pub struct MatrixTimeline {
    pub events: Vec<MatrixEvent>,
}

/// A Matrix event.
#[derive(Debug, Deserialize)]
pub struct MatrixEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub content: Option<MatrixContent>,
    pub sender: Option<String>,
    pub event_id: Option<String>,
    pub origin_server_ts: Option<i64>,
}

/// Matrix event content.
#[derive(Debug, Deserialize)]
pub struct MatrixContent {
    pub msgtype: Option<String>,
    pub body: Option<String>,
}

/// Matrix send response.
#[derive(Debug, Deserialize)]
struct MatrixSendResponse {
    event_id: String,
}

/// Matrix whoami response.
#[derive(Debug, Deserialize)]
struct MatrixWhoamiResponse {
    user_id: String,
}

/// Matrix channel using Client-Server API.
pub struct MatrixChannel {
    base: BaseChannel,
    config: MatrixConfig,
    http: reqwest::Client,
    running: Arc<parking_lot::RwLock<bool>>,
    since_token: Arc<parking_lot::RwLock<Option<String>>>,
    bus_sender: broadcast::Sender<InboundMessage>,
}

impl MatrixChannel {
    /// Creates a new `MatrixChannel`.
    pub fn new(config: MatrixConfig, bus_sender: broadcast::Sender<InboundMessage>) -> Result<Self> {
        if config.homeserver.is_empty() || config.access_token.is_empty() {
            return Err(NemesisError::Channel(
                "matrix homeserver and access_token are required".to_string(),
            ));
        }

        let homeserver = config.homeserver.trim_end_matches('/').to_string();

        Ok(Self {
            base: BaseChannel::new("matrix"),
            config: MatrixConfig { homeserver, ..config },
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| NemesisError::Channel(format!("HTTP client creation failed: {e}")))?,
            running: Arc::new(parking_lot::RwLock::new(false)),
            since_token: Arc::new(parking_lot::RwLock::new(None)),
            bus_sender,
        })
    }

    /// Returns the homeserver URL.
    pub fn homeserver(&self) -> &str {
        &self.config.homeserver
    }

    /// Verifies credentials via /account/whoami.
    pub async fn verify_credentials(&self) -> Result<String> {
        let url = format!(
            "{}/_matrix/client/v3/account/whoami",
            self.config.homeserver
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix whoami failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "matrix whoami error: {body}"
            )));
        }

        let result: MatrixWhoamiResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix whoami parse failed: {e}")))?;

        Ok(result.user_id)
    }

    /// Performs initial sync to get the next_batch token.
    pub async fn initial_sync(&self) -> Result<String> {
        let url = format!(
            "{}/_matrix/client/v3/sync?timeout=0&filter={{\"room\":{{\"timeline\":{{\"limit\":0}}}}}}",
            self.config.homeserver
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix initial sync failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "matrix initial sync error: {body}"
            )));
        }

        let sync_resp: MatrixSyncResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix sync parse failed: {e}")))?;

        *self.since_token.write() = Some(sync_resp.next_batch.clone());
        Ok(sync_resp.next_batch)
    }

    /// Performs a long-poll sync.
    pub async fn do_sync(&self, timeout_ms: u32) -> Result<MatrixSyncResponse> {
        let since = self.since_token.read().clone();
        let mut url = format!(
            "{}/_matrix/client/v3/sync?timeout={}",
            self.config.homeserver, timeout_ms
        );
        if let Some(s) = since {
            url.push_str("&since=");
            url.push_str(&s);
        }

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix sync failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "matrix sync error: {body}"
            )));
        }

        let sync_resp: MatrixSyncResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix sync parse failed: {e}")))?;

        *self.since_token.write() = Some(sync_resp.next_batch.clone());
        Ok(sync_resp)
    }

    /// Sends a text message to a room.
    pub async fn send_room_message(&self, room_id: &str, content: &str) -> Result<String> {
        let txn_id = format!("nb_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            self.config.homeserver, room_id, txn_id
        );

        let payload = serde_json::json!({
            "msgtype": "m.text",
            "body": content,
        });

        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix send failed: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Channel(format!(
                "matrix send error: {body}"
            )));
        }

        let result: MatrixSendResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Channel(format!("matrix send parse failed: {e}")))?;

        Ok(result.event_id)
    }

    /// Processes events from a sync response.
    pub fn process_sync_events(
        &self,
        sync: &MatrixSyncResponse,
        bot_user_id: &str,
    ) -> Vec<(String, String, String)> {
        let mut messages = Vec::new();

        if let Some(ref rooms) = sync.rooms {
            if let Some(ref join) = rooms.join {
                for (room_id, room_data) in join {
                    if let Some(ref timeline) = room_data.timeline {
                        for event in &timeline.events {
                            if event.event_type != "m.room.message" {
                                continue;
                            }

                            let sender = match event.sender {
                                Some(ref s) => s,
                                None => continue,
                            };

                            // Ignore our own messages
                            if sender == bot_user_id {
                                continue;
                            }

                            let content = match event.content {
                                Some(ref c) => {
                                    if c.msgtype.as_deref() != Some("m.text") {
                                        continue;
                                    }
                                    c.body.as_deref().unwrap_or("")
                                }
                                None => continue,
                            };

                            if content.is_empty() {
                                continue;
                            }

                            messages.push((
                                sender.clone(),
                                room_id.clone(),
                                content.to_string(),
                            ));
                        }
                    }
                }
            }
        }

        messages
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    async fn start(&self) -> Result<()> {
        info!("starting Matrix channel");
        *self.running.write() = true;
        self.base.set_enabled(true);

        let bus = self.bus_sender.clone();
        let http = self.http.clone();
        let homeserver = self.config.homeserver.clone();
        let access_token = self.config.access_token.clone();
        let bot_user_id = self.config.user_id.clone();
        let running = self.running.clone();
        let since_token = self.since_token.clone();

        // Do initial sync to get the since token, skipping old messages
        if since_token.read().is_none() {
            if let Ok(token) = self.initial_sync().await {
                *since_token.write() = Some(token);
            }
        }

        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            let max_backoff = std::time::Duration::from_secs(60);

            loop {
                if !*running.read() {
                    break;
                }

                // Build /sync URL
                let since = since_token.read().clone();
                let mut url = format!(
                    "{}/_matrix/client/v3/sync?timeout=30000&filter={{\"room\":{{\"timeline\":{{\"limit\":10}}}}}}",
                    homeserver
                );
                if let Some(ref token) = since {
                    url.push_str(&format!("&since={token}"));
                }

                let resp = match http
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Matrix sync error: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(max_backoff);
                        continue;
                    }
                };

                if !resp.status().is_success() {
                    warn!("Matrix sync returned {}", resp.status());
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }

                backoff = std::time::Duration::from_secs(1);

                let body: serde_json::Value = match resp.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        warn!("Matrix sync parse error: {e}");
                        continue;
                    }
                };

                // Update since token
                if let Some(next) = body["next_batch"].as_str() {
                    *since_token.write() = Some(next.to_string());
                }

                // Process room events
                if let Some(rooms) = body["rooms"]["join"].as_object() {
                    for (room_id, room_data) in rooms {
                        if let Some(events) = room_data["timeline"]["events"].as_array() {
                            for event in events {
                                let event_type = event["type"].as_str().unwrap_or("");
                                if event_type != "m.room.message" {
                                    continue;
                                }

                                let sender = event["sender"].as_str().unwrap_or("");
                                if sender == bot_user_id {
                                    continue;
                                }

                                let content = event["content"]["body"]
                                    .as_str()
                                    .unwrap_or("");
                                if content.is_empty() {
                                    continue;
                                }

                                let inbound = InboundMessage {
                                    channel: "matrix".to_string(),
                                    sender_id: sender.to_string(),
                                    chat_id: room_id.clone(),
                                    content: content.to_string(),
                                    media: Vec::new(),
                                    session_key: room_id.clone(),
                                    correlation_id: String::new(),
                                    metadata: std::collections::HashMap::new(),
                                };

                                let _ = bus.send(inbound);
                            }
                        }
                    }
                }
            }

            info!("Matrix sync loop stopped");
        });

        info!("Matrix channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("stopping Matrix channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "matrix channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        let room_id = if msg.chat_id.is_empty() {
            self.config.room_id.as_deref().ok_or_else(|| {
                NemesisError::Channel("no room ID specified".to_string())
            })?
        } else {
            &msg.chat_id
        };

        debug!(room_id = %room_id, "Matrix sending message");
        self.send_room_message(room_id, &msg.content).await?;
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
    async fn test_matrix_channel_new_validates() {
        let config = MatrixConfig {
            homeserver: String::new(),
            user_id: String::new(),
            access_token: String::new(),
            room_id: None,
            allow_from: Vec::new(),
        };
        assert!(MatrixChannel::new(config, test_bus()).is_err());
    }

    #[tokio::test]
    async fn test_matrix_channel_lifecycle() {
        let config = MatrixConfig {
            homeserver: "https://matrix.org".to_string(),
            user_id: "@bot:matrix.org".to_string(),
            access_token: "token".to_string(),
            room_id: Some("!room:matrix.org".to_string()),
            allow_from: Vec::new(),
        };
        let ch = MatrixChannel::new(config, test_bus()).unwrap();
        assert_eq!(ch.name(), "matrix");

        ch.start().await.unwrap();
        assert!(*ch.running.read());

        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }

    #[test]
    fn test_process_sync_events() {
        let config = MatrixConfig {
            homeserver: "https://matrix.org".to_string(),
            user_id: "@bot:matrix.org".to_string(),
            access_token: "token".to_string(),
            room_id: None,
            allow_from: Vec::new(),
        };
        let ch = MatrixChannel::new(config, test_bus()).unwrap();

        let sync = MatrixSyncResponse {
            next_batch: "batch-2".to_string(),
            rooms: Some(MatrixRooms {
                join: Some({
                    let mut map = std::collections::HashMap::new();
                    map.insert(
                        "!room:matrix.org".to_string(),
                        MatrixJoinedRoom {
                            timeline: Some(MatrixTimeline {
                                events: vec![
                                    MatrixEvent {
                                        event_type: "m.room.message".to_string(),
                                        content: Some(MatrixContent {
                                            msgtype: Some("m.text".to_string()),
                                            body: Some("Hello".to_string()),
                                        }),
                                        sender: Some("@user:matrix.org".to_string()),
                                        event_id: Some("$event1".to_string()),
                                        origin_server_ts: Some(1234567890),
                                    },
                                    MatrixEvent {
                                        event_type: "m.room.message".to_string(),
                                        content: Some(MatrixContent {
                                            msgtype: Some("m.text".to_string()),
                                            body: Some("Bot message".to_string()),
                                        }),
                                        sender: Some("@bot:matrix.org".to_string()),
                                        event_id: Some("$event2".to_string()),
                                        origin_server_ts: Some(1234567891),
                                    },
                                ],
                            }),
                        },
                    );
                    map
                }),
            }),
        };

        let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, "@user:matrix.org");
        assert_eq!(messages[0].2, "Hello");
    }

    // ---- New tests ----

    #[test]
    fn test_matrix_config_fields() {
        let config = MatrixConfig {
            homeserver: "https://matrix.org".into(),
            user_id: "@bot:matrix.org".into(),
            access_token: "secret".into(),
            room_id: Some("!room:matrix.org".into()),
            allow_from: vec!["@user:matrix.org".into()],
        };
        assert_eq!(config.homeserver, "https://matrix.org");
        assert_eq!(config.user_id, "@bot:matrix.org");
        assert!(config.room_id.is_some());
        assert_eq!(config.allow_from.len(), 1);
    }

    #[test]
    fn test_matrix_sync_response_deserialize() {
        let json = r#"{"next_batch":"s1","rooms":{"join":{"!r:m.org":{"timeline":{"events":[{"type":"m.room.message","content":{"msgtype":"m.text","body":"hi"},"sender":"@u:m.org","event_id":"$e1","origin_server_ts":1}]}}}}}"#;
        let sync: MatrixSyncResponse = serde_json::from_str(json).unwrap();
        assert_eq!(sync.next_batch, "s1");
        assert!(sync.rooms.is_some());
    }

    #[test]
    fn test_matrix_sync_empty_rooms() {
        let json = r#"{"next_batch":"b1"}"#;
        let sync: MatrixSyncResponse = serde_json::from_str(json).unwrap();
        assert!(sync.rooms.is_none());
    }

    #[test]
    fn test_process_sync_empty() {
        let config = MatrixConfig {
            homeserver: "https://matrix.org".to_string(),
            user_id: "@bot:matrix.org".to_string(),
            access_token: "token".to_string(),
            room_id: None,
            allow_from: Vec::new(),
        };
        let ch = MatrixChannel::new(config, test_bus()).unwrap();

        let sync = MatrixSyncResponse {
            next_batch: "b1".to_string(),
            rooms: None,
        };
        let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
        assert!(messages.is_empty());
    }

    #[test]
    fn test_process_sync_non_message_events_ignored() {
        let config = MatrixConfig {
            homeserver: "https://matrix.org".to_string(),
            user_id: "@bot:matrix.org".to_string(),
            access_token: "token".to_string(),
            room_id: None,
            allow_from: Vec::new(),
        };
        let ch = MatrixChannel::new(config, test_bus()).unwrap();

        let sync = MatrixSyncResponse {
            next_batch: "b2".to_string(),
            rooms: Some(MatrixRooms {
                join: Some({
                    let mut map = std::collections::HashMap::new();
                    map.insert("!room:matrix.org".to_string(), MatrixJoinedRoom {
                        timeline: Some(MatrixTimeline {
                            events: vec![
                                MatrixEvent {
                                    event_type: "m.room.member".to_string(),
                                    content: Some(MatrixContent {
                                        msgtype: None,
                                        body: None,
                                    }),
                                    sender: Some("@user:matrix.org".to_string()),
                                    event_id: Some("$e1".to_string()),
                                    origin_server_ts: Some(1),
                                },
                            ],
                        }),
                    });
                    map
                }),
            }),
        };
        let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
        assert!(messages.is_empty());
    }

    #[test]
    fn test_process_sync_allow_from_filter() {
        let config = MatrixConfig {
            homeserver: "https://matrix.org".to_string(),
            user_id: "@bot:matrix.org".to_string(),
            access_token: "token".to_string(),
            room_id: None,
            allow_from: vec!["@allowed:matrix.org".to_string()],
        };
        let ch = MatrixChannel::new(config, test_bus()).unwrap();

        let sync = MatrixSyncResponse {
            next_batch: "b3".to_string(),
            rooms: Some(MatrixRooms {
                join: Some({
                    let mut map = std::collections::HashMap::new();
                    map.insert("!room:matrix.org".to_string(), MatrixJoinedRoom {
                        timeline: Some(MatrixTimeline {
                            events: vec![
                                MatrixEvent {
                                    event_type: "m.room.message".to_string(),
                                    content: Some(MatrixContent {
                                        msgtype: Some("m.text".to_string()),
                                        body: Some("Hello".to_string()),
                                    }),
                                    sender: Some("@blocked:matrix.org".to_string()),
                                    event_id: Some("$e1".to_string()),
                                    origin_server_ts: Some(1),
                                },
                            ],
                        }),
                    });
                    map
                }),
            }),
        };
        let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
        assert!(messages.is_empty()); // sender not in allow_from
    }
}
