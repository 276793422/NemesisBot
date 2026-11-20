//! #47 记忆投毒隔离（W2 批三，2026-11-20 拍板）：可疑记忆「隔离态入库」
//! ——入库 + `quarantine` metadata 章 + 不参与召回 + 可人工放行。
//! 拒收会丢证据，隔离态断掉感染路径（召回面全过滤）且留取证现场
//! （章内带命中规则/原因/时间/来源）。
//!
//! 分层：本模块 = metadata 编解码 + [`MemoryContentGate`] trait（
//! nemesis-memory 不依赖 nemesis-config/security——扫描器实现由宿主侧
//! （nemesisbot 装配层，桥 nemesis-security::preinstall 双信号）注入；
//! trait 缺席 = 闸未装配，写入直通（现形态，升级零行为突变））。
//!
//! 三存储面覆盖：
//! - **general/vector**（[`crate::manager::MemoryManager`]）：写入门 +
//!   `store_entry_checked` 面向 WSAPI 投影命中记录；召回过滤在 vector
//!   `query_with_embedding` + manager keyword 臂 + `get`；放行 = 摘章 +
//!   `update_entry`（不重嵌入，双存储同步修正）。
//! - **episodic**（agent `memory_store` episodic 臂）：写入前扫描，命中
//!   照常入库但盖 episode.metadata 章 + 对模型诚实回执；召回过滤在
//!   `search_episodic`/`get_recent_episodes`；放行 = delete_by_id + 重 append
//!   （episode id 不可原位改，新 id 重灌）。
//! - **graph**（entity/triple）：命中**诚实拒绝**不落盘（v1 裁剪：图结构
//!   没有可隔离的「条目元数据」位——entity properties 可以塞章但 triple
//!   没有载体；半吊子隔离比拒绝更危险，拍板记录同款不对称注记）。
//!
//! `is_quarantined` **fail-closed**：章在场即隔离（解析失败按隔离处理）
//! ——放行路径是唯一能摘章的面，读面绝不因章损坏而误放行。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// quarantine 章的 metadata 键（Entry.metadata / Episode.metadata 共用；
/// 值 = [`QuarantineRecord`] 的 JSON 序列化）。
pub const META_QUARANTINE: &str = "quarantine";

/// 隔离记录（章的值；JSON 序列化进 metadata——metadata 是
/// `HashMap<String, String>`，结构化信息以字符串形态承载）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineRecord {
    /// 人读原因（注入检测命中 / 规则命中摘要首条等）。
    pub reason: String,
    /// 命中标识列表（注入 pattern 名 / 规则 id）。
    pub matched: Vec<String>,
    /// RFC3339 隔离时间。
    pub at: String,
    /// 写入来源（`wsapi` / `agent-memory_store` / `unknown` 等）。
    pub source: String,
}

impl QuarantineRecord {
    pub fn new(reason: String, matched: Vec<String>, source: &str) -> Self {
        Self {
            reason,
            matched,
            at: chrono::Utc::now().to_rfc3339(),
            source: source.to_string(),
        }
    }

    /// 序列化为章值（metadata 存储形态）。
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            // 序列化失败（不自反类型不可能，防御臂）：退化存 reason 纯文本
            // ——章仍在场 = 仍隔离（fail-closed 语义不破）。
            self.reason.clone()
        })
    }
}

/// 是否隔离（**fail-closed**：章在场即隔离，不看值能否解析）。
pub fn is_quarantined(metadata: &HashMap<String, String>) -> bool {
    metadata.contains_key(META_QUARANTINE)
}

/// 解析章值；损坏（非法 JSON）→ None 但 [`is_quarantined`] 仍为 true
/// （展示面给「章损坏」注记，放行面按摘章处理）。
pub fn parse_quarantine(metadata: &HashMap<String, String>) -> Option<QuarantineRecord> {
    let raw = metadata.get(META_QUARANTINE)?;
    serde_json::from_str(raw).ok()
}

/// 盖章（写入面：manager / memory_tools 在 store 前 mark）。
pub fn mark_quarantine(metadata: &mut HashMap<String, String>, record: &QuarantineRecord) {
    metadata.insert(META_QUARANTINE.to_string(), record.encode());
}

/// 摘章（放行面唯一入口）：返回被摘除的原始章值（无章 = None）。
pub fn unmark_quarantine(metadata: &mut HashMap<String, String>) -> Option<String> {
    metadata.remove(META_QUARANTINE)
}

// ---------------------------------------------------------------------------
// 内容闸 trait（扫描器实现由宿主侧注入）
// ---------------------------------------------------------------------------

/// 内容闸：扫描一条待写入文本，命中 → 返回隔离记录（None = 干净 / 闸
/// 未装配）。同步签名：扫描是本地纯计算（注入检测 regex + 规则求值，
/// 无 IO 无网络），毫秒级；写入路径可承受。
pub trait MemoryContentGate: Send + Sync {
    fn scan(&self, content: &str, source: &str) -> Option<QuarantineRecord>;
}
