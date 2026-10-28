//! Credential Vault —— 凭据加密存储（P0 安全升级，2026-09-22）。
//! 计划：`docs/PLAN/2026-09-22_credential-vault-and-risk-limits.md`
//!
//! 设计要点（与计划文档对齐）：
//! - **单一真相源**：秘密以 AES-256-GCM 逐条加密落盘（路径由调用方给定，约定
//!   `<workspace>/config/vault.enc`）。本模块不认识"模型 key / 通道 token"等
//!   业务概念——任何秘密都是一条别名条目，业务知识全部住在调用方与配置里。
//! - **主密钥双模式**：一个随机 DEK（数据加密密钥，32B）被平台 KEK 包裹：
//!   - `Dpapi`（Windows）：`CryptProtectData` per-user 包裹——零摩擦，本机本用户
//!     免口令即开；文件离开该用户/机器即不可解。
//!   - `Argon2id`：口令派生 KEK 包裹——Linux/headless 与跨机可移植场景；
//!     OWASP 默认参数（m=19MiB, t=2, p=1）。换口令 = 只重包 DEK，条目密文不动。
//! - **`list` 永不见值**：元数据（domain/描述/时间）明文可读，未解锁也能列；
//!   值只有 [`VaultStore::get`] 一条出路。
//! - **诚实错误**：口令错误 / 文件损坏 / 别名不存在 / 未解锁，一一可区分。
//! - 条目密文以**别名字节作 AAD** 绑定，防条目置换；DEK 包裹以固定域串作 AAD。
//! - 跨平台核心与平台绑定分层：DPAPI 绑定仅本文件 `dpapi` 段一处（`#[cfg(windows)]`）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

/// AES-GCM 标准nonce 长度。
const NONCE_LEN: usize = 12;
/// DEK 长度（AES-256）。
const DEK_LEN: usize = 32;
/// 当前文件格式版本（不兼容变更时递增）。
const FORMAT_VERSION: u32 = 1;
/// DEK 包裹的 AAD 域分隔串。
const DEK_AAD: &[u8] = b"nemesisbot-vault-dek-v1";

/// Vault 错误。所有变体都可对用户诚实展示。
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault 文件不存在: {0}")]
    NotFound(String),
    #[error("vault 文件已存在: {0}")]
    FileExists(String),
    #[error("非法别名: {0:?}（禁止空串、首尾空白、控制字符）")]
    InvalidAlias(String),
    #[error("别名不存在: {0}")]
    UnknownAlias(String),
    #[error("口令错误（DEK 解包认证失败）")]
    WrongPassphrase,
    #[error("vault 已锁定：argon2 模式需先 unlock")]
    Locked,
    #[error("vault 文件损坏: {0}")]
    Corrupted(String),
    #[error("平台层错误: {0}")]
    Platform(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("加密层错误: {0}")]
    Crypto(String),
}

/// 主密钥包裹模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultMode {
    /// Windows DPAPI（per-user，零摩擦）。
    Dpapi,
    /// Argon2id 口令派生（跨平台/可移植）。
    Argon2id,
}

impl std::fmt::Display for VaultMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 与 serde rename_all = "lowercase" 落盘拼写一致。
        f.write_str(match self {
            VaultMode::Dpapi => "dpapi",
            VaultMode::Argon2id => "argon2id",
        })
    }
}

/// Argon2id 派生参数（默认 = OWASP 推荐：m=19MiB, t=2, p=1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Argon2Params {
    /// 内存成本，KiB。
    pub m_cost_kib: u32,
    /// 迭代轮数。
    pub t_cost: u32,
    /// 并行度。
    pub parallelism: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            m_cost_kib: 19456,
            t_cost: 2,
            parallelism: 1,
        }
    }
}

/// 条目元数据（`list` 可见，永不含值）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntryMeta {
    pub domain: String,
    pub description: String,
    /// RFC3339。
    pub created_at: String,
    /// 覆盖写入（轮换）时间；从未覆盖则 None。
    pub rotated_at: Option<String>,
}

/// `list` 条目：别名 + 元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultListing {
    pub alias: String,
    #[serde(flatten)]
    pub meta: VaultEntryMeta,
}

