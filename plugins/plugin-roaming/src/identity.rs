//! 插件内身份面（M2）：roaming.key 自产自销——语义镜像宿主
//! `nemesis_cluster::roaming::endpoint::RoamingIdentity`（iroh SecretKey =
//! ed25519 seed 32B 包装，node_id = 公钥 32B；本 crate 用 ed25519-dalek
//! `SigningKey` 数学等价实现）。文件路径与格式不变：`<workspace>/cluster/
//! roaming.key`，hex 32B seed，Unix 0600——磁盘真相源迁移零成本（宿主旧
//! 密钥直接续用）。
//!
//! 等价性由 dev-dep 交叉验证钉死（tests：同 seed 双实现 node_id 一致）。

use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;

/// 节点漫游身份（ed25519 密钥对 + 派生 node_id）。
pub struct Identity {
    signing: SigningKey,
}

impl Identity {
    /// 生成全新身份（内存态；持久化用 [`persist`]）。
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).expect("OS 随机源不可用");
        Self::from_seed(&seed)
    }

    /// 从 32B seed 还原（加载 roaming.key / 宿主过渡传递 seed 用）。
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// 32B seed（roaming.key 的二进制形态）。
    pub fn seed(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    /// node_id hex（= ed25519 公钥 32B；`iroh:` 地址后缀同源语义）。
    pub fn node_id_hex(&self) -> String {
        hex::encode(self.signing.verifying_key().to_bytes())
    }

    /// 加载或生成：文件存在 → 读 hex seed 还原；否则生成并落盘。
    /// 落盘权限 Unix 0600（Windows 无等价语义，依赖目录 ACL）；父目录
    /// 不存在则创建。返回 (身份, 是否新生成)。
    pub fn load_or_create(workspace: &Path) -> Result<(Self, bool), String> {
        let path = roaming_key_path(workspace);
        if path.exists() {
            return Ok((Self::load(&path)?, false));
        }
        let id = Self::generate();
        persist(&id, &path)?;
        Ok((id, true))
    }

    /// 从 roaming.key 还原（hex 32B seed；非法/缺文件 loud 报错）。
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let bytes = hex::decode(text.trim()).map_err(|e| format!("roaming.key hex 非法: {e}"))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|v: Vec<u8>| format!("roaming.key 长度 {} 字节 ≠ 32（seed）", v.len()))?;
        Ok(Self::from_seed(&seed))
    }
}

/// 持久化身份到 roaming.key（hex seed；父目录自动创建；Unix 0600）。
pub fn persist(id: &Identity, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    std::fs::write(path, hex::encode(id.seed()))
        .map_err(|e| format!("写 {} 失败: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("收紧 {} 权限失败: {e}", path.display()))?;
    }
    Ok(())
}

/// roaming.key 路径（workspace/cluster/roaming.key——宿主同款，格式不变）。
pub fn roaming_key_path(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("roaming.key")
}
