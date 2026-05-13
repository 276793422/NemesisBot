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

pub mod external;
pub mod maixcam;

pub mod line;
