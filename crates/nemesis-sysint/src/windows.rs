//! Windows 系统集成实现：HKCU 注册表（ARP/Run）+ WScript.Shell .lnk
//! 快捷方式 + Win32 信任证书库。
//!
//! 纪律（移植自 installer/windows ops.rs，2026-11-10 收编 bot）：
//! - 全部子进程 spawn 挂 `CREATE_NO_WINDOW`——绝无弹窗黑窗（与 CLAUDE.md
//!   「禁止 start /B 弹窗命令」纪律同源）；
//! - 注册表只写 HKCU（用户级，零 UAC）；
//! - ARP 条目只在卸载副本在场时写——指向缺失卸载器的控制面板按钮
//!   是死按钮，比不写更糟；
//! - 证书装 HKCU Root 触发系统确认框 = 设计内知情同意 UI（用户拍板
//!   「默认签名不安装」——只有显式 cert install 走到这）。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::certdata;
use crate::content::{self, RegValue};
use crate::{RegisterReport, RegisterRequest, StatusReport, UnregisterReport};

use windows_sys::Win32::Foundation::GetLastError;

/// HKCU ARP 卸载面板键路径。
pub const ARP_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\NemesisBot";
/// HKCU Run 自启键路径。
pub const RUN_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// 信任根证书库（HKCU per-user Root）。
pub const CERT_STORE_NAME: &str = "Root";

/// 子进程隐藏窗口旗标。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 隐藏窗口执行外部命令（powershell/tasklist 全走隐藏窗口）。
pub fn run_hidden(program: &str, args: &[&str]) -> Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("spawn {program} 失败"))
}

// ---------- 路径定位（生产形态：环境变量驱动） ----------

/// 开始菜单快捷方式路径。
pub fn start_menu_shortcut() -> Result<PathBuf> {
    let base = std::env::var("APPDATA").context("APPDATA 环境变量缺席")?;
    Ok(PathBuf::from(base)
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(content::APP_NAME)
        .join(format!("{}.lnk", content::APP_NAME)))
}

/// 桌面快捷方式路径。
pub fn desktop_shortcut_path() -> Result<PathBuf> {
    let home = std::env::var("USERPROFILE").context("USERPROFILE 环境变量缺席")?;
    Ok(PathBuf::from(home)
        .join("Desktop")
        .join(format!("{}.lnk", content::APP_NAME)))
}

// ---------- 注册（逐项 best-effort） ----------

/// 注册系统集成：ARP + 快捷方式 + 自启（HKCU，零 UAC）。
pub fn register(req: &RegisterRequest) -> RegisterReport {
    let mut rep = RegisterReport::default();

    // ① ARP 卸载面板条目（卸载副本在场才写——死按钮比不写更糟）。
    let uninstaller = req.install_dir.join("uninstall.exe");
    if uninstaller.exists() {
        match write_arp_at(ARP_PATH, &req.install_dir, &req.version) {
            Ok(()) => rep.registered.push("卸载面板条目（ARP）".into()),
            Err(e) => rep.warnings.push(format!("卸载面板条目: {e:#}")),
        }
    } else {
        rep.warnings
            .push("无 uninstall.exe，跳过卸载面板条目（开发目录注册只有自启/快捷方式）".into());
    }

    // ② 快捷方式（开始菜单恒建，桌面可选）。
    if req.shortcuts {
        match create_shortcuts(&req.install_dir, req.desktop) {
            Ok(made) => {
                rep.registered.push(format!("快捷方式 ×{}", made.len()));
            }
            Err(e) => rep.warnings.push(format!("快捷方式: {e:#}")),
        }
    }

    // ③ 开机自启（HKCU Run）。
    if req.autostart
        && let Err(e) = set_autostart_at(RUN_PATH, &req.install_dir)
    {
        rep.warnings.push(format!("开机自启: {e:#}"));
    } else if req.autostart {
        rep.registered.push("开机自启（Run）".into());
    }

    rep
}

/// 注销系统集成（幂等：条目缺席也回成功）。
pub fn unregister(_install_dir: &Path) -> UnregisterReport {
    let mut rep = UnregisterReport::default();
    match remove_arp_at(ARP_PATH) {
        Ok(()) => rep.removed.push("卸载面板条目（ARP）".into()),
        Err(e) => rep.warnings.push(format!("卸载面板条目: {e:#}")),
    }
    match remove_autostart_at(RUN_PATH) {
        Ok(()) => rep.removed.push("开机自启（Run）".into()),
        Err(e) => rep.warnings.push(format!("开机自启: {e:#}")),
    }
    let (removed, failures) = remove_shortcuts();
    if !removed.is_empty() {
        rep.removed.push(format!("快捷方式 ×{}", removed.len()));
    }
    for (path, err) in failures {
        rep.warnings.push(format!("快捷方式 {path}: {err}"));
    }
    rep
}

