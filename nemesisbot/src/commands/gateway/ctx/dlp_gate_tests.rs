//! （扩展计划）：出站 DLP 脱敏闸单测。
//!
//! 被测 = `apply_outbound_dlp`（agent outbound → bus 桥内逐消息判定，
//! agent 产出的唯一漏斗）。装配顺序语义：Step 9 桥先于 Step 9b 安全装配
//! 创建——槽空 = 直通（装配前不可能有出站流量）；init_agent 回填后逐消息
//! 脱敏 + 审计链留痕（审计只记命中摘要，不落原始内容）。

use std::sync::Arc;
use std::sync::OnceLock;

use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
use nemesis_types::channel::OutboundMessage;

use super::apply_outbound_dlp;

type Slot = OnceLock<Option<Arc<SecurityPlugin>>>;

/// 空槽（init_agent 装配前）= 直通，消息字节不变。
#[test]
fn empty_slot_passthrough() {
    let slot = Slot::new();
    let original = "api_key = aabbccdd11223344556677889900aabb";
    let mut msg = OutboundMessage::new("web", "chat-1", original);
    apply_outbound_dlp(&slot, &mut msg);
    assert_eq!(msg.content, original);
}

/// 安全关闭（Some(None)）= 直通。
#[test]
fn security_disabled_passthrough() {
    let slot = Slot::new();
    let _ = slot.set(None);
    let original = "token = aabbccdd11223344556677889900aabb";
    let mut msg = OutboundMessage::new("web", "chat-1", original);
    apply_outbound_dlp(&slot, &mut msg);
    assert_eq!(msg.content, original);
}

/// 默认装配（DLP 开）= 凭据模式命中 → 内容替换为脱敏文本，原始密钥不再出现。
#[test]
fn api_key_redacted() {
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig::default()));
    let slot = Slot::new();
    let _ = slot.set(Some(plugin));

    let secret = "aabbccdd11223344556677889900aabb";
    let original = format!("连接成功，api_key = {secret}");
    let mut msg = OutboundMessage::new("web", "chat-1", &original);
    apply_outbound_dlp(&slot, &mut msg);
    assert_ne!(msg.content, original);
    assert!(
        !msg.content.contains(secret),
        "redacted content leaked the key: {}",
        msg.content
    );
}

/// 干净文本（无凭据模式命中）= 字节不变（DLP 不改写正常出站内容）。
#[test]
fn clean_message_byte_identical() {
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig::default()));
    let slot = Slot::new();
    let _ = slot.set(Some(plugin));
    let original = "任务完成，共修改 3 个文件。";
    let mut msg = OutboundMessage::new("web", "chat-1", original);
    apply_outbound_dlp(&slot, &mut msg);
    assert_eq!(msg.content, original);
}

/// 仅观测档命中（BUG-20261110-046 邻域复查修复）：Low/log 档（IP/邮箱）
/// 直通不改写——旧行为把正常回复里的地址/联系方式打成 [REDACTED]。
/// 出站打码必须由 enforcing 档（block/redact）授权，与 output_rescan
/// 出侧语义同源。
#[test]
fn low_confidence_only_message_passthrough() {
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig::default()));
    let slot = Slot::new();
    let _ = slot.set(Some(plugin));
    let original = "网关监听 http://127.0.0.1:49000，联系 user@example.com 反馈。";
    let mut msg = OutboundMessage::new("web", "chat-1", original);
    apply_outbound_dlp(&slot, &mut msg);
    assert_eq!(
        msg.content, original,
        "观测档命中不得改写出站消息: {}",
        msg.content
    );
}

/// 混合命中（enforcing + 观测档）= 整体脱敏照旧（打码授权后遮蔽超集，
/// 与 output_rescan redact_content 全文遮蔽同语义）。
#[test]
fn mixed_hits_still_redacted() {
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig::default()));
    let slot = Slot::new();
    let _ = slot.set(Some(plugin));
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let original = format!("已部署到 10.0.0.5，key is {secret}");
    let mut msg = OutboundMessage::new("web", "chat-1", &original);
    apply_outbound_dlp(&slot, &mut msg);
    assert!(
        !msg.content.contains(secret),
        "enforcing 命中必须遮蔽: {}",
        msg.content
    );
}

/// 审计链开启 = 脱敏同时落审计事件（入链且链校验通过；事件 reason 为
/// 命中摘要，不含原始密钥）。
#[test]
fn redaction_writes_audit_event() {
    let chain_path = std::env::temp_dir().join(format!(
        "nmb_dlp_gate_test_{}_chain.jsonl",
        std::process::id()
    ));
    let mut config = SecurityPluginConfig::default();
    config.audit_chain_enabled = true;
    config.audit_chain_path = Some(chain_path.to_string_lossy().into_owned());
    let plugin = Arc::new(SecurityPlugin::new(config));
    let slot = Slot::new();
    let _ = slot.set(Some(plugin));

    // aws_access_key 形态（AKIA + 16 位大写数字，High 置信）。
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let mut msg = OutboundMessage::new("telegram", "42", &format!("key is {secret}"));
    apply_outbound_dlp(&slot, &mut msg);
    assert!(
        !msg.content.contains(secret),
        "redacted content leaked the key: {}",
        msg.content
    );

    let plugin = slot.get().unwrap().as_ref().unwrap();
    let chain = plugin.audit_chain().expect("audit chain enabled");
    assert!(chain.event_count() >= 1, "redaction event not recorded");
    let last = chain.get_event((chain.event_count() - 1) as usize).unwrap();
    assert_eq!(last.operation, "dlp_outbound_redact");
    assert_eq!(last.decision, "redacted");
    assert_eq!(last.target, "telegram/42");
    assert!(!last.reason.contains(secret), "audit reason leaked the key");
    assert!(
        chain
            .verify_range(0, (chain.event_count() - 1) as usize)
            .unwrap()
    );
    let _ = std::fs::remove_file(&chain_path);
}
