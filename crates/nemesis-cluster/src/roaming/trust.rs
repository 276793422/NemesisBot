//! — TrustBook：iroh 节点本地信任名单。
//!
//! 信任三层语义（决策项，PLAN 2026-10-18）：
//! 1. iroh NodeId = 传输身份（连接到谁——公钥即地址，冒名连不上）；
//! 2. 集群 AEAD token = 应用授权（连上后能不能说话）；
//! 3. TrustBook = 本地 allowlist（**可选**第三层）——文件存在即启用：
//!    只放行 trusted 节点且 revoked 一票否决；文件不存在 = 未启用，
//!    现有 token 单层语义字节不变（向后兼容）。
//!
//! 跨网漫游信任模型：连接卡不携带权限，信任由本地名单裁决、可随时
//! 吊销（revoked 优先于 trusted——同 id 同时出现在两节时拒）。

use std::collections::{BTreeMap, HashSet};
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
    /// a（三期）：mandate 签名公钥钉存段——cluster node_id
    /// → P-256 公钥 hex（SEC1 uncompressed 130 / compressed 66，小写）。
    /// 键是 cluster node_id（mandate 校验在 RPC 层按 `rpc_meta.from` 裁决，
    /// 与分发钉存键必须同族）。BTreeMap = 序列化确定序（人工可读可 diff）。
    /// `#[serde(default)]`：-only 存量簿缺段加载为空，零迁移。
    #[serde(default)]
    pub mandate_keys: BTreeMap<String, String>,
    /// mandate 吊销留痕（node_id；优先于 mandate_keys——同 revoked 一票
    /// 否决纪律）。
    #[serde(default)]
    pub mandate_revoked: Vec<String>,
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
        // fsutil 剥 BOM 单一真相源（trust.toml 是手工管理面文件，与
        // peers.toml 同暴露级； 横扫漏网补齐——记事本
        // UTF-8-BOM 形态不再构成坏簿 fail-closed 的意外来源）。
        let text = nemesis_utils::fsutil::read_config_string(&path)
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

    /// 落盘（父目录自动创建；TOML 序列化；原子替换写——TOFU 钉存提升了写频，
    /// 直写 `std::fs:write` 的撕裂面消除）。
    pub fn save(&self, workspace: &Path) -> Result<(), String> {
        let path = trust_book_path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        let text = toml::to_string_pretty(&self.data)
            .map_err(|e| format!("trust.toml 序列化失败: {e}"))?;
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o600)
            .map_err(|e| format!("写 {} 失败: {e}", path.display()))
    }
}

/// node_id 归一（剥 `iroh:` 前缀 + 小写；裁决与存储同口径）。
pub(crate) fn normalize_id(s: &str) -> String {
    s.trim().trim_start_matches("iroh:").to_lowercase()
}

// ---------------------------------------------------------------------------
// a：mandate 签名公钥钉存面。
//
// 设计文档：`-03_L13a节点间公钥分发设计文档`。两个信任维度同一张簿：
// 段（iroh NodeId，连接层 allowlist）/ -a 段（cluster node_id →
// P-256 公钥，授权层 pin）。TOFU_PIN_LOCK 与 iroh 钉存共用——同一文件的
// load-modify-save 并发互相覆盖整簿。on-curve 校验刻意不做（本 crate 不
// 依赖 nemesis-verify；错钥在 -b 验签时自然 loud 失败——分发面只管形态）。
// ---------------------------------------------------------------------------

/// mandate 公钥形态校验 + 归一（trim/小写）。合法 = 66 hex（`02`/`03` 头，
/// SEC1 compressed）或 130 hex（`04` 头，SEC1 uncompressed——nemesis-verify
/// `verifying_key_from_hex` 同口径）。非法 = None（调用方诚实拒绝）。
pub fn normalize_mandate_pubkey(s: &str) -> Option<String> {
    let v = s.trim().to_lowercase();
    let ok = match v.len() {
        66 => v.starts_with("02") || v.starts_with("03"),
        130 => v.starts_with("04"),
        _ => false,
    } && v.chars().all(|c| c.is_ascii_hexdigit());
    if ok { Some(v) } else { None }
}

