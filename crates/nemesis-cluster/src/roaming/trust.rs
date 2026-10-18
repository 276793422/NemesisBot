//! A1 — TrustBook：iroh 节点本地信任名单（P3）。
//!
//! 信任三层语义（拍板项，PLAN 2026-10-18）：
//! 1. iroh NodeId = 传输身份（连接到谁——公钥即地址，冒名连不上）；
//! 2. 集群 AEAD token = 应用授权（连上后能不能说话）；
//! 3. TrustBook = 本地 allowlist（**可选**第三层）——文件存在即启用：
//!    只放行 trusted 节点且 revoked 一票否决；文件不存在 = 未启用，
//!    现有 token 单层语义字节不变（向后兼容）。
//!
//! 对齐 goose-roaming 模型：连接卡不携带权限，信任由本地名单裁决、可随时
//! 吊销（revoked 优先于 trusted——同 id 同时出现在两节时拒）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// TrustBook 文件路径（`<workspace>/cluster/trust.toml`）。
pub fn trust_book_path(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("trust.toml")
}

/// trust.toml 磁盘形态（node_id 均为 64 字符小写 hex）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustBookData {
    /// 放行名单（空 + revoked 空 = 文件在但全拒——显式关门的合法形态）。
    #[serde(default)]
    pub trusted: Vec<String>,
    /// 吊销名单（一票否决，优先于 trusted；留痕不删——吊销历史可审计）。
    #[serde(default)]
    pub revoked: Vec<String>,
}

/// 加载后的 TrustBook（None = 未启用：文件不存在）。
#[derive(Debug, Clone)]
pub struct TrustBook {
    data: TrustBookData,
    trusted: HashSet<String>,
    revoked: HashSet<String>,
}

impl TrustBook {
    /// 从 workspace 加载；文件不存在 → None（未启用，调用方走旧语义）。
    pub fn load(workspace: &Path) -> Result<Option<Self>, String> {
        let path = trust_book_path(workspace);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let data: TrustBookData =
            toml::from_str(&text).map_err(|e| format!("trust.toml 解析失败: {e}"))?;
        Ok(Some(Self::from_data(data)))
    }

    /// 从数据构造（归一化剥 `iroh:` 前缀 + 小写——与 [`Self::is_trusted`]
    /// 裁决同口径；非法形态条目照收——裁决时非 hex 必不命中 trusted，
    /// 不因脏数据炸掉整个信任层）。
    pub fn from_data(data: TrustBookData) -> Self {
        let norm =
            |v: &Vec<String>| -> HashSet<String> { v.iter().map(|s| normalize_id(s)).collect() };
        Self {
            trusted: norm(&data.trusted),
            revoked: norm(&data.revoked),
            data,
        }
    }

    /// 未启用判断（空表 = 逻辑上仍在，见 [`is_trusted`]）。
    pub fn enabled(&self) -> bool {
        true
    }

    /// 裁决：revoked 一票否决 → trusted 放行 → 默认拒（allowlist 语义）。
    ///
    /// 输入统一小写归一（与 `from_data` 同口径；调用方传 hex 或 `iroh:` 前缀
    /// 地址皆可——前缀在这里剥掉）。
    pub fn is_trusted(&self, node_id_hex: &str) -> bool {
        let id = node_id_hex
            .trim()
            .trim_start_matches("iroh:")
            .to_lowercase();
        if self.revoked.contains(&id) {
            return false;
        }
        self.trusted.contains(&id)
    }

    /// 磁盘数据快照（编辑面/CLI 展示用）。
    pub fn data(&self) -> &TrustBookData {
        &self.data
    }

    /// trust 节点（幂等；不清洗 revoked——吊销是显式人工动作，须走
    /// [`revoke_entries`] 移除）。
    pub fn trust(&mut self, node_id_hex: &str) {
        let id = normalize_id(node_id_hex);
        if !self.data.trusted.iter().any(|s| normalize_id(s) == id) {
            self.data.trusted.push(id.clone());
        }
        self.trusted.insert(id);
    }

    /// 吊销节点（幂等；同时从 trusted 移除生效——revoked 留痕在 revoked 节）。
    pub fn revoke(&mut self, node_id_hex: &str) {
        let id = normalize_id(node_id_hex);
        self.data.trusted.retain(|s| normalize_id(s) != id);
        self.trusted.remove(&id);
        if !self.data.revoked.iter().any(|s| normalize_id(s) == id) {
            self.data.revoked.push(id.clone());
        }
        self.revoked.insert(id);
    }

    /// 从 revoked 移除（解除吊销；不动 trusted——重新信任是显式 trust 动作）。
    pub fn unrevoke(&mut self, node_id_hex: &str) {
        let id = normalize_id(node_id_hex);
        self.data.revoked.retain(|s| normalize_id(s) != id);
        self.revoked.remove(&id);
    }

    /// 落盘（父目录自动创建；TOML 序列化）。
    pub fn save(&self, workspace: &Path) -> Result<(), String> {
        let path = trust_book_path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        let text = toml::to_string_pretty(&self.data)
            .map_err(|e| format!("trust.toml 序列化失败: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("写 {} 失败: {e}", path.display()))
    }
}

/// node_id 归一（剥 `iroh:` 前缀 + 小写；裁决与存储同口径）。
fn normalize_id(s: &str) -> String {
    s.trim().trim_start_matches("iroh:").to_lowercase()
}
