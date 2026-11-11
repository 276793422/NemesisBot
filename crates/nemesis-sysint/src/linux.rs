//! Linux 系统集成实现：XDG 用户级三件套（~/.local/bin 符号链接 / XDG
//! .desktop / PATH 哨兵块）+ systemd --user unit 自启 + ca-certificates
//! 信任根（需 root，缺席诚实报错）。
//!
//! 纪律（移植自 installer/linux ops.rs，2026-11-10 收编 bot）：
//! - 零 sudo/UAC 前提：常规注册全部落在 $HOME 用户域；
//! - systemd unit 是 --autostart 可选项（best-effort：systemctl 缺席 →
//!   警告跳过，unit 文件照写，绝不失败整个注册）；
//! - 证书 = /usr/local/share/ca-certificates + update-ca-certificates，
//!   需 root（写不进/跑不动 = 诚实报错带根因，不做用户域假信任）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::certdata;
use crate::content;
use crate::{RegisterReport, RegisterRequest, StatusReport, UnregisterReport};

// ---------- 路径定位（生产形态：环境变量驱动，_at 形态供测试注入） ----------

/// `~/.local/bin` 符号链接目标路径。
pub fn local_bin_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home).join(".local/bin/nemesisbot"))
}

/// `~/.profile` 路径。
pub fn profile_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home).join(".profile"))
}

/// XDG 应用入口路径（XDG_DATA_HOME 尊重 → $HOME/.local/share）。
pub fn desktop_entry_path() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return Ok(PathBuf::from(xdg).join("applications/nemesisbot.desktop"));
    }
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home)
        .join(".local/share/applications")
        .join("nemesisbot.desktop"))
}

/// systemd user unit 路径（XDG_CONFIG_HOME 尊重 → $HOME/.config）。
pub fn systemd_unit_path() -> Result<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let base = match base {
        Some(b) => PathBuf::from(b),
        None => {
            let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("systemd/user/nemesisbot.service"))
}

/// ca-certificates 本地证书目录（Debian/Ubuntu 系；需 root）。
pub const CERT_DIR: &str = "/usr/local/share/ca-certificates";

// ---------- 注册（逐项 best-effort） ----------

/// 注册系统集成：symlink + .desktop + PATH 哨兵块 +（可选）systemd unit。
pub fn register(req: &RegisterRequest) -> RegisterReport {
    let mut rep = RegisterReport::default();

    // ① ~/.local/bin 符号链接（PATH 三件套之一）。
    if req.shortcuts {
        match local_bin_path()
            .and_then(|link| install_symlink(&link, &req.install_dir.join("nemesisbot")))
        {
            Ok(()) => rep.registered.push("命令符号链接（~/.local/bin）".into()),
            Err(e) => rep.warnings.push(format!("符号链接: {e:#}")),
        }
        // ② XDG .desktop 入口。
        match desktop_entry_path().and_then(|path| {
            write_desktop_entry_at(
                &path,
                &req.install_dir.join("nemesisbot"),
                &req.install_dir.join("share/icon.png"),
            )
        }) {
            Ok(_) => rep.registered.push("桌面入口（XDG .desktop）".into()),
            Err(e) => rep.warnings.push(format!("桌面入口: {e:#}")),
        }
        // ③ PATH 哨兵块（~/.profile）。
        match profile_path().and_then(|p| append_path_snippet_at(&p)) {
            Ok(true) => rep
                .registered
                .push("PATH 注记（~/.profile，重登录生效）".into()),
            Ok(false) => rep.registered.push("PATH 注记（已在册）".into()),
            Err(e) => rep.warnings.push(format!("PATH 注记: {e:#}")),
        }
    }

    // ④ 自启 = systemd user unit（unit 文件照写；systemctl 激活 best-effort）。
    if req.autostart {
        let unit_res = systemd_unit_path()
            .and_then(|path| write_unit_at(&path, &req.install_dir.join("nemesisbot")));
        match unit_res {
            Ok(path) => {
                rep.registered
                    .push(format!("自启 unit（{}）", path.display()));
                for w in activate_systemd_unit() {
                    rep.warnings.push(w);
                }
            }
            Err(e) => rep.warnings.push(format!("自启 unit: {e:#}")),
        }
    }

    rep
}

