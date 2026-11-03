//! L13-b（2026-11-03 改进项三期）：任务 mandate 签名授权——数据形态与
//! 密码学内核（纯函数层；wire 接线在 rpc/client（A 端签名注入）与
//! rpc/peer_chat_handler（B 端前置闸），执行面守卫在 nemesis-agent +
//! nemesisbot cluster_agent）。
//!
//! mandate = A 端（master）对单个出站 peer_chat 任务签发的**授权书**：
//! `payload["mandate"]` = `{v:1, task_id, tool_allowlist, time_window,
//! budget, signed_at, sig}`。B 端只信本地钉存钥（trust.toml
//! `[mandate_keys][source_node_id]`，L13-a 分发面）验签——**信封不含公钥
//! 槽**（防密钥替换攻击的结构性前提：攻击者改信封公钥无从影响验证）。
//!
//! 摘要语义单源声明：规范化字节 = `serde_json::to_vec(&MandateBody)`
//! （derive 结构体按声明序序列化），摘要 = SHA-256(canonical)，与
//! nemesis-verify `v4_content_digest` 对非 PE 内容（raw codec 全文件哈希）
//! **字节等价**——parity 测试钉住（规格「A 端 v4_content_digest+签名」语义
//! 单源不漂移）。直依赖 p256+sha2 而非 nemesis-verify（后者 reqwest 重），
//! 密钥形态与算法栈与 v4 leaf 同口径（拍板「复用 leaf」= 复用形态不是复用
//! 密钥文件——节点 mandate 密钥是每节点运行时身份，ceremony 私钥红线不进
//! workspace）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::trust::{MandateKeyVerdict, mandate_key_state};

// ---------------------------------------------------------------------------
// Wire 形态
// ---------------------------------------------------------------------------

/// mandate 时间窗（绝对 epoch secs；假设 NTP，与 announce 过期 G3 同一假设）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateTimeWindow {
    /// 生效下界（unix secs）。
    pub not_before: u64,
    /// 到期上界（unix secs）。A 端默认 = signed_at + window_secs。
    pub expires_at: u64,
}

/// mandate 预算（0 = 不限）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateBudget {
    /// token 上限（session ledger delta 口径）。
    pub max_tokens: u64,
    /// 工具调用次数上限。
    pub max_tool_calls: u64,
}

/// mandate 签名体（规范化字节的单一真相源）。**全部字段必填（无 serde
/// default）**：缺失字段 = Malformed 拒，不做缺省猜测——这保证 B 端反序列
/// 化后重新规范化与 A 端签发字节逐位一致（canonical roundtrip 的前提）。
/// 字段声明序即 wire 序（serde_json struct 保声明序），改序 = 破坏性变更。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateBody {
    /// 协议版本（当前恒 1；未知版本 = Malformed 拒）。
    pub v: u32,
    /// 绑定的任务 id（防移植：B 端与 payload.task_id 双重绑定校验）。
    pub task_id: String,
    /// 工具白名单：null = 不限；`[]` = 零授权；非空 = 精确名单（v1 无通配符）。
    pub tool_allowlist: Option<Vec<String>>,
    pub time_window: MandateTimeWindow,
    pub budget: MandateBudget,
    /// 签发时刻（unix secs）。
    pub signed_at: u64,
}

/// mandate 信封（body 内联 + sig 追尾）。`#[serde(flatten)]` 保证 wire 上
/// body 字段在顶层、`sig` 最后——A 端组装与 B 端解析共用同一形态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MandateEnvelope {
    #[serde(flatten)]
    pub body: MandateBody,
    /// ECDSA P-256 签名（64B r||s 的 128 hex 小写）。
    pub sig: String,
}

// ---------------------------------------------------------------------------
// 摘要与签名原语
// ---------------------------------------------------------------------------

/// 当前 unix 秒（chrono 假设 NTP，与 announce G3 同一假设）。
pub fn now_unix() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

/// mandate 规范化字节（**单一真相源**：签发与验证同走本函数）。
pub fn canonical_bytes(body: &MandateBody) -> Result<Vec<u8>, String> {
    serde_json::to_vec(body).map_err(|e| format!("mandate canonical serialize: {e}"))
}

/// mandate 摘要 = SHA-256(canonical)。与 nemesis-verify `v4_content_digest`
/// 对非 PE 内容字节等价（parity 测试钉住）。
pub fn mandate_digest(bytes: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(bytes).into()
}

