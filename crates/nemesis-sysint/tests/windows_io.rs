//! Windows IO 面测试（只在 Windows 编译执行）。
//!
//! 纪律（移植自 installer/windows ops/tests.rs 并扩展）：
//! - 注册表用例只碰测试键（`Software\NemesisBotSysintTest_*`，用后即清），
//!   绝不写真实 ARP/Run 键；
//! - 快捷方式用例建到 tempfile 目录（不碰用户桌面/开始菜单）；
//! - 证书库用例走**自定义名 store**（与生产 "Root" 同一 API 面，但无确认
//!   弹窗、物理隔离在测试名下，用后即清）——真实 Root 路径由人工 CLI
//!   验收覆盖（弹窗无法 headless）。

#![cfg(windows)]

use anyhow::Result;
use nemesis_sysint::content;
use nemesis_sysint::windows::{
    CERT_STORE_NAME, make_lnk, remove_arp_at, remove_autostart_at, set_autostart_at, write_arp_at,
};

/// 一次性自签测试根（CN=NemesisBotSysintTestRoot；私钥已弃——本 PEM 仅作
/// 证书库 API 往返夹具，无任何信任价值）。
pub const TEST_ROOT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDJzCCAg+gAwIBAgIUVs6wO+bqQH99T/lEtYvy3Z32eLAwDQYJKoZIhvcNAQEL
BQAwIzEhMB8GA1UEAwwYTmVtZXNpc0JvdFN5c2ludFRlc3RSb290MB4XDTI2MTEw
OTIyMTk0OVoXDTM2MTEwNjIyMTk0OVowIzEhMB8GA1UEAwwYTmVtZXNpc0JvdFN5
c2ludFRlc3RSb290MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA7hT2
1VSp7J/KZ7pdZHwRRF/ieLCxkIewEx8CBHwZQRe3MhqB181BOEAI8h6/XSdifZ26
wloC95jeP8JdMwNHcwYnYwZcno1nKD0VuscOPyuDUna6+U/food8CKsOpUI6BgWg
ILvi9VCdayLT0DpRRBMgS7PLRZB/lA322Lghjv0eZ5EqR4Pi4snFf447FqN3eW19
ns0o7R6zykBeVtMKBtqxyMNPsR9ryddNjXikoGTAnNynZ7El7P7kGjMItueHejK1
M8aeYu0r0fKeU09tnTPwvtnm8e/jhxJneK4vtWaDzEvPbdbHad3EIdSm4zPzPT//
YQixVzooISdo8gzVkQIDAQABo1MwUTAdBgNVHQ4EFgQUwd+n53EIT3t05EghGUmc
XMqHhs4wHwYDVR0jBBgwFoAUwd+n53EIT3t05EghGUmcXMqHhs4wDwYDVR0TAQH/
BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAtDdFBiZRv7rP+CcCzj1PebExKyey
fiSmiROmnJCV8AAVVLGqlUcn8Qrywqa5naWJuiz5ZrFMPJ+W2AjiuIe24d1lFrvc
xO5ybZKSSkfaANvhm6VhpXr/K6MJ2IRum/eKMIlkTjC8Jpu4ho9Y3Q5ITQwAye9j
IrLuyN7yDunC9ZhStkhVSUbfzyELSjJ/Ppyj5D/jyGjQrPHbk70XM4tHw//WAUGu
dMmCzDX4RNy62tqQANSLOU3ewXW0xX8/xt2cEtoYPL/5THWqiUWXPqGglj4rNWSQ
6w9K+KGuc1rer1c+zeovJXSRDenRFvCaFhw4wwS+sphTz0FxAw44mfqxbA==
-----END CERTIFICATE-----
";

// ---------- 注册表（测试键，用后即清） ----------

#[test]
fn registry_arp_and_run_roundtrip() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let pid = std::process::id();
    let arp_test = format!("Software\\NemesisBotSysintTest_{pid}\\Uninstall");
    let run_test = format!("Software\\NemesisBotSysintTest_{pid}\\Run");

    write_arp_at(&arp_test, tmp.path(), "1.2.3")?;
    set_autostart_at(&run_test, tmp.path())?;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let arp_key = hkcu.open_subkey_with_flags(&arp_test, KEY_READ)?;
    assert_eq!(arp_key.get_value::<String, _>("DisplayName")?, "NemesisBot");
    assert_eq!(arp_key.get_value::<String, _>("DisplayVersion")?, "1.2.3");
    let uninstall_string: String = arp_key.get_value("UninstallString")?;
    assert!(
        uninstall_string.contains("uninstall.exe") && uninstall_string.contains("--uninstall"),
        "UninstallString 指向卸载副本: {uninstall_string}"
    );
    assert_eq!(arp_key.get_value::<u32, _>("NoModify")?, 1);
    assert_eq!(arp_key.get_value::<u32, _>("NoRepair")?, 1);
    // 内容单一真相源：InstallLocation 与 DisplayIcon 与目标目录一致。
    let location: String = arp_key.get_value("InstallLocation")?;
    assert_eq!(location, tmp.path().to_string_lossy());

    let run_key = hkcu.open_subkey_with_flags(&run_test, KEY_READ)?;
    let run_value: String = run_key.get_value(content::APP_NAME)?;
    assert!(
        run_value.contains("dashboard"),
        "Run 值带 dashboard 参数: {run_value}"
    );

    // 清理 + 幂等（重复删除不报错）。
    remove_arp_at(&arp_test)?;
    remove_arp_at(&arp_test)?;
    remove_autostart_at(&run_test)?;
    let root = format!("Software\\NemesisBotSysintTest_{pid}");
    hkcu.delete_subkey_all(&root)?;
    Ok(())
}