/// 单条目落盘结构（密文 + 明文元数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultEntry {
    domain: String,
    description: String,
    created_at: String,
    rotated_at: Option<String>,
    /// base64(12B nonce)。
    nonce: String,
    /// base64(AES-256-GCM(secret, DEK, nonce, AAD=别名))。
    ciphertext: String,
}

/// 文件格式（`vault.enc` 的 JSON 结构）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    mode: VaultMode,
    /// dpapi 模式：base64(CryptProtectData(DEK))。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dpapi_blob: Option<String>,
    /// argon2 模式：base64(16B 盐)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrap_salt: Option<String>,
    /// argon2 模式：base64(12B 包裹 nonce)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrap_nonce: Option<String>,
    /// argon2 模式：派生参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    argon2: Option<Argon2Params>,
    /// argon2 模式：base64(AES-GCM(DEK, KEK))。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrapped_dek: Option<String>,
    entries: BTreeMap<String, VaultEntry>,
}

/// Vault 存储句柄：解析后的文件 + 进程内 DEK 缓存。
///
/// dpapi 模式 open 即解锁；argon2 模式 open 后保持锁定，须 [`VaultStore::unlock`]。
pub struct VaultStore {
    path: PathBuf,
    file: VaultFile,
    dek: Option<Vec<u8>>,
}

/// [`VaultStore`] 的 Debug：**永不输出 DEK 与条目**（derive 会把密钥打进日志）。
impl std::fmt::Debug for VaultStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultStore")
            .field("path", &self.path)
            .field("mode", &self.file.mode)
            .field("unlocked", &self.dek.is_some())
            .finish_non_exhaustive()
    }
}

impl VaultStore {
    /// 平台默认模式：Windows = DPAPI；其余 = Argon2id。
    pub fn default_mode() -> VaultMode {
        #[cfg(windows)]
        {
            VaultMode::Dpapi
        }
        #[cfg(not(windows))]
        {
            VaultMode::Argon2id
        }
    }

