//! NemesisBot - Channel System
//!
//! Channel adapters for message I/O between external services and the agent engine.
//! Provides a unified `Channel` trait, a `ChannelManager` for lifecycle management,
//! and concrete implementations for various messaging platforms.

pub mod base;
pub mod manager;
pub mod rpc_channel;
pub mod web;
pub mod webhook_inbound;
#[cfg(feature = "websocket")]
pub mod websocket;

// Platform channels (optional, enabled via Cargo features)
#[cfg(feature = "telegram")]
pub mod telegram;
#[cfg(feature = "telegram")]
pub mod telegram_commands;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "slack")]
pub mod slack;

#[cfg(feature = "whatsapp")]
pub mod whatsapp;

#[cfg(feature = "feishu")]
pub mod feishu;

#[cfg(feature = "dingtalk")]
pub mod dingtalk;

#[cfg(feature = "tencent")]
pub mod qq;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "matrix")]
pub mod matrix;

#[cfg(feature = "irc")]
pub mod irc;

#[cfg(feature = "signal")]
pub mod signal;

#[cfg(feature = "mastodon")]
pub mod mastodon;

#[cfg(feature = "bluesky")]
pub mod bluesky;

#[cfg(feature = "onebot")]
pub mod onebot;

// Wave 3（P25-P29）：五新通道——feature 默认关，mod 声明先行开桩
//（实现按工作流填充各自文件/目录，共享注册面不再并行改动）。
#[cfg(feature = "wecom")]
pub mod wecom;

#[cfg(feature = "mattermost")]
pub mod mattermost;

#[cfg(feature = "nostr")]
pub mod nostr;

#[cfg(feature = "mqtt")]
pub mod mqtt;

#[cfg(feature = "wechat")]
pub mod wechat;

#[cfg(feature = "external")]
pub mod external;

#[cfg(feature = "maixcam")]
pub mod maixcam;

#[cfg(feature = "line")]
pub mod line;