/// 状态读数（各条目在场/缺席 + 指向）。
pub fn status(install_dir: &Path) -> Result<StatusReport> {
    let mut rows: StatusReport = Vec::new();

    // ARP。
    match read_arp_summary() {
        Ok(Some(s)) => rows.push(("卸载面板条目（ARP）".into(), s)),
        Ok(None) => rows.push(("卸载面板条目（ARP）".into(), "未注册".into())),
        Err(e) => rows.push(("卸载面板条目（ARP）".into(), format!("读取失败: {e:#}"))),
    }

    // Run 自启。
    let run_state = read_run_value()
        .map(|v| match v {
            Some(s) => s,
            None => "未配置".into(),
        })
        .unwrap_or_else(|e| format!("读取失败: {e:#}"));
    rows.push(("开机自启（Run）".into(), run_state));

    // 快捷方式。
    for (label, path) in [
        ("开始菜单快捷方式", start_menu_shortcut()),
        ("桌面快捷方式", desktop_shortcut_path()),
    ] {
        let state = match path {
            Ok(p) if p.exists() => format!("在册（{}）", p.display()),
            Ok(_) => "未创建".into(),
            Err(e) => format!("定位失败: {e:#}"),
        };
        rows.push((label.into(), state));
    }

    rows.push(("安装目录".into(), install_dir.display().to_string()));
    Ok(rows)
}

// ---------- 注册表低层（_at 形态：键路径注入，测试用测试键） ----------

/// 写 HKCU ARP 键（键路径注入：生产传 [`ARP_PATH`]，测试传测试键——
/// 绝不写真实注册表）。
pub fn write_arp_at(path: &str, target: &Path, version: &str) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(path)
        .with_context(|| format!("建注册表键失败: HKCU\\{path}"))?;
    for (name, value) in content::arp_values(target, version) {
        match value {
            RegValue::Str(s) => key.set_value(name, &s)?,
            RegValue::Dword(v) => key.set_value(name, &v)?,
        }
    }
    Ok(())
}

/// 摘 ARP 键（NotFound = 幂等删除；与 fsutil::delete_file 幂等纪律同源）。
pub fn remove_arp_at(path: &str) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context(format!("删注册表键失败: HKCU\\{path}"))),
    }
}

/// 写 HKCU Run 自启值（键路径注入同 [`write_arp_at`]）。
pub fn set_autostart_at(path: &str, target: &Path) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(path)
        .with_context(|| format!("建注册表键失败: HKCU\\{path}"))?;
    key.set_value(content::APP_NAME, &content::autostart_run_value(target))?;
    Ok(())
}

/// 摘 Run 自启值（值缺席不报错）。
pub fn remove_autostart_at(path: &str) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(path, KEY_WRITE)
        .context("打开 Run 键失败")?;
    let _ = key.delete_value(content::APP_NAME);
    Ok(())
}

/// ARP 摘要读数（None = 键缺席）。
fn read_arp_summary() -> Result<Option<String>> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = match hkcu.open_subkey_with_flags(ARP_PATH, KEY_READ) {
        Ok(k) => k,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let version: String = key.get_value("DisplayVersion").unwrap_or_default();
    let loc: String = key.get_value("InstallLocation").unwrap_or_default();
    Ok(Some(format!("已注册（版本 {version}，位置 {loc}）")))
}