/// 公钥 SEC1 uncompressed 编码（65B；与 nemesis-verify `public_key_bytes`
/// 同口径）。
pub fn public_key_bytes(vk: &VerifyingKey) -> [u8; 65] {
    let point = vk.as_affine().to_encoded_point(false);
    let mut out = [0u8; 65];
    out.copy_from_slice(point.as_bytes());
    out
}

/// P-256 签名 → 64B r||s（RFC 6979 确定式 nonce；与 v4 同口径）。
pub fn p256_sign(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
    use p256::ecdsa::signature::Signer;
    let sig: Signature = sk.sign(msg);
    sig.to_bytes().into()
}

/// P-256 验签（64B r||s；true = 有效）。
pub fn p256_verify(vk: &VerifyingKey, msg: &[u8], sig: &[u8; 64]) -> bool {
    use p256::ecdsa::signature::Verifier;
    match Signature::from_slice(sig) {
        Ok(s) => vk.verify(msg, &s).is_ok(),
        Err(_) => false,
    }
}

/// 从 hex 私钥（64 hex = 32B 标量）构造 SigningKey。
pub fn signing_key_from_hex(hex_str: &str) -> Result<SigningKey, String> {
    let bytes = hex::decode(hex_str.trim()).map_err(|e| format!("mandate 私钥 hex 非法: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("mandate 私钥长度 {}B ≠ 32B", v.len()))?;
    SigningKey::from_bytes(&arr.into()).map_err(|e| format!("mandate 私钥非法: {e}"))
}

/// 从 hex 公钥构造 VerifyingKey（收 130 hex uncompressed / 66 hex compressed）。
pub fn verifying_key_from_hex(hex_str: &str) -> Result<VerifyingKey, String> {
    let bytes = hex::decode(hex_str.trim()).map_err(|e| format!("mandate 公钥 hex 非法: {e}"))?;
    if bytes.len() != 65 && bytes.len() != 33 {
        return Err(format!(
            "mandate 公钥长度 {}B ≠ 65/33B（SEC1 uncompressed/compressed）",
            bytes.len()
        ));
    }
    VerifyingKey::from_sec1_bytes(&bytes).map_err(|e| format!("mandate 公钥非法: {e}"))
}

/// 公钥 SHA-256 指纹（hex 小写；`cluster status` 签名器状态行显示用）。
pub fn key_fp_hex(pubkey: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(pubkey))
}

// ---------------------------------------------------------------------------
// 签发（A 端）
// ---------------------------------------------------------------------------

/// 时钟容差：not_before 相对 signed_at 回拨量。B 端时钟落后 A 端时，
/// `not_before = signed_at` 会让合规 mandate 在 B 端 NotYetValid 假拒
/// （e2e 实测双机 epoch 差 4s）。与 announce 时钟漂移 G3 预警线（60s）
/// 同口径。
pub const CLOCK_SKEW_TOLERANCE_SECS: u64 = 60;

/// 从授权配置构建默认 mandate body（A 端派发统一授权模板；chat 派发与
/// board 派发共用，v1 不按任务粒度分化——设计文档 §1 明确不做）。
///
/// `tool_allowlist` 空数组 = 不限（None 语义）；显式列名 = 白名单。
pub fn build_default_mandate(
    task_id: &str,
    tool_allowlist: Option<Vec<String>>,
    window_secs: u64,
    max_tokens: u64,
    max_tool_calls: u64,
    signed_at: u64,
) -> MandateBody {
    MandateBody {
        v: 1,
        task_id: task_id.to_string(),
        tool_allowlist,
        time_window: MandateTimeWindow {
            not_before: signed_at.saturating_sub(CLOCK_SKEW_TOLERANCE_SECS),
            expires_at: signed_at.saturating_add(window_secs),
        },
        budget: MandateBudget {
            max_tokens,
            max_tool_calls,
        },
        signed_at,
    }
}

/// 签发 mandate 信封（body → canonical → digest → P-256 → hex）。
pub fn sign_mandate(body: &MandateBody, sk: &SigningKey) -> Result<MandateEnvelope, String> {
    let canonical = canonical_bytes(body)?;
    let digest = mandate_digest(&canonical);
    let sig = p256_sign(sk, &digest);
    Ok(MandateEnvelope {
        body: body.clone(),
        sig: hex::encode(sig),
    })
}

/// A 端签名器（gateway 装配期构造一次，注入 [`crate::rpc::client::RpcClient::
/// set_mandate_signer`]——dispatch 咽喉对每单出站 peer_chat 调 `sign_for_task`）。
/// 授权模板快照自 config `[mandate]`（重启生效，无热键——与 cluster 其余系统
/// 参数同口径）。
#[derive(Clone)]
pub struct MandateSigner {
    key: Arc<SigningKey>,
    tool_allowlist: Option<Vec<String>>,
    window_secs: u64,
    max_tokens: u64,
    max_tool_calls: u64,
    /// 公钥指纹（hex 小写；`cluster status` 签名器状态行显示）。
    pub key_fp: String,
    /// 公钥（130 hex SEC1 uncompressed；announce 槽位覆盖静态声明用）。
    pub public_key: String,
}

impl MandateSigner {
    /// 从密钥文件 + 授权模板构造。密钥文件已由
    /// [`load_or_generate_mandate_key`] 校验公私一致。
    pub fn from_key_file(
        kf: &MandateKeyFile,
        tool_allowlist: Option<Vec<String>>,
        window_secs: u64,
        max_tokens: u64,
        max_tool_calls: u64,
    ) -> Result<Self, String> {
        let key = Arc::new(signing_key_from_hex(&kf.private_key)?);
        Ok(Self {
            key,
            tool_allowlist,
            window_secs,
            max_tokens,
            max_tool_calls,
            key_fp: key_fp_hex(&hex::decode(&kf.public_key).map_err(|e| format!("公钥 hex: {e}"))?),
            public_key: kf.public_key.clone(),
        })
    }

    /// 为单个任务签发 mandate（wire Value 形态——直接进 `payload["mandate"]`）。
    pub fn sign_for_task(&self, task_id: &str) -> Result<serde_json::Value, String> {
        let body = build_default_mandate(
            task_id,
            self.tool_allowlist.clone(),
            self.window_secs,
            self.max_tokens,
            self.max_tool_calls,
            now_unix(),
        );
        let env = sign_mandate(&body, &self.key)?;
        serde_json::to_value(&env).map_err(|e| format!("mandate envelope 序列化失败: {e}"))
    }
}

// ---------------------------------------------------------------------------
// B 端前置闸
// ---------------------------------------------------------------------------

/// B 端 mandate 校验策略（gateway 装配期构造，注入 PeerChatHandler）。
#[derive(Debug, Clone)]
pub struct MandatePolicy {
    /// workspace（trust.toml 钉存钥面根）。
    pub workspace: PathBuf,
    /// posture（config `[mandate].mode` 解析产物；装配期钉死，重启生效）。
    pub mode: super::super::config_loader::MandateMode,
}

impl MandatePolicy {
    /// 前置闸裁决（PeerChatHandler::handle 消费；产出执行面守卫投影）。
    ///
    /// 判定序（设计文档 §5）：
    /// - mode=Off → 放行不验（回滚开关）；
    /// - mandate 缺失：Verify → 放行（旧行为逐字节保持）/ Require → 拒
    ///   （防剥离攻击）；policy 未装配 = Verify 同语义放行；
    /// - mandate 在场 → [`verify_mandate_full`] 全链（吊销 → 钉存 → 签名
    ///   → 绑定 → 时限），通过 = 投影 enforcement。
    ///
    /// 失败返回 [`MandateViolation`]（Display 即 ACK reason 文案）。
    pub fn check(
        &self,
        source_node_id: &str,
        payload: &serde_json::Value,
        task_id: &str,
        now: u64,
    ) -> Result<Option<nemesis_types::cluster::MandateEnforcement>, MandateViolation> {
        if self.mode == super::super::config_loader::MandateMode::Off {
            return Ok(None);
        }
        let mandate_value = payload.get("mandate");
        let Some(mandate_value) = mandate_value else {
            return if self.mode == super::super::config_loader::MandateMode::Require {
                Err(MandateViolation::Malformed(
                    "mandate 缺失（require 模式）".into(),
                ))
            } else {
                Ok(None)
            };
        };
        // mandate: null 与缺失同语义（serde null 不是对象，verify_mandate_full
        // 会 Malformed——这里显式归一为缺失臂，require 下拒、verify 下放行）。
        if mandate_value.is_null() {
            return if self.mode == super::super::config_loader::MandateMode::Require {
                Err(MandateViolation::Malformed(
                    "mandate 缺失（require 模式）".into(),
                ))
            } else {
                Ok(None)
            };
        }
        let body =
            verify_mandate_full(&self.workspace, source_node_id, mandate_value, task_id, now)?;
        Ok(Some(enforcement_from_body(&body)))
    }
}

// ---------------------------------------------------------------------------
// 验证（B 端）
// ---------------------------------------------------------------------------

/// mandate 校验失败分类（四道校验的结构化结果；Display 文案 = ACK error
/// reason 与审计留痕的单一真相源，前缀「mandate 拒绝:」由调用方拼）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MandateViolation {
    /// 形态非法（非对象/缺字段/版本未知/sig 非 128 hex/公钥解不出）。
    Malformed(String),
    /// 签发节点 mandate 钥已被吊销（trust.toml `[mandate_revoked]`，一票
    /// 否决先于验签）。
    Revoked,
    /// 本地无钉存钥（簿缺席/未钉）——TOFU 纪律：不做任务时自动钉，钉存
    /// 只走 announce 面。
    NoPinnedKey,
    /// 签名验不过（伪造/篡改/钉存钥与签发钥不一致）。
    BadSignature,
    /// mandate.task_id ≠ 载荷任务 id（移植攻击）。
    TaskIdMismatch,
    /// 时间窗未开始（B 端时钟落后于签发端超过 window 容差）。
    NotYetValid { not_before: u64, now: u64 },
    /// 已过期（重放过期授权）。
    Expired { expires_at: u64, now: u64 },
}

