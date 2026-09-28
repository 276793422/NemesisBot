//! DACL 定向档 D1：workspace SID 确定性派生。
//!
//! 设计文档：`docs/PLAN/2026-09-27_windows-acl-targeted-deny-design.md` §3.1。
//! write-restricted 受限令牌方案的身份锚——工作区路径派生出一个**确定性**
//! 的自定义 SID，树上 standing GRANT ACE（D2）与子进程 restricting SIDs
//! （D3）以它对齐。
//!
//! ## 派生规则（确定性三步）
//!
//! ```text
//! canonicalize(workspace)                      // 解析 junction/symlink 到真实路径
//! bytes = SHA-256(lowercase(strip_verbatim(canon_path)))
//! sid   = S-1-5-21-<be32(bytes[0..4])>-<be32(bytes[4..8])>-<be32(bytes[8..12])>
//! ```
//!
//! - **canonicalize 必须**：`C:\ws` 与经 junction 的 `C:\link\ws` 收敛到同
//!   一真实路径 → 同一 SID。standing ACE 是持久物，派生不稳定 = 旧 ACE 全
//!   部失效（复检候选 2a 实证的 junction 语义在这里是助力：路径解析一次到
//!   位）。
//! - **strip_verbatim**：`std::fs::canonicalize` 在 Windows 产出
//!   `\\?\C:\...`（UNC 为 `\\?\UNC\server\share`）形态；剥掉前缀让 hash
//!   输入是普通绝对路径（日志展示 SID 溯源时可读）。
//! - **lowercase**：NTFS 大小写不敏感，`C:\Ws` 与 `c:\ws` 是同一目录，必
//!   须同 SID。
//! - **形态 `S-1-5-21-a-b-c`**：模仿域 SID 三子授权形态，96 位截断（32 位
//!   ×3）。冲突面是天文小概率，且**冲突方向安全**——撞 SID 的两个工作区
//!   共享授权面，等价于用户手动合并授权（本地单用户场景可接受）。
//!
//! 本模块是纯逻辑（SHA-256 + 字符串拼装），全平台编译——Windows 消费方
//! （D2 `ensure_grant_ace_tree` / D3 `token.rs`）各自接线；非 Windows 平台
//! 编译无害（供跨平台单测钉纯函数行为）。

use std::path::Path;

use sha2::{Digest, Sha256};

// 非 Windows 平台：lib 目标无消费方（D2/D3 接线都在 cfg(windows) 侧），
// 但跨平台单测仍钉这三个纯函数的行为——诚实 allow（有单测证明活性），
// 非 Windows 下防 dead_code 撞 clippy -D warnings 门禁。

/// canonicalize 结果的规范化：剥 Windows verbatim 前缀 + lowercase。
///
/// 输入应是 `canonicalize` 的产物（绝对、已解析链接）；对非 verbatim 输入
/// 原样 lowercase 返回（Unix 路径永不命中前缀判定，单一代码路径跨平台）。
/// 模块内可见 + 测试直测（`pub(crate)` 收窄——派生规则的 hash 输入形态是
/// 内部约定，不进公共 API 面）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn normalize_canonical(canon: &Path) -> String {
    let s = canon.to_string_lossy();
    let stripped = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    };
    stripped.to_lowercase()
}

/// 96 位截断：SHA-256 摘要前 12 字节按大端拆 3 个 u32 子授权，拼
/// `S-1-5-21-a-b-c` 形态（纯函数，单测钉死与派生规则的字节序约定；
/// `pub(crate)` 同 normalize_canonical——内部字节序契约不进公共 API）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn sid_from_digest(digest: &[u8; 32]) -> String {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    format!(
        "S-1-5-21-{}-{}-{}",
        be32(&digest[0..4]),
        be32(&digest[4..8]),
        be32(&digest[8..12]),
    )
}

/// 从工作区路径派生 workspace SID（确定性）。
///
/// 路径不存在/不可达 → Err（canonicalize 失败）——D2 的 standing ACE 打在
/// 不存在的树上无意义，fail-closed 交给调用方（executor 装配点工作区必然
/// 存在；测试路径用真实 tempdir）。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn derive_workspace_sid(workspace: &Path) -> Result<String, String> {
    let canon = workspace.canonicalize().map_err(|e| {
        format!(
            "workspace canonicalize 失败（{}）: {e}——工作区必须已存在才能派生 SID",
            workspace.display()
        )
    })?;
    let normalized = normalize_canonical(&canon);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(sid_from_digest(&digest))
}