/// mandate 钉存状态四态（b 验签消费点：Revoked 拒 / Pinned 验签 /
/// Absent 维持现行行为——mandate 缺失兼容旧节点 / BookUnreadable 拒——
/// 簿在场但读失败 = regime 已激活而验签基础设施失效，与「簿缺席（regime
/// 休眠）」语义分立，消费侧不得并入 Absent 做降级放行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MandateKeyVerdict {
    /// 已钉存（值为归一小写 hex）。
    Pinned(String),
    /// 已吊销（一票否决，优先于 keys——同 revoked 纪律）。
    Revoked,
    /// 无钉存（该节点未分发过 mandate 公钥）。
    Absent,
    /// 簿在场但读失败（损坏/编码非法）——与 Absent 的区别：trust.toml
    /// 在场即 regime 已激活，管理员预期验签在跑；坏簿必须 fail-closed
    /// （announce 闸同方向），静默放行 = 已激活 regime 被无声降级。
    BookUnreadable,
}

/// mandate TOFU 钉存裁决（[`mandate_tofu_pin`] 返回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MandatePinOutcome {
    /// 首见，已钉存。
    Pinned,
    /// 已有同值，幂等 noop。
    Unchanged,
    /// 异值冲突：**保留首见**（strict TOFU），轮换走显式 `mandate_pin_forced`。
    Conflict { existing: String },
    /// 节点 mandate 已吊销——吊销态 TOFU 不更新 pin。
    Distrusted,
    /// 簿缺席（regime 未激活）——不建文件不钉存（纪律镜像：announce
    /// 抵达绝不激活 trust.toml）。
    BookAbsent,
}

impl MandatePinOutcome {
    /// 是否落盘成功（钉存语义完成；Conflict/Distrusted/BookAbsent 均未写）。
    pub fn persisted(&self) -> bool {
        matches!(
            self,
            MandatePinOutcome::Pinned | MandatePinOutcome::Unchanged
        )
    }
}

/// mandate TOFU 首见钉存（announce 消费点）。
///
/// TOFU_PIN_LOCK 锁内 load-check-write（与 iroh TOFU 同款防并发覆盖整簿）。
/// 读失败返回 `Err`——调用方 warn 后照常入册（分发失败不伤发现链，下次
/// announce 自愈重钉，同 「钉存写盘失败仍 Adopt」纪律）。
pub fn mandate_tofu_pin(
    workspace: &Path,
    node_id: &str,
    pubkey: &str,
) -> Result<MandatePinOutcome, String> {
    let Some(key) = normalize_mandate_pubkey(pubkey) else {
        return Err(format!("mandate 公钥形态非法（期待 66/130 hex）：{pubkey}"));
    };
    let _serial = TOFU_PIN_LOCK.lock();
    let mut book = match TrustBook::load(workspace)? {
        Some(b) => b,
        None => return Ok(MandatePinOutcome::BookAbsent),
    };
    mandate_pin_locked(&mut book, node_id, &key, false, workspace)
}

/// 显式人工钉存（CLI/WSAPI 管理面）：覆盖现有值（人工动作优先于 TOFU 首见
/// 纪律——轮换的唯一通道）；簿缺席时**创建簿**（管理面显式写簿 = regime
/// 激活唯一路径，同 trust-add 先例）。
pub fn mandate_pin_forced(
    workspace: &Path,
    node_id: &str,
    pubkey: &str,
) -> Result<MandatePinOutcome, String> {
    let Some(key) = normalize_mandate_pubkey(pubkey) else {
        return Err(format!("mandate 公钥形态非法（期待 66/130 hex）：{pubkey}"));
    };
    let _serial = TOFU_PIN_LOCK.lock();
    let mut book = match TrustBook::load(workspace)? {
        Some(b) => b,
        // 管理面写簿激活（announce 路径没有这个臂——见 mandate_tofu_pin）。
        None => TrustBook::from_data(TrustBookData::default()),
    };
    mandate_pin_locked(&mut book, node_id, &key, true, workspace)
}

/// mandate pin 内核（调用方已持 TOFU_PIN_LOCK）。`forced=true` 覆盖异值。
///
/// 判定序：revoked → Distrusted（吊销态不更新，无论 forced——重新授权是
/// 显式 unrevoke 后再 pin）→ 同值 Unchanged → 异值 forced 覆盖 / 非 forced
/// Conflict 保留首见 → 无 pin 钉存。落盘失败 = Err 上抛（内存态随 book
/// drop 丢弃，无半提交状态泄漏到盘）。
fn mandate_pin_locked(
    book: &mut TrustBook,
    node_id: &str,
    key: &str,
    forced: bool,
    workspace: &Path,
) -> Result<MandatePinOutcome, String> {
    let id = node_id.trim().to_lowercase();
    if book
        .data()
        .mandate_revoked
        .iter()
        .any(|s| s.trim().to_lowercase() == id)
    {
        return Ok(MandatePinOutcome::Distrusted);
    }
    match book.data().mandate_keys.get(&id) {
        Some(existing) if normalize_mandate_pubkey(existing).as_deref() == Some(key) => {
            Ok(MandatePinOutcome::Unchanged)
        }
        Some(existing) if !forced => Ok(MandatePinOutcome::Conflict {
            existing: existing.clone(),
        }),
        _ => {
            book.data.mandate_keys.insert(id, key.to_string());
            book.save(workspace)?;
            Ok(MandatePinOutcome::Pinned)
        }
    }
}