impl std::fmt::Display for MandateViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(d) => write!(f, "mandate 拒绝: 形态非法（{d}）"),
            Self::Revoked => write!(f, "mandate 拒绝: 签发节点密钥已吊销"),
            Self::NoPinnedKey => write!(
                f,
                "mandate 拒绝: 无钉存公钥（等待 announce 分发或人工 mandate-pin）"
            ),
            Self::BadSignature => write!(f, "mandate 拒绝: 签名验证失败"),
            Self::TaskIdMismatch => write!(f, "mandate 拒绝: task_id 绑定不匹配"),
            Self::NotYetValid { not_before, now } => {
                write!(
                    f,
                    "mandate 拒绝: 时间窗未开始（not_before={not_before} > now={now}）"
                )
            }
            Self::Expired { expires_at, now } => {
                write!(
                    f,
                    "mandate 拒绝: 已过期（expires_at={expires_at} < now={now}）"
                )
            }
        }
    }
}

/// 从 wire Value 解析信封（严格形态：缺字段/类型错/未知版本 = Malformed）。
pub fn parse_envelope(value: &serde_json::Value) -> Result<MandateEnvelope, MandateViolation> {
    if !value.is_object() {
        return Err(MandateViolation::Malformed("mandate 不是 JSON 对象".into()));
    }
    let env: MandateEnvelope = serde_json::from_value(value.clone())
        .map_err(|e| MandateViolation::Malformed(format!("字段形态: {e}")))?;
    if env.body.v != 1 {
        return Err(MandateViolation::Malformed(format!(
            "未知协议版本 v={}",
            env.body.v
        )));
    }
    Ok(env)
}