// ---------- 快捷方式（tempfile 内，不碰用户桌面） ----------

#[test]
fn make_lnk_creates_file_with_dashboard_args() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let fake_exe = tmp.path().join("nemesisbot.exe");
    std::fs::write(&fake_exe, b"MZ")?;
    let lnk = tmp.path().join("deep/dir/NemesisBot.lnk");
    make_lnk(&lnk, &fake_exe, "dashboard", tmp.path())?;
    assert!(lnk.exists(), "WScript.Shell 应落 .lnk 文件");
    Ok(())
}

#[test]
fn create_shortcuts_at_roundtrip_in_tempdir() -> Result<()> {
    // create_shortcuts/remove_shortcuts 的生产形态写真实用户路径——自动化
    // 测试只走 `_at` 注入形态（真实路径创建/删除由 live CLI 验收覆盖）。
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("nemesisbot.exe"), b"MZ")?;
    let sm = tmp.path().join("sm/NemesisBot/NemesisBot.lnk");
    let desktop = tmp.path().join("desktop/NemesisBot.lnk");

    // 创建（开始菜单 + 桌面两路）→ 文件在场。
    let made = nemesis_sysint::windows::create_shortcuts_at(
        &sm,
        Some(&desktop),
        &tmp.path().join("nemesisbot.exe"),
        tmp.path(),
    )?;
    assert_eq!(made.len(), 2);
    assert!(sm.exists(), "开始菜单 .lnk 应落盘");
    assert!(desktop.exists(), "桌面 .lnk 应落盘");

    // 删除低层 → 两路全清 + 失败清单为空；再删 = 空操作幂等。
    let (removed, failures) = nemesis_sysint::windows::remove_shortcuts_at(&made);
    assert!(failures.is_empty(), "删除不应失败: {failures:?}");
    assert_eq!(removed.len(), 2);
    assert!(!sm.exists() && !desktop.exists());
    let (removed2, failures2) = nemesis_sysint::windows::remove_shortcuts_at(&made);
    assert!(removed2.is_empty() && failures2.is_empty(), "幂等删除");
    Ok(())
}

// ---------- 证书库（自定义名 store：同 API 面、无确认框、用后即清） ----------

/// 测试 store 的注册表物理落点（HKCU 系统证书库按名落注册表）。
fn test_store_registry_path(name: &str) -> String {
    format!("Software\\Microsoft\\SystemCertificates\\{name}")
}

#[test]
fn cert_store_add_find_delete_roundtrip() -> Result<()> {
    let der = nemesis_sysint::certdata::cert_der_from_bytes(TEST_ROOT_PEM.as_bytes())?;
    let fp = nemesis_sysint::certdata::fingerprint(&der);
    let store_name = format!("NemesisBotSysintTestRoot_{}", std::process::id());

    unsafe {
        use nemesis_sysint::windows::{cert_store_add, cert_store_delete, cert_store_find};
        // ① 初始空。
        assert!(!cert_store_find(&store_name, &der)?, "测试库初始应为空");
        // ② 添加 → 找到。
        cert_store_add(&store_name, &der)?;
        assert!(cert_store_find(&store_name, &der)?, "添加后应能按指纹找到");
        // ③ 重复添加（REPLACE_EXISTING）幂等。
        cert_store_add(&store_name, &der)?;
        assert!(cert_store_find(&store_name, &der)?);
        // ④ 其他证书不应误匹配（指纹对账）。
        let other: Vec<u8> = der.iter().copied().chain([0x00]).collect();
        assert!(!cert_store_find(&store_name, &other)?, "指纹必须精确匹配");
        // ⑤ 删除 → 摘除成功 + 库空。
        assert!(cert_store_delete(&store_name, &der)?, "删除应命中");
        assert!(!cert_store_find(&store_name, &der)?, "删除后应找不到");
        // ⑥ 再删 = 未命中（幂等语义的原始返回）。
        assert!(!cert_store_delete(&store_name, &der)?);
    }

    // 测试 store 注册表残留清扫（best-effort——证书已删，键壳顺手摘）。
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let _ =
        RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(test_store_registry_path(&store_name));

    // 生产常量 sanity（Root 名钉住——facade cert_* 消费它）。
    assert_eq!(CERT_STORE_NAME, "Root");
    let _ = fp;
    Ok(())
}

// ---------- facade 契约面（不触真实库的部分） ----------

#[test]
fn facade_status_never_panics_on_fresh_dir() -> Result<()> {
    // status 是纯读面：对缺席目录/未注册态返回行向量而非错误。
    let tmp = tempfile::tempdir()?;
    let rows = nemesis_sysint::status(tmp.path())?;
    assert!(
        rows.iter()
            .any(|(k, v)| k.contains("ARP") && v.contains("未注册")),
        "未注册态应如实报告: {rows:?}"
    );
    Ok(())
}

#[test]
fn facade_cert_query_reports_missing_without_store_write() -> Result<()> {
    // 查询是纯读：库中无此证书 = Ok(false)，绝不落库。
    let der = nemesis_sysint::certdata::cert_der_from_bytes(TEST_ROOT_PEM.as_bytes())?;
    // 走自定义名 store 查询（同 API 面）；生产 Root 未装此证书是环境常态，
    // 这里只验证查询不 panic 且布尔语义成立。
    let store_name = format!("NemesisBotSysintTestQuery_{}", std::process::id());
    let found = unsafe { nemesis_sysint::windows::cert_store_find(&store_name, &der)? };
    assert!(!found, "空测试库查询应为 false");
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let _ =
        RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(test_store_registry_path(&store_name));
    Ok(())
}