    /// 新建空 vault 文件（已存在则报 [`VaultError::FileExists`]）。
    ///
    /// `passphrase`：argon2 模式必填；dpapi 模式忽略。
    pub fn create(
        path: &Path,
        mode: VaultMode,
        passphrase: Option<&str>,
    ) -> Result<Self, VaultError> {
        if path.exists() {
            return Err(VaultError::FileExists(path.display().to_string()));
        }
        let mut dek = vec![0u8; DEK_LEN];
        OsRng.fill_bytes(&mut dek);
        let mut file = VaultFile {
            version: FORMAT_VERSION,
            mode,
            dpapi_blob: None,
            wrap_salt: None,
            wrap_nonce: None,
            argon2: None,
            wrapped_dek: None,
            entries: BTreeMap::new(),
        };
        seal_dek_into(&mut file, mode, &dek, passphrase)?;
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            fs::create_dir_all(dir)?;
        }
        let store = Self {
            path: path.to_path_buf(),
            file,
            dek: Some(dek),
        };
        store.save()?;
        Ok(store)
    }

    /// 打开已有 vault。dpapi 模式自动解包 DEK；argon2 模式保持锁定。
    pub fn open(path: &Path) -> Result<Self, VaultError> {
        let raw = fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => VaultError::NotFound(path.display().to_string()),
            _ => VaultError::Io(e),
        })?;
        let file: VaultFile = serde_json::from_slice(&raw)
            .map_err(|e| VaultError::Corrupted(format!("JSON 解析失败: {e}")))?;
        if file.version != FORMAT_VERSION {
            return Err(VaultError::Corrupted(format!(
                "不支持的格式版本 {}（当前支持 {FORMAT_VERSION}）",
                file.version
            )));
        }
        let dek = match file.mode {
            // dpapi：同用户同机即解锁；失败（跨用户/跨机/损坏）诚实报错。
            VaultMode::Dpapi => Some(unwrap_dek(&file, None)?),
            VaultMode::Argon2id => None,
        };
        Ok(Self {
            path: path.to_path_buf(),
            file,
            dek,
        })
    }

    /// 文件模式。
    pub fn mode(&self) -> VaultMode {
        self.file.mode
    }

    /// DEK 是否已解锁（dpapi 模式恒 true；argon2 模式 unlock 后 true）。
    pub fn is_unlocked(&self) -> bool {
        self.dek.is_some()
    }

    /// 解锁（仅 argon2 模式有意义；dpapi 模式已解锁，幂等成功）。
    pub fn unlock(&mut self, passphrase: &str) -> Result<(), VaultError> {
        if self.dek.is_some() {
            return Ok(());
        }
        self.dek = Some(unwrap_dek(&self.file, Some(passphrase))?);
        Ok(())
    }

    /// 写入（或覆盖）别名。覆盖时保留 `created_at`、更新 `rotated_at`。
    pub fn set(
        &mut self,
        alias: &str,
        secret: &str,
        domain: &str,
        description: &str,
    ) -> Result<(), VaultError> {
        validate_alias(alias)?;
        let dek = self.dek()?;
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let ct = aead_seal(dek, &nonce, secret.as_bytes(), alias.as_bytes())?;
        let now = chrono::Local::now().to_rfc3339();
        let rotated = self.file.entries.contains_key(alias);
        let created_at = if rotated {
            self.file.entries[alias].created_at.clone()
        } else {
            now.clone()
        };
        self.file.entries.insert(
            alias.to_string(),
            VaultEntry {
                domain: domain.to_string(),
                description: description.to_string(),
                created_at,
                rotated_at: rotated.then_some(now),
                nonce: B64.encode(nonce),
                ciphertext: B64.encode(ct),
            },
        );
        Ok(())
    }

    /// 读取别名明文。未解锁 → [`VaultError::Locked`]。
    pub fn get(&self, alias: &str) -> Result<String, VaultError> {
        let dek = self.dek()?;
        let entry = self
            .file
            .entries
            .get(alias)
            .ok_or_else(|| VaultError::UnknownAlias(alias.to_string()))?;
        let nonce = decode_nonce(&entry.nonce)?;
        let ct = B64
            .decode(&entry.ciphertext)
            .map_err(|e| VaultError::Corrupted(format!("密文 base64 解码失败: {e}")))?;
        let pt = aead_open(dek, &nonce, &ct, alias.as_bytes()).map_err(|_| {
            VaultError::Corrupted(format!("条目 `{alias}` 解密失败（密文或 AAD 不匹配）"))
        })?;
        String::from_utf8(pt).map_err(|_| VaultError::Corrupted("明文非 UTF-8".into()))
    }

    /// 删除别名，返回是否删除了已存在的条目。
    pub fn remove(&mut self, alias: &str) -> Result<bool, VaultError> {
        Ok(self.file.entries.remove(alias).is_some())
    }

    /// 列出别名与元数据。**永不含值**；未解锁也可用。
    pub fn list(&self) -> Vec<VaultListing> {
        self.file
            .entries
            .iter()
            .map(|(alias, e)| VaultListing {
                alias: alias.clone(),
                meta: VaultEntryMeta {
                    domain: e.domain.clone(),
                    description: e.description.clone(),
                    created_at: e.created_at.clone(),
                    rotated_at: e.rotated_at.clone(),
                },
            })
            .collect()
    }

    /// 全部别名。
    pub fn aliases(&self) -> Vec<String> {
        self.file.entries.keys().cloned().collect()
    }

    /// 原子写盘 — REL-002（2026-09-23）起委托统一 helper
    /// `nemesis_utils::write_file_atomic`（唯一临时名 + sync_all + 失败清理；
    /// 0600 创建即挂，取代本处自制 `.tmp` rename）。
    pub fn save(&self) -> Result<(), VaultError> {
        let json = serde_json::to_vec_pretty(&self.file)
            .map_err(|e| VaultError::Crypto(format!("序列化失败: {e}")))?;
        nemesis_utils::write_file_atomic(&self.path.to_string_lossy(), &json, 0o600)
            .map_err(|e| VaultError::Io(std::io::Error::other(e)))?;
        Ok(())
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 导出若干别名为可迁移密文 blob（P6-R2 agent 迁移）。
    ///
    /// 语义：用**随机一次性导出 DEK** 重封所选条目（AES-256-GCM，AAD 仍是
    /// 别名——与 [`VaultEntry`] 常规落盘形态完全同构），导出 DEK 经
    /// Argon2id + `passphrase` 包裹进 blob 头。`passphrase` 由调用方随机
    /// 生成、只随迁移 commit RPC 传递，**绝不落盘**——blob 全程密文。
    /// 元数据（domain/description/created_at/rotated_at）原样保留。
    ///
    /// 错误：未解锁 → [`VaultError::Locked`]；任一别名不在场 →
    /// [`VaultError::UnknownAlias`]（loud，调用方先校验清单）。
    pub fn export_entries(
        &self,
        aliases: &[String],
        passphrase: &str,
    ) -> Result<Vec<u8>, VaultError> {
        let src_dek = self.dek()?;
        // 先校验全部别名在场，再动密码学（fail fast，不做半份导出）。
        for a in aliases {
            if !self.file.entries.contains_key(a) {
                return Err(VaultError::UnknownAlias(a.clone()));
            }
        }
        let mut export_dek = vec![0u8; DEK_LEN];
        OsRng.fill_bytes(&mut export_dek);
        let mut file = VaultFile {
            version: FORMAT_VERSION,
            mode: VaultMode::Argon2id,
            dpapi_blob: None,
            wrap_salt: None,
            wrap_nonce: None,
            argon2: None,
            wrapped_dek: None,
            entries: BTreeMap::new(),
        };
        seal_dek_into(
            &mut file,
            VaultMode::Argon2id,
            &export_dek,
            Some(passphrase),
        )?;
        for a in aliases {
            let e = &self.file.entries[a];
            let nonce = decode_nonce(&e.nonce)?;
            let ct = B64
                .decode(&e.ciphertext)
                .map_err(|err| VaultError::Corrupted(format!("密文 base64 解码失败: {err}")))?;
            let pt = aead_open(src_dek, &nonce, &ct, a.as_bytes())?;
            let mut out_nonce = [0u8; NONCE_LEN];
            OsRng.fill_bytes(&mut out_nonce);
            let out_ct = aead_seal(&export_dek, &out_nonce, &pt, a.as_bytes())?;
            file.entries.insert(
                a.clone(),
                VaultEntry {
                    domain: e.domain.clone(),
                    description: e.description.clone(),
                    created_at: e.created_at.clone(),
                    rotated_at: e.rotated_at.clone(),
                    nonce: B64.encode(out_nonce),
                    ciphertext: B64.encode(out_ct),
                },
            );
        }
        serde_json::to_vec(&file).map_err(|e| VaultError::Crypto(format!("序列化失败: {e}")))
    }

    /// 从 [`VaultStore::export_entries`] 产物导入条目（P6-R2 agent 迁移）。
    ///
    /// 语义：用 `passphrase` 还原导出 DEK，把每条目用**本 vault 的 DEK**
    /// 重封落进本 vault（已存在的别名按 `set` 轮换语义覆盖，元数据保留
    /// blob 里的原始 created_at/rotated_at）。调用方自行 `save()` 持久化
    /// （与 `set`/`remove` 同约定）。返回导入的别名清单。
    ///
    /// 错误：blob 解析失败/版本不符 → [`VaultError::Corrupted`]；
    /// passphrase 错 →
    /// [`VaultError::WrongPassphrase`]；本 vault 未解锁 →
    /// [`VaultError::Locked`]。
    pub fn import_entries(
        &mut self,
        blob: &[u8],
        passphrase: &str,
    ) -> Result<Vec<String>, VaultError> {
        let parsed: VaultFile = serde_json::from_slice(blob)
            .map_err(|e| VaultError::Corrupted(format!("迁移凭据 blob 解析失败: {e}")))?;
        if parsed.version != FORMAT_VERSION {
            return Err(VaultError::Corrupted(format!(
                "迁移凭据 blob 版本 {} 不受支持（当前支持 {FORMAT_VERSION}）",
                parsed.version
            )));
        }
        // 导出产物恒为 Argon2id（export_entries 构造约束）；dpapi 头意味着
        // 篡改或异源 blob，loud 拒绝。
        if parsed.mode != VaultMode::Argon2id {
            return Err(VaultError::Corrupted(
                "迁移凭据 blob 模式非法（应为 argon2id）".into(),
            ));
        }
        let export_dek = unwrap_dek(&parsed, Some(passphrase))?;
        // dek() 借用 self；循环里要 mutate entries，先拿 owned。
        let dst_dek = self.dek()?.to_vec();
        let mut imported = Vec::with_capacity(parsed.entries.len());
        for (alias, e) in &parsed.entries {
            validate_alias(alias)?;
            let nonce = decode_nonce(&e.nonce)?;
            let ct = B64
                .decode(&e.ciphertext)
                .map_err(|err| VaultError::Corrupted(format!("密文 base64 解码失败: {err}")))?;
            let pt = aead_open(&export_dek, &nonce, &ct, alias.as_bytes())?;
            let mut out_nonce = [0u8; NONCE_LEN];
            OsRng.fill_bytes(&mut out_nonce);
            let out_ct = aead_seal(&dst_dek, &out_nonce, &pt, alias.as_bytes())?;
            self.file.entries.insert(
                alias.clone(),
                VaultEntry {
                    domain: e.domain.clone(),
                    description: e.description.clone(),
                    created_at: e.created_at.clone(),
                    rotated_at: e.rotated_at.clone(),
                    nonce: B64.encode(out_nonce),
                    ciphertext: B64.encode(out_ct),
                },
            );
            imported.push(alias.clone());
        }
        Ok(imported)
    }

    fn dek(&self) -> Result<&[u8], VaultError> {
        self.dek.as_deref().ok_or(VaultError::Locked)
    }
}