/// 核心验证链（钉存钥已由调用方查好传入；签名 → 绑定 → 时限三道）。
pub fn verify_mandate(
    env: &MandateEnvelope,
    expected_task_id: &str,
    now: u64,
    pinned_pubkey_hex: &str,
) -> Result<MandateBody, MandateViolation> {
    let sig_bytes = hex::decode(env.sig.trim())
        .ok()
        .and_then(|v| <[u8; 64]>::try_from(v).ok());
    let Some(sig) = sig_bytes else {
        return Err(MandateViolation::Malformed(
            "sig 非法（应 128 hex = 64B r||s）".into(),
        ));
    };
    let vk = verifying_key_from_hex(pinned_pubkey_hex).map_err(MandateViolation::Malformed)?;
    let canonical = canonical_bytes(&env.body).map_err(MandateViolation::Malformed)?;
    let digest = mandate_digest(&canonical);
    if !p256_verify(&vk, &digest, &sig) {
        return Err(MandateViolation::BadSignature);
    }
    if env.body.task_id != expected_task_id {
        return Err(MandateViolation::TaskIdMismatch);
    }
    let w = &env.body.time_window;
    if now < w.not_before {
        return Err(MandateViolation::NotYetValid {
            not_before: w.not_before,
            now,
        });
    }
    if now >= w.expires_at {
        return Err(MandateViolation::Expired {
            expires_at: w.expires_at,
            now,
        });
    }
    Ok(env.body.clone())
}