/// mandate 钉存状态查询（b 验签消费点）。簿缺席 = Absent（regime
/// 未激活，verify 侧可降级）；**读失败 = BookUnreadable**（regime 已激活
/// 而簿坏，验签侧 fail-closed——不可与 Absent 合并，否则管理员激活的
/// regime 会被一次坏盘/手改无声降级为放行态）。
pub fn mandate_key_state(workspace: &Path, node_id: &str) -> MandateKeyVerdict {
    let book = match TrustBook::load(workspace) {
        Ok(Some(b)) => b,
        Ok(None) => return MandateKeyVerdict::Absent,
        Err(_) => return MandateKeyVerdict::BookUnreadable,
    };
    mandate_state_locked(&book, node_id)
}

/// 簿上 mandate 状态（内部/编辑面共用）。
fn mandate_state_locked(book: &TrustBook, node_id: &str) -> MandateKeyVerdict {
    let id = node_id.trim().to_lowercase();
    if book
        .data()
        .mandate_revoked
        .iter()
        .any(|s| s.trim().to_lowercase() == id)
    {
        return MandateKeyVerdict::Revoked;
    }
    match book.data().mandate_keys.get(&id) {
        Some(k) => MandateKeyVerdict::Pinned(k.clone()),
        None => MandateKeyVerdict::Absent,
    }
}

/// mandate 吊销（幂等；keys 保留留痕——吊销历史可审计， revoked 纪律）。
/// 簇簿缺席 = Err（吊销不激活簿——对一个不存在的信任域吊销是操作错误）。
pub fn mandate_revoke(workspace: &Path, node_id: &str) -> Result<(), String> {
    let _serial = TOFU_PIN_LOCK.lock();
    let mut book = match TrustBook::load(workspace)? {
        Some(b) => b,
        None => {
            return Err(
                "trust.toml 不存在（mandate 互信未启用）——吊销无对象，先 mandate-pin".to_string(),
            );
        }
    };
    let id = node_id.trim().to_lowercase();
    book.data
        .mandate_revoked
        .retain(|s| s.trim().to_lowercase() != id);
    book.data.mandate_revoked.push(id);
    book.save(workspace)
}

/// mandate 解除吊销（幂等；不动 keys——被吊销节点的 pin 保留，unrevoke 后
/// 验签即恢复；这是「吊销只加标记」语义的自然推论）。
pub fn mandate_unrevoke(workspace: &Path, node_id: &str) -> Result<(), String> {
    let _serial = TOFU_PIN_LOCK.lock();
    let mut book = match TrustBook::load(workspace)? {
        Some(b) => b,
        None => {
            return Err("trust.toml 不存在（mandate 互信未启用）——解除吊销无对象".to_string());
        }
    };
    let id = node_id.trim().to_lowercase();
    book.data
        .mandate_revoked
        .retain(|s| s.trim().to_lowercase() != id);
    book.save(workspace)
}

// ---------------------------------------------------------------------------
// announce 采信闸 + dial 吊销 veto。
//
// regime 激活模型：trust.toml 不存在 = 闸未启用（announce 全采信 / dial 无
// veto——零配置兼容现行发现链，字节级现状）；文件存在 = regime 激活（见下）。
// 激活唯一路径 = 管理面显式写簿（CLI trust-add/revoke / WSAPI 同源）——
// announce 面 TOFU 钉存只往已激活的簿追加，绝不因 announce 抵达而建文件
// （否则首条 announce 激活 allowlist 会让未钉存节点的 accept 全拒）。
// ---------------------------------------------------------------------------

/// TOFU 钉存进程内串行锁（check-then-pin 原子性；跨进程竞态=整簿替换最后
/// 写者胜，最坏 trusted 残留脏条目——裁决层 revoked 一票否决兜底无安全后果）。
static TOFU_PIN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// announce 采信闸裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnounceVerdict {
    /// 采信（含 TOFU 已钉存形态）——调用方照常走后续闸与入册。
    Adopt,
    /// 不采信（revoked veto / 读失败 fail-closed）——announce 整条丢弃。
    Veto(&'static str),
}