/// 注销系统集成（幂等）。
///
/// **PATH 哨兵块按设计保留**：~/.profile 可能含用户手工编辑的邻近内容，
/// 自动摘除有误伤面；哨兵块原地无害（幂等识别：重复注册不会重复写）。
pub fn unregister(_install_dir: &Path) -> UnregisterReport {
    let mut rep = UnregisterReport::default();

    // ① symlink。
    if let Ok(p) = local_bin_path() {
        match std::fs::symlink_metadata(&p) {
            Ok(_) => match std::fs::remove_file(&p) {
                Ok(()) => rep.removed.push("命令符号链接（~/.local/bin）".into()),
                Err(e) => rep.warnings.push(format!("符号链接删除: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => rep.warnings.push(format!("符号链接: {e}")),
        }
    }
    // ② .desktop。
    if let Ok(p) = desktop_entry_path() {
        match std::fs::symlink_metadata(&p) {
            Ok(_) => match std::fs::remove_file(&p) {
                Ok(()) => rep.removed.push("桌面入口（XDG .desktop）".into()),
                Err(e) => rep.warnings.push(format!("桌面入口删除: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => rep.warnings.push(format!("桌面入口: {e}")),
        }
    }
    // ③ systemd unit（disable --now + 删文件 + daemon-reload，全部 best-effort）。
    if let Ok(p) = systemd_unit_path()
        && std::fs::symlink_metadata(&p).is_ok()
    {
        for w in deactivate_systemd_unit() {
            rep.warnings.push(w);
        }
        match std::fs::remove_file(&p) {
            Ok(()) => rep.removed.push("自启 unit".into()),
            Err(e) => rep.warnings.push(format!("自启 unit 删除: {e}")),
        }
    }
    rep.removed
        .push("PATH 注记按设计保留（哨兵块原地无害）".into());
    rep
}

/// 状态读数。
pub fn status(install_dir: &Path) -> Result<StatusReport> {
    let mut rows: StatusReport = Vec::new();

    // symlink。
    let link_state = local_bin_path().map_or_else(
        |e| format!("定位失败: {e}"),
        |p| match std::fs::read_link(&p) {
            Ok(t) => format!("在册（{} → {}）", p.display(), t.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "未创建".into(),
            Err(_) => format!("在册（{}，非符号链接）", p.display()),
        },
    );
    rows.push(("命令符号链接".into(), link_state));

    // .desktop。
    let de_state = desktop_entry_path().map_or_else(
        |e| format!("定位失败: {e}"),
        |p| {
            if p.exists() {
                format!("在册（{}）", p.display())
            } else {
                "未创建".into()
            }
        },
    );
    rows.push(("桌面入口".into(), de_state));

    // PATH 哨兵块。
    let path_state = profile_path().map_or_else(
        |e| format!("定位失败: {e}"),
        |p| {
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            if body.contains(content::PATH_MARKER_BEGIN) {
                format!("在册（{}）", p.display())
            } else {
                "未写入".into()
            }
        },
    );
    rows.push(("PATH 注记".into(), path_state));

    // unit。
    let unit_state = systemd_unit_path().map_or_else(
        |e| format!("定位失败: {e}"),
        |p| {
            if p.exists() {
                format!("在册（{}）", p.display())
            } else {
                "未创建".into()
            }
        },
    );
    rows.push(("自启 unit".into(), unit_state));

    rows.push(("安装目录".into(), install_dir.display().to_string()));
    Ok(rows)
}

// ---------- IO 低层（_at 形态：路径注入，测试不碰真实 $HOME） ----------

/// 幂等重建符号链接（已指向同目标 = 不动；否则摘旧重建）。
pub fn install_symlink(link: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("建目录失败: {}", parent.display()))?;
    }
    // 幂等：已是正确符号链接则跳过。
    if let Ok(meta) = std::fs::symlink_metadata(link)
        && meta.file_type().is_symlink()
        && std::fs::read_link(link).is_ok_and(|p| p == target)
    {
        return Ok(());
    }
    let _ = std::fs::remove_file(link);
    symlink(target, link)
        .with_context(|| format!("建符号链接失败: {} → {}", link.display(), target.display()))
}

/// 写 XDG .desktop 入口（路径注入形态）。
pub fn write_desktop_entry_at(path: &Path, exec: &Path, icon: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content::desktop_entry_text(exec, icon))
        .with_context(|| format!("写桌面入口失败: {}", path.display()))
}

/// 写 systemd user unit（路径注入形态）。返回 unit 路径。
pub fn write_unit_at(path: &Path, exec: &Path) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content::systemd_unit_text(exec))
        .with_context(|| format!("写 systemd unit 失败: {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// 追加 PATH 哨兵块到 profile 文件（幂等：已含哨兵或 PATH 已精确含
/// `~/.local/bin` = 不动）。返回是否实际追加。
pub fn append_path_snippet_at(profile: &Path) -> Result<bool> {
    let existing = std::fs::read_to_string(profile).unwrap_or_default();
    let local_bin = local_bin_path()?;
    if !content::needs_path_append(
        &std::env::var("PATH").unwrap_or_default(),
        &existing,
        &local_bin.to_string_lossy(),
    ) {
        return Ok(false);
    }
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(profile)
            .with_context(|| format!("打开 profile 失败: {}", profile.display()))?;
        writeln!(f, "{}", content::path_snippet())?;
    }
    Ok(true)
}

/// systemctl --user daemon-reload + enable（best-effort：缺席/失败 = 警告串）。
pub fn activate_systemd_unit() -> Vec<String> {
    let mut warnings = Vec::new();
    let reload = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    match reload {
        Ok(o) if o.status.success() => {
            let enable = std::process::Command::new("systemctl")
                .args(["--user", "enable", "nemesisbot.service"])
                .output()
                .context("systemctl enable 执行失败");
            match enable {
                Ok(o) if o.status.success() => {}
                Ok(o) => warnings.push(format!(
                    "systemctl enable 失败（unit 已写入未激活）: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                )),
                Err(e) => warnings.push(format!("systemctl enable 执行失败: {e}")),
            }
        }
        Ok(o) => warnings.push(format!(
            "systemctl --user daemon-reload 失败（自启未激活）: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(_) => warnings
            .push("systemctl 缺席（无 systemd 或 PATH 缺失），自启 unit 已写入但未激活".into()),
    }
    warnings
}

/// systemctl --user disable --now + daemon-reload（best-effort：全静默）。
pub fn deactivate_systemd_unit() -> Vec<String> {
    let warnings = Vec::new();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "nemesisbot.service"])
        .output();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    warnings
}

// ---------- 信任根证书库（ca-certificates，需 root，独立操作默认不装） ----------

/// 本库落名：`nemesisbot-<指纹前 12 位>.crt`（指纹可回查 + 卸载可定位）。
fn cert_file_name(der: &[u8]) -> String {
    format!("nemesisbot-{}.crt", &certdata::fingerprint(der)[..12])
}

/// PEM 载体（ca-certificates 只吃 PEM）。
fn pem_wrap(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

/// 安装根证书：写 `/usr/local/share/ca-certificates/nemesisbot-<fp12>.crt`
/// + `update-ca-certificates`（两步都需 root；失败诚实报错带根因）。
pub fn cert_install(der: &[u8]) -> Result<String> {
    cert_write_file_at(Path::new(CERT_DIR), der)?;
    run_update_ca_certificates()?;
    Ok(certdata::fingerprint(der))
}

/// 摘除根证书：删落名文件 + `update-ca-certificates`（文件不在 = Err）。
pub fn cert_uninstall(der: &[u8]) -> Result<()> {
    cert_remove_file_at(Path::new(CERT_DIR), der)?;
    run_update_ca_certificates()
}

/// 查询证书是否在信任库（落名文件在场 = 在库；不做系统链状态深查）。
pub fn cert_query(der: &[u8]) -> Result<bool> {
    Ok(Path::new(CERT_DIR).join(cert_file_name(der)).exists())
}

// ---------- 证书文件层低层（`_at` 注入形态：目录可替换，测试不碰真实库） ----------

/// 写证书 PEM 文件到指定目录（落名规则与生产 [`cert_install`] 同源）。
pub fn cert_write_file_at(dir: &Path, der: &[u8]) -> Result<PathBuf> {
    let path = dir.join(cert_file_name(der));
    std::fs::write(&path, pem_wrap(der))
        .with_context(|| format!("写 {} 失败（需要 root/sudo）", path.display()))?;
    Ok(path)
}

/// 删证书 PEM 文件（缺席 = Err 显式报，与 Windows 摘除语义对齐）。
pub fn cert_remove_file_at(dir: &Path, der: &[u8]) -> Result<()> {
    let path = dir.join(cert_file_name(der));
    if !path.exists() {
        bail!("证书不在信任库（{}，无可摘除项）", path.display());
    }
    std::fs::remove_file(&path)
        .with_context(|| format!("删 {} 失败（需要 root/sudo）", path.display()))
}

/// `update-ca-certificates` 跑一次（失败诚实报错带根因）。
fn run_update_ca_certificates() -> Result<()> {
    let out = std::process::Command::new("update-ca-certificates")
        .output()
        .context("update-ca-certificates 执行失败（命令缺席或需要 root）")?;
    if !out.status.success() {
        bail!(
            "update-ca-certificates 失败（需要 root/sudo）: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
