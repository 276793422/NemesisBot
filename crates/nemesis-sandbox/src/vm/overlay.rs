//! qcow2 overlay 创建（P5 第 2 步）——per-agent 差分盘的**唯一参数产地**。
//!
//! base 镜像只读共享、每个 agent 实例一个 overlay（`-b backing` 差分）：
//! 启动零拷贝、丢弃即回收。backing 允许指向另一个 overlay——**快照树的
//! 预留点**（§5「快照树」恢复路径 = 在此函数之上做多层 backing 链，接口
//! 现已留好）。
//!
//! 纪律：qemu-img 是外部工具——参数在纯函数 [`create_overlay_args`] 产出
//! （可断言），真实执行在 [`create_overlay`]（退出码非 0 = 错误带 stderr
//! 现场）。backing 必须**已存在**且取绝对路径（qcow2 元数据记的是
//! backing 路径字符串，相对路径会随 CWD 漂移——启动期必炸的隐患，装配期
//! 直接钉死绝对形态）。

use std::path::{Path, PathBuf};

/// 规范化 canonicalize 产出的 Windows `\\?\C:\...` UNC 前缀（QEMU 对
/// `\\?\` 形态的兼容不稳，落回常规盘符路径）。
fn strip_verbatim(p: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let s = p.as_os_str().to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
    }
    p
}

/// 装配 `qemu-img create` 参数表（纯函数）。
///
/// 返回完整 argv（含程序名）：`[qemu_img, create, -f, qcow2, -b,
/// <abs backing>, -F, qcow2, <overlay>, <size>G]`。
pub fn create_overlay_args(
    qemu_img: &Path,
    overlay: &Path,
    backing: &Path,
    size_gb: u32,
) -> Result<Vec<String>, String> {
    if !backing.exists() {
        return Err(format!(
            "overlay backing 文件不存在: {}（base 镜像管线先于 overlay 创建）",
            backing.display()
        ));
    }
    let backing_abs: PathBuf = backing
        .canonicalize()
        .map_err(|e| format!("backing 规范化失败 {}: {e}", backing.display()))?;
    let backing_abs = strip_verbatim(backing_abs);
    Ok(vec![
        qemu_img.display().to_string(),
        "create".into(),
        "-f".into(),
        "qcow2".into(),
        "-b".into(),
        backing_abs.display().to_string(),
        "-F".into(),
        "qcow2".into(),
        overlay.display().to_string(),
        format!("{size_gb}G"),
    ])
}

/// 真实执行 overlay 创建（qemu-img 实跑；父目录自动补建；退出码非 0 =
/// Err 带 stderr 现场）。
pub async fn create_overlay(
    qemu_img: &Path,
    overlay: &Path,
    backing: &Path,
    size_gb: u32,
) -> Result<(), String> {
    if let Some(parent) = overlay.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("overlay 父目录创建失败 {}: {e}", parent.display()))?;
    }
    let args = create_overlay_args(qemu_img, overlay, backing, size_gb)?;
    let out = tokio::process::Command::new(&args[0])
        .args(&args[1..])
        .output()
        .await
        .map_err(|e| format!("qemu-img 不可执行 {}: {e}", args[0]))?;
    if !out.status.success() {
        return Err(format!(
            "qemu-img create 退出码非 0（{}）：{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