fn validate_alias(alias: &str) -> Result<(), VaultError> {
    if alias.is_empty() || alias.trim() != alias || alias.chars().any(char::is_control) {
        return Err(VaultError::InvalidAlias(alias.to_string()));
    }
    Ok(())
}

fn aead_seal(
    key: &[u8],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| VaultError::Crypto(format!("AES-GCM 加密失败: {e}")))
}

fn aead_open(
    key: &[u8],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|e| VaultError::Crypto(format!("AES-GCM 解密失败: {e}")))
}

fn decode_nonce(s: &str) -> Result<[u8; NONCE_LEN], VaultError> {
    let v = B64
        .decode(s)
        .map_err(|e| VaultError::Corrupted(format!("nonce base64 解码失败: {e}")))?;
    let len = v.len();
    v.try_into()
        .map_err(|_| VaultError::Corrupted(format!("nonce 长度非法: {len}B")))
}

/// 生成 DEK 并按模式包裹，把包裹字段写进 `file`。
fn seal_dek_into(
    file: &mut VaultFile,
    mode: VaultMode,
    dek: &[u8],
    passphrase: Option<&str>,
) -> Result<(), VaultError> {
    match mode {
        VaultMode::Dpapi => {
            #[cfg(windows)]
            {
                file.dpapi_blob = Some(B64.encode(dpapi::protect(dek)?));
                Ok(())
            }
            #[cfg(not(windows))]
            {
                let _ = (dek, passphrase);
                Err(VaultError::Platform("DPAPI 仅在 Windows 可用".into()))
            }
        }
        VaultMode::Argon2id => {
            let pw = passphrase.ok_or_else(|| VaultError::Crypto("argon2 模式需要口令".into()))?;
            let params = Argon2Params::default();
            let mut salt = [0u8; 16];
            OsRng.fill_bytes(&mut salt);
            let kek = derive_kek(pw.as_bytes(), &salt, &params)?;
            let mut nonce = [0u8; NONCE_LEN];
            OsRng.fill_bytes(&mut nonce);
            let wrapped = aead_seal(&kek, &nonce, dek, DEK_AAD)?;
            file.argon2 = Some(params);
            file.wrap_salt = Some(B64.encode(salt));
            file.wrap_nonce = Some(B64.encode(nonce));
            file.wrapped_dek = Some(B64.encode(wrapped));
            Ok(())
        }
    }
}