/// announce 采信闸（③ 互信才采信 announce 身份宣告）。
///
/// 判定序：簿缺席 → Adopt（regime 未激活，不建文件不钉存）；读失败 →
/// Veto（fail-closed，与插件 accept 闸同方向——激活态下吊销语义保守）；
/// revoked → Veto；trusted → Adopt；unknown → TOFU 钉存后 Adopt（锁内重读
/// 再验——锁间隙被并发吊销时让位 veto）。
///
/// 仅对携带合法 iroh hex 的 announce 调用（无 key 旧节点由调用方跳过本闸
/// =维持现行行为）。
pub fn announce_gate(workspace: &Path, iroh_hex: &str) -> AnnounceVerdict {
    let book = match TrustBook::load(workspace) {
        Ok(Some(b)) => b,
        Ok(None) => return AnnounceVerdict::Adopt,
        Err(e) => {
            tracing::warn!(
                iroh = %iroh_hex,
                error = %e,
                "[TrustBook] trust.toml 读取失败，announce fail-closed 不采信"
            );
            return AnnounceVerdict::Veto("trust_book_read_failed");
        }
    };
    let id = normalize_id(iroh_hex);
    if book.data().revoked.iter().any(|s| normalize_id(s) == id) {
        tracing::warn!(
            iroh = %iroh_hex,
            "[TrustBook] announce 来自已吊销节点——整条不采信（iroh:{id}）"
        );
        return AnnounceVerdict::Veto("revoked");
    }
    let trusted = book.data().trusted.iter().any(|s| normalize_id(s) == id);
    if trusted {
        return AnnounceVerdict::Adopt;
    }
    // unknown → TOFU 首见钉存。锁内重读再验（防锁间隙 revoked 竞态）。
    let _serial = TOFU_PIN_LOCK.lock();
    let mut fresh = match TrustBook::load(workspace) {
        Ok(Some(b)) => b,
        Ok(None) => return AnnounceVerdict::Adopt,
        Err(e) => {
            tracing::warn!(
                iroh = %iroh_hex,
                error = %e,
                "[TrustBook] trust.toml 重读失败，announce fail-closed 不采信"
            );
            return AnnounceVerdict::Veto("trust_book_read_failed");
        }
    };
    if fresh.data().revoked.iter().any(|s| normalize_id(s) == id) {
        tracing::warn!(
            iroh = %iroh_hex,
            "[TrustBook] announce 来自已吊销节点（TOFU 竞态窗口内被吊销）——不采信"
        );
        return AnnounceVerdict::Veto("revoked");
    }
    fresh.trust(&id);
    match fresh.save(workspace) {
        Ok(()) => {
            tracing::info!(
                iroh = %iroh_hex,
                "[TrustBook] TOFU 首见自动钉存: iroh:{id}"
            );
            AnnounceVerdict::Adopt
        }
        Err(e) => {
            // 钉存写盘失败仍 Adopt：拒绝合法 announce 因存储故障 = 可用性
            // 损失不换安全收益；下次 announce 再钉（自愈）。
            tracing::warn!(
                iroh = %iroh_hex,
                error = %e,
                "[TrustBook] TOFU 钉存写盘失败（本条采信，下次 announce 重试钉存）"
            );
            AnnounceVerdict::Adopt
        }
    }
}

/// dial 吊销 veto（② 吊销后拒连的出站臂）。只防 revoked：unknown 放行
/// （对端 accept 闸是守门人；出站 veto 的目标 = 不主动撞已被本节点吊销的
/// 节点）。簿缺席 = 未启用 false；**读失败 fail-open**（方向性刻意：announce/
/// accept 拒绝=收紧，dial 拒绝=本节点出站能力损失——坏簿不瘫痪出站漫游）。
pub fn dial_vetoed(workspace: &Path, iroh_hex: &str) -> bool {
    let book = match TrustBook::load(workspace) {
        Ok(Some(b)) => b,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(
                iroh = %iroh_hex,
                error = %e,
                "[TrustBook] trust.toml 读取失败，dial veto fail-open 放行"
            );
            return false;
        }
    };
    let id = normalize_id(iroh_hex);
    book.data().revoked.iter().any(|s| normalize_id(s) == id)
}

#[cfg(test)]
mod tests;