/// 全链验证（吊销查 → 钉存查 → 签名 → 绑定 → 时限）。B 端 PeerChatHandler
/// 前置闸的唯一入口；`source_node_id` 来自 RPC 帧 `_rpc.from`（传输层注入，
/// 伪造不了），钉存钥查按它走。
pub fn verify_mandate_full(
    workspace: &Path,
    source_node_id: &str,
    mandate_value: &serde_json::Value,
    expected_task_id: &str,
    now: u64,
) -> Result<MandateBody, MandateViolation> {
    let state = mandate_key_state(workspace, source_node_id);
    let pinned = match state {
        MandateKeyVerdict::Revoked => return Err(MandateViolation::Revoked),
        MandateKeyVerdict::Absent => return Err(MandateViolation::NoPinnedKey),
        MandateKeyVerdict::Pinned(k) => k,
    };
    let env = parse_envelope(mandate_value)?;
    verify_mandate(&env, expected_task_id, now, &pinned)
}

/// B 端验签通过后的执行面守卫投影（剥签名/时间戳，只留执行期约束）。
pub fn enforcement_from_body(body: &MandateBody) -> nemesis_types::cluster::MandateEnforcement {
    nemesis_types::cluster::MandateEnforcement {
        task_id: body.task_id.clone(),
        tool_allowlist: body.tool_allowlist.clone(),
        expires_at: body.time_window.expires_at,
        max_tokens: body.budget.max_tokens,
        max_tool_calls: body.budget.max_tool_calls,
    }
}

// ---------------------------------------------------------------------------
// 节点签名密钥（load-or-generate）
// ---------------------------------------------------------------------------

/// 节点 mandate 密钥文件（`<workspace>/cluster/mandate_key.json`）。每节点
/// 运行时身份——拍板「复用 leaf」= 复用 v4 密钥形态（P-256 / SEC1
/// uncompressed 130 hex / 64B r||s / RFC 6979），不是复用 ceremony 签名
/// 密钥文件（私钥红线：绝不进 workspace/仓库）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MandateKeyFile {
    /// hex P-256 私钥标量（64 hex = 32B）。
    pub private_key: String,
    /// hex 公钥 SEC1 uncompressed（130 hex = 65B）。
    pub public_key: String,
    /// 生成时刻（rfc3339）。
    pub created_at: String,
}

/// 节点 mandate 密钥文件路径。
pub fn mandate_key_path(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("mandate_key.json")
}

/// 惰性 load-or-generate：文件在场且形态一致（公钥=私钥派生）→ 复用；
/// 缺失 → 生成 + 原子落盘（0600）；损坏/公私不一致 → **Err 拒绝**（诚实
/// 失败——签名器构造方禁用签名 + warn，绝不静默换钥：换钥 = 对端 TOFU
/// Conflict 风暴，宁可不签）。
pub fn load_or_generate_mandate_key(workspace: &Path) -> Result<MandateKeyFile, String> {
    let path = mandate_key_path(workspace);
    if path.exists() {
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("读 {}: {e}", path.display()))?;
        let kf: MandateKeyFile =
            serde_json::from_str(&content).map_err(|e| format!("解析 {}: {e}", path.display()))?;
        // 形态自洽校验：公钥 = 私钥派生（防手改半截——只改私钥的文件会在
        // 验签面产生无声的钉存钥错配，这里前置拦）。
        let sk = signing_key_from_hex(&kf.private_key)?;
        let derived = hex::encode(public_key_bytes(sk.verifying_key()));
        if derived != kf.public_key.trim().to_lowercase() {
            return Err(format!(
                "{} 公钥与私钥派生不一致（手改残留？）——拒绝使用，删除该文件将重新生成新密钥（对端需重新钉存）",
                path.display()
            ));
        }
        return Ok(kf);
    }
    let sk = SigningKey::random(&mut OsRng);
    let kf = MandateKeyFile {
        private_key: hex::encode(sk.to_bytes().as_slice()),
        public_key: hex::encode(public_key_bytes(sk.verifying_key())),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&kf)
        .map_err(|e| format!("mandate_key.json 序列化失败: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), json.as_bytes(), 0o600)
        .map_err(|e| format!("写 {}: {e}", path.display()))?;
    Ok(kf)
}

#[cfg(test)]
mod mandate_tests;