/// 从文件包裹字段还原 DEK。
fn unwrap_dek(file: &VaultFile, passphrase: Option<&str>) -> Result<Vec<u8>, VaultError> {
    match file.mode {
        VaultMode::Dpapi => {
            #[cfg(windows)]
            {
                let blob = file
                    .dpapi_blob
                    .as_deref()
                    .ok_or_else(|| VaultError::Corrupted("dpapi 模式缺 dpapi_blob".into()))?;
                let blob = B64.decode(blob).map_err(|e| {
                    VaultError::Corrupted(format!("dpapi_blob base64 解码失败: {e}"))
                })?;
                dpapi::unprotect(&blob)
            }
            #[cfg(not(windows))]
            {
                let _ = passphrase;
                Err(VaultError::Platform(
                    "DPAPI vault 只能在 Windows 上（同用户）解锁".into(),
                ))
            }
        }
        VaultMode::Argon2id => {
            let pw = passphrase.ok_or(VaultError::Locked)?;
            let salt = file
                .wrap_salt
                .as_deref()
                .ok_or_else(|| VaultError::Corrupted("argon2 模式缺 wrap_salt".into()))?;
            let nonce_s = file
                .wrap_nonce
                .as_deref()
                .ok_or_else(|| VaultError::Corrupted("argon2 模式缺 wrap_nonce".into()))?;
            let wrapped = file
                .wrapped_dek
                .as_deref()
                .ok_or_else(|| VaultError::Corrupted("argon2 模式缺 wrapped_dek".into()))?;
            let salt: [u8; 16] = B64
                .decode(salt)
                .map_err(|e| VaultError::Corrupted(format!("wrap_salt base64 解码失败: {e}")))?
                .try_into()
                .map_err(|_| VaultError::Corrupted("wrap_salt 长度非法".into()))?;
            let nonce = decode_nonce(nonce_s)?;
            let wrapped = B64
                .decode(wrapped)
                .map_err(|e| VaultError::Corrupted(format!("wrapped_dek base64 解码失败: {e}")))?;
            let params = file.argon2.clone().unwrap_or_default();
            let kek = derive_kek(pw.as_bytes(), &salt, &params)?;
            aead_open(&kek, &nonce, &wrapped, DEK_AAD).map_err(|_| VaultError::WrongPassphrase)
        }
    }
}