/// Run 自启值读数（None = 值缺席）。
fn read_run_value() -> Result<Option<String>> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(RUN_PATH, KEY_READ)?;
    match key.get_value::<String, _>(content::APP_NAME) {
        Ok(v) => Ok(Some(format!("已配置（{v}）"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// ---------- 快捷方式（WScript.Shell .lnk） ----------

/// 经 WScript.Shell 创建 .lnk（脚本内容单一真相源 content::lnk_script）。
pub fn make_lnk(lnk: &Path, target_exe: &Path, arguments: &str, workdir: &Path) -> Result<()> {
    if let Some(p) = lnk.parent() {
        std::fs::create_dir_all(p)?;
    }
    let script = content::lnk_script(lnk, target_exe, arguments, workdir);
    let out = run_hidden(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ],
    )?;
    if !out.status.success() {
        bail!(
            "创建快捷方式失败（{}）: {}",
            lnk.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if !lnk.exists() {
        bail!("快捷方式未落盘: {}", lnk.display());
    }
    Ok(())
}

/// 创建快捷方式（开始菜单恒建；桌面可选）。返回已创建路径。
/// 生产形态（真实用户路径）；测试走 [`create_shortcuts_at`] 注入形态。
pub fn create_shortcuts(target: &Path, desktop: bool) -> Result<Vec<PathBuf>> {
    let exe = target.join("nemesisbot.exe");
    let d = if desktop {
        Some(desktop_shortcut_path()?)
    } else {
        None
    };
    create_shortcuts_at(&start_menu_shortcut()?, d.as_deref(), &exe, target)
}

/// 快捷方式创建低层（路径注入：sm/desktop 由调用方给定，测试全临时目录）。
pub fn create_shortcuts_at(
    sm: &Path,
    desktop: Option<&Path>,
    exe: &Path,
    workdir: &Path,
) -> Result<Vec<PathBuf>> {
    let mut made = Vec::new();
    make_lnk(sm, exe, content::LNK_ARGUMENTS, workdir)?;
    made.push(sm.to_path_buf());
    if let Some(d) = desktop {
        make_lnk(d, exe, content::LNK_ARGUMENTS, workdir)?;
        made.push(d.to_path_buf());
    }
    Ok(made)
}

/// 删除快捷方式（best-effort）。返回（已删路径，失败清单）；开始菜单
/// NemesisBot 空目录壳顺手剪除（删 .lnk 后目录残留是资源管理器可见垃圾）。
/// 生产形态（真实用户路径）；测试走 [`remove_shortcuts_at`] 注入形态。
pub fn remove_shortcuts() -> (Vec<String>, Vec<(String, String)>) {
    let paths: Vec<PathBuf> = [start_menu_shortcut(), desktop_shortcut_path()]
        .into_iter()
        .flatten()
        .collect();
    let sm_dir = start_menu_shortcut()
        .ok()
        .and_then(|sm| sm.parent().map(Path::to_path_buf));
    let out = remove_shortcuts_at(&paths);
    if let Some(dir) = sm_dir
        && std::fs::read_dir(&dir).is_ok_and(|mut it| it.next().is_none())
    {
        let _ = std::fs::remove_dir(&dir);
    }
    out
}

/// 快捷方式删除低层（路径注入；不做空目录剪除——目录归调用方管）。
pub fn remove_shortcuts_at(paths: &[PathBuf]) -> (Vec<String>, Vec<(String, String)>) {
    let mut removed = Vec::new();
    let mut failures = Vec::new();
    for path in paths {
        if path.exists() {
            match std::fs::remove_file(path) {
                Ok(()) => removed.push(path.to_string_lossy().into()),
                Err(e) => failures.push((path.to_string_lossy().into(), e.to_string())),
            }
        }
    }
    (removed, failures)
}

// ---------- 信任根证书库（独立操作，默认永不安装） ----------

/// 安装根证书到 HKCU per-user Root 库（**系统确认框 = 设计内知情同意 UI**）。
/// 返回指纹（sha256 hex）。
pub fn cert_install(der: &[u8]) -> Result<String> {
    let fp = certdata::fingerprint(der);
    unsafe { cert_store_add(CERT_STORE_NAME, der)? };
    Ok(fp)
}

/// 从 Root 库摘除证书（按指纹匹配；不在库 = Err 显式报）。
pub fn cert_uninstall(der: &[u8]) -> Result<()> {
    if !unsafe { cert_store_delete(CERT_STORE_NAME, der)? } {
        bail!(
            "证书不在 Root 库（指纹 {}，无可摘除项）",
            certdata::fingerprint(der)
        );
    }
    Ok(())
}

/// 查询证书是否在 Root 库（按指纹匹配）。
pub fn cert_query(der: &[u8]) -> Result<bool> {
    unsafe { cert_store_find(CERT_STORE_NAME, der) }
}

/// 证书指纹比对辅助（store 遍历时逐证书算 sha256 对账）。
fn der_matches(der_bytes: &[u8], fingerprint: &str) -> bool {
    certdata::fingerprint(der_bytes) == fingerprint
}

/// 打开按名指定的 HKCU 系统证书库。生产用 "Root"；测试用自定义名
/// （同一 API 面，绕开 Root 的确认框——自动化可测的根源）。
unsafe fn open_store(name: &str) -> Result<*mut core::ffi::c_void> {
    use windows_sys::Win32::Security::Cryptography::CertOpenSystemStoreW;
    let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let store = unsafe { CertOpenSystemStoreW(0, name_w.as_ptr()) };
    if store.is_null() {
        bail!("打开证书库 {name} 失败（GetLastError={}）", unsafe {
            GetLastError()
        });
    }
    Ok(store)
}

/// 向指定库添加证书（REPLACE_EXISTING：重装幂等）。
/// `store_name` 注入形态：生产 "Root"，测试自定义名（不弹确认框）。
///
/// # Safety
/// 调用方保证 `der` 指向完整有效的 DER 证书载荷（Windows 证书 API 按原始
/// 指针读取该缓冲区）。
pub unsafe fn cert_store_add(store_name: &str, der: &[u8]) -> Result<()> {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_STORE_ADD_REPLACE_EXISTING, CertAddEncodedCertificateToStore,
        CertCloseStore, CertFreeCertificateContext, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };
    let store = unsafe { open_store(store_name)? };
    let mut ctx: *mut CERT_CONTEXT = std::ptr::null_mut();
    let ok = unsafe {
        CertAddEncodedCertificateToStore(
            store,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            der.as_ptr(),
            der.len() as u32,
            CERT_STORE_ADD_REPLACE_EXISTING,
            &mut ctx,
        )
    };
    if !ctx.is_null() {
        unsafe { CertFreeCertificateContext(ctx as *const _) };
    }
    unsafe { CertCloseStore(store, 0) };
    if ok == 0 {
        bail!(
            "证书写入 {} 库失败（GetLastError={}）",
            store_name,
            unsafe { GetLastError() }
        );
    }
    Ok(())
}

/// 按指纹在指定库找证书（遍历 + sha256 对账）。找到时**不消费**上下文
/// （手动释放）；返回 (是否找到)。
unsafe fn cert_store_find_ctx(
    store_name: &str,
    fingerprint: &str,
) -> Result<(
    bool,
    *const windows_sys::Win32::Security::Cryptography::CERT_CONTEXT,
)> {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_FIND_ANY, CertCloseStore, CertFindCertificateInStore,
        PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    };
    let store = unsafe { open_store(store_name)? };
    let mut prev: *const CERT_CONTEXT = std::ptr::null();
    loop {
        // CertFindCertificateInStore 消费（释放）prev——所有权随调用移交；
        // 循环每条路径要么返回要么给 prev 重赋值，无需中间置空。
        let ctx = unsafe {
            CertFindCertificateInStore(
                store,
                X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
                0,
                CERT_FIND_ANY,
                std::ptr::null(),
                prev,
            )
        };
        if ctx.is_null() {
            unsafe { CertCloseStore(store, 0) };
            return Ok((false, std::ptr::null()));
        }
        let bytes = unsafe {
            std::slice::from_raw_parts((*ctx).pbCertEncoded, (*ctx).cbCertEncoded as usize)
        };
        if der_matches(bytes, fingerprint) {
            unsafe { CertCloseStore(store, 0) };
            return Ok((true, ctx));
        }
        // 未命中：交还下一次调用释放。
        prev = ctx;
    }
}

/// 查询指定库是否含指纹证书。`store_name` 注入形态同 [`cert_store_add`]。
///
/// # Safety
/// 同 [`cert_store_add`]：`der` 必须是完整有效的 DER 载荷。
pub unsafe fn cert_store_find(store_name: &str, der: &[u8]) -> Result<bool> {
    let (found, ctx) = unsafe { cert_store_find_ctx(store_name, &certdata::fingerprint(der))? };
    if found && !ctx.is_null() {
        unsafe {
            windows_sys::Win32::Security::Cryptography::CertFreeCertificateContext(ctx);
        }
    }
    Ok(found)
}

/// 从指定库删除指纹匹配的证书（CertDeleteCertificateFromStore 消费并
/// 释放上下文）。返回是否确实删除（false = 库中无此证书）。
///
/// # Safety
/// 同 [`cert_store_add`]：`der` 必须是完整有效的 DER 载荷。
pub unsafe fn cert_store_delete(store_name: &str, der: &[u8]) -> Result<bool> {
    use windows_sys::Win32::Security::Cryptography::CertDeleteCertificateFromStore;
    let (_, ctx) = unsafe { cert_store_find_ctx(store_name, &certdata::fingerprint(der))? };
    if ctx.is_null() {
        return Ok(false);
    }
    let ok = unsafe { CertDeleteCertificateFromStore(ctx) };
    if ok == 0 {
        bail!("证书删除失败（GetLastError={}）", unsafe {
            GetLastError()
        });
    }
    Ok(true)
}
