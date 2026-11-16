//! 插件内原子写（插件自持依赖，不引 nemesis-utils；语义对齐宿主
//! `nemesis_utils::write_file_atomic`：同目录唯一临时名 → create_new +
//! (unix) mode 即挂 → write → flush → sync_all → rename 覆盖 → 失败清理
//! 临时文件）。
//!
//! 消费方：identity（roaming.key seed）与 relay_server（TLS 私钥/证书
//! PEM）——凭据面文件禁「先明文写后收紧」（unix 0644 窗口）与裸写撕裂
//! （进程中断 = 半截 hex/PEM，且 relay 证书幂等复用逻辑会把半截文件当
//! 有效内容复用）。

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 原子写文件。`private = true` 时 unix 权限在临时文件创建时即挂 0600
/// （Windows no-op——ACL 不处理，诚实边界，依赖目录 ACL）。
pub(crate) fn atomic_write(path: &Path, data: &[u8], private: bool) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    let tmp = parent.join(format!(
        ".tmp-{}-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;

    let write = || -> Result<(), String> {
        let mut f = opts
            .open(&tmp)
            .map_err(|e| format!("写 {} 失败（临时文件）: {e}", path.display()))?;
        f.write_all(data)
            .map_err(|e| format!("写 {} 失败: {e}", path.display()))?;
        f.flush()
            .map_err(|e| format!("flush {} 失败: {e}", path.display()))?;
        f.sync_all()
            .map_err(|e| format!("sync {} 失败: {e}", path.display()))?;
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| format!("改名 {} 失败: {e}", path.display()))
    };
    match write() {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