/// Argon2id 派生 32B KEK。
fn derive_kek(
    passphrase: &[u8],
    salt: &[u8; 16],
    params: &Argon2Params,
) -> Result<[u8; 32], VaultError> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let p = Params::new(
        params.m_cost_kib,
        params.t_cost,
        params.parallelism,
        Some(32),
    )
    .map_err(|e| VaultError::Crypto(format!("argon2 参数非法: {e}")))?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
    let mut kek = [0u8; 32];
    a2.hash_password_into(passphrase, salt, &mut kek)
        .map_err(|e| VaultError::Crypto(format!("argon2 派生失败: {e}")))?;
    Ok(kek)
}

/// Windows DPAPI 绑定（平台绑定全项目仅此一处）。
#[cfg(windows)]
mod dpapi {
    use super::VaultError;
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    /// CryptProtectData（per-user）。`CRYPTPROTECT_UI_FORBIDDEN`：服务/无人值守形态不弹窗。
    pub fn protect(data: &[u8]) -> Result<Vec<u8>, VaultError> {
        unsafe {
            let input = blob_of(data);
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            let ok = CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );
            if ok == 0 {
                return Err(VaultError::Platform("CryptProtectData 失败".into()));
            }
            let out = take_blob(output)?;
            Ok(out)
        }
    }

    /// CryptUnprotectData。失败（跨用户/跨机/损坏）诚实报错。
    pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, VaultError> {
        unsafe {
            let input = blob_of(blob);
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            let mut descr: windows_sys::core::PWSTR = std::ptr::null_mut();
            let ok = CryptUnprotectData(
                &input,
                &mut descr,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );
            if ok == 0 {
                return Err(VaultError::Platform(
                    "CryptUnprotectData 失败（vault 是否来自其他 Windows 用户/机器？）".into(),
                ));
            }
            if !descr.is_null() {
                LocalFree(descr as HLOCAL);
            }
            take_blob(output)
        }
    }

    fn blob_of(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// 从 API 输出 blob 拷贝数据并释放系统分配的内存。
    ///
    /// SAFETY（由调用方保证）：`blob` 来自刚成功的 DPAPI 输出——`pbData` 指向
    /// `cbData` 字节的系统分配内存且尚未释放。
    unsafe fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Result<Vec<u8>, VaultError> {
        let out = unsafe {
            if blob.pbData.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec()
            }
        };
        if !blob.pbData.is_null() {
            unsafe {
                LocalFree(blob.pbData as HLOCAL);
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 模型 key 迁移核心（raw Value，CLI `vault migrate` 与 WSAPI
// `models.migrate_keys` 共用的单一真相源；自 nemesisbot/commands/vault.rs 下沉）
// ---------------------------------------------------------------------------

/// `migrate_model_keys_to_vault` 的报告（CLI 输出与 WSAPI 投影共用）。
#[derive(Debug, Default, Clone)]
pub struct VaultMigrateReport {
    /// (模型显示名, 别名, domain)——domain 恒 "model"。
    pub migrated: Vec<(String, String, String)>,
    /// 同值别名复用次数（vault 内或同名后缀候选）。
    pub reused: usize,
    /// (想要的别名, 实际用的别名)——同名异值加 `__N` 后缀。
    pub conflicts: Vec<(String, String)>,
    /// (模型显示名, 断引别名)——`yaml:` 引用在 credentials.yaml 查无此条；
    /// 配置不动，fail loud 报给调用方（不静默丢引用）。
    pub yaml_broken: Vec<(String, String)>,
    /// 迁移后从 credentials.yaml 移除的别名（明文彻底离盘）。
    pub yaml_aliases_removed: Vec<String>,
    /// `env:` 引用跳过数（非明文落盘，保持原样）。
    pub skipped_env: usize,
    /// 已是 `vault:` 引用的条目数（幂等重跑面）。
    pub already_vault: usize,
    /// credentials.yaml 有条目被移除（调用方需要回写该文件的信号）。
    pub creds_changed: bool,
}

impl VaultMigrateReport {
    /// config 是否需要回写：任何 migrated 都改写了 `api_key` 字段
    ///（yaml_broken/already_vault/skipped 臂全部不动配置）。
    pub fn config_changed(&self) -> bool {
        !self.migrated.is_empty()
    }

    /// 无迁移也无断引（完全幂等重跑）。
    pub fn is_noop(&self) -> bool {
        self.migrated.is_empty() && self.yaml_broken.is_empty() && self.reused == 0
    }
}

/// 模型 key 迁移核心（raw Value，纯内存不落盘）：把 config Value 里
/// model_list 的明文 `api_key`（含 `yaml:` 引用升格）改写成 `vault:<alias>`，
/// 秘密经 `store.set` 进 vault 内存态。三个来源一次处理：
///
/// 1. 内联明文 key → vault（别名 = sanitize(model_name)）
/// 2. `yaml:<alias>` 引用 → 值从 `creds` 搬进 vault（别名不变），配置回写
///    `vault:<alias>`，creds 对应条目移除（明文彻底离盘）
/// 3. `env:` 保持原样；`vault:` 已迁移跳过；断引用 → `yaml_broken` 且不动配置
///
/// 冲突策略与 `credentials::migrate_inline_keys_to_yaml` 一致：已有别名同值
/// → 复用；异值 → `__N` 后缀，绝不覆盖。`store.set` 失败（非法别名等）=
/// **Err 中断**——调用方据此放弃落盘（WSAPI 事务闭包 Err = config 不回写；
/// CLI 壳 = 整体报错），绝不带着半迁移态静默继续。
///
/// **纯内存**：本函数不落任何盘——持久化与顺序不变量由调用方保证
/// （BUG-20261028-119 后的完整口径）：
/// ① **vault 落盘先于 config 回写**——先剥离明文再落 vault 的中间态 =
/// 配置指 `vault:<alias>` 但 vault 没有该别名的断链盘面；
/// ② **creds 改写后于 config 提交**——config 提交是事务点，事务点之前
/// 销毁 yaml 别名会把提交失败变成永久断链（config 仍指 `yaml:<alias>`
/// 但别名已删）；WSAPI 形态 = 闭包内只 `store.save()`，creds 回写在
/// `update_raw_config` 返回 Ok 之后，后置失败以 warning 诚实上报。
/// CLI 壳同序。
pub fn migrate_model_keys_to_vault(
    config: &mut serde_json::Value,
    store: &mut VaultStore,
    mut creds: Option<&mut nemesis_config::credentials::CredentialsFile>,
) -> Result<VaultMigrateReport, VaultError> {
    let mut report = VaultMigrateReport::default();
    let Some(entries) = config.get_mut("model_list").and_then(|v| v.as_array_mut()) else {
        return Ok(report);
    };
    for mc in entries.iter_mut() {
        let Some(obj) = mc.as_object_mut() else {
            continue;
        };
        let display_name = {
            let name = obj
                .get("model_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if name.is_empty() {
                obj.get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
            } else {
                name
            }
        }
        .to_string();
        let api_key = obj
            .get("api_key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if api_key.is_empty() {
            continue;
        }
        if api_key.starts_with(nemesis_config::vault_ref::VAULT_PREFIX) {
            // 残留清扫（BUG-20261028-119）：config 已指 vault 但 creds 仍留
            // 同名明文别名 = 上次迁移 config 已提交、creds 回写失败的现场
            // ——顺手清掉，让重试真正收敛（明文彻底离盘）。
            if let (Some(alias), Some(creds_mut)) = (
                api_key.strip_prefix(nemesis_config::vault_ref::VAULT_PREFIX),
                creds.as_mut(),
            ) && creds_mut.keys.remove(alias).is_some()
            {
                report.creds_changed = true;
                report.yaml_aliases_removed.push(alias.to_string());
            }
            report.already_vault += 1;
            continue;
        }
        if api_key.starts_with("env:") {
            report.skipped_env += 1;
            continue;
        }

        // 取出明文值：yaml 引用查表，字面量直取。
        let (literal, yaml_alias) = if let Some(alias) = api_key.strip_prefix("yaml:") {
            match creds.as_ref().and_then(|c| c.keys.get(alias)) {
                Some(v) => (v.clone(), Some(alias.to_string())),
                None => {
                    // 断引自修复（BUG-20261028-119）：vault 已有同名条目 =
                    // 先前迁移已把值收进 vault 但 config 提交失败（旧顺序在
                    // 事务点前删别名的现场）——直接把引用升格 `vault:<alias>`，
                    // 死路变自愈；vault 无此条目才是真断引，照旧不动配置。
                    if store.get(alias).is_ok() {
                        let repaired = alias.to_string();
                        obj.insert(
                            "api_key".to_string(),
                            serde_json::Value::String(format!("vault:{repaired}")),
                        );
                        report
                            .migrated
                            .push((display_name, repaired, "model".into()));
                    } else {
                        // 断引用：不动配置，报给调用方（fail loud 而非静默丢引用）。
                        report.yaml_broken.push((display_name, alias.to_string()));
                    }
                    continue;
                }
            }
        } else {
            (api_key.clone(), None)
        };

        let base_alias = yaml_alias
            .clone()
            .unwrap_or_else(|| nemesis_config::credentials::sanitize_alias(&display_name));
        let alias = vault_alias_for(store, &base_alias, &literal, &mut report);
        store.set(&alias, &literal, "model", "")?;
        obj.insert(
            "api_key".to_string(),
            serde_json::Value::String(format!("vault:{alias}")),
        );
        report.migrated.push((display_name, alias, "model".into()));

        if let Some(removed) = yaml_alias
            && let Some(creds) = creds.as_mut()
        {
            creds.keys.remove(&removed);
            report.creds_changed = true;
            report.yaml_aliases_removed.push(removed);
        }
    }
    Ok(report)
}

/// vault 别名冲突裁决（credentials 迁移同策略）：同值复用；异值 `__N` 后缀。
fn vault_alias_for(
    store: &VaultStore,
    base_alias: &str,
    literal: &str,
    report: &mut VaultMigrateReport,
) -> String {
    if let Ok(existing) = store.get(base_alias) {
        if existing == literal {
            report.reused += 1;
            return base_alias.to_string();
        }
        let mut n = 2;
        let mut candidate = format!("{base_alias}__{n}");
        loop {
            match store.get(&candidate) {
                Ok(v) if v == literal => {
                    report.reused += 1;
                    return candidate;
                }
                Ok(_) => {
                    n += 1;
                    candidate = format!("{base_alias}__{n}");
                }
                Err(_) => {
                    report
                        .conflicts
                        .push((base_alias.to_string(), candidate.clone()));
                    return candidate;
                }
            }
        }
    }
    base_alias.to_string()
}

// 覆盖率补充批次整文件 Windows 形态（DPAPI 生命周期 + OpenOptionsExt 共享
// 锁句柄），Linux 上编译期消失（2026-09-02 约定；nightly/coverage 均跑不跑）。
#[cfg(all(test, windows))]
mod cov_tests;
#[cfg(test)]
mod tests;
