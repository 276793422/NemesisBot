//! 运营链测试。
//!
//! 纪律：
//! - 注册表用例只碰测试键（`Software\NemesisBotSetupTest_*`，用后即清），
//!   绝不写真实 ARP/Run 键；
//! - 快捷方式用例建到 tempfile 目录（不碰用户桌面/开始菜单）；
//! - `running_bots` 断言形状不断言空（本机可能有真实实例在跑）。

use std::path::Path;

use anyhow::Result;
use windows_sys::Win32::Storage::FileSystem as wfs;

use super::*;

// ---------- 退出码映射 ----------

#[test]
fn exit_code_maps_typed_and_ops_failures() {
    let integrity: anyhow::Error = TypedFail::Integrity("x".into()).into();
    assert_eq!(exit_code_of(&integrity), 2);
    let risk: anyhow::Error = TypedFail::SigRisk("x".into()).into();
    assert_eq!(exit_code_of(&risk), 5);
    let downgrade: anyhow::Error = TypedFail::DowngradeRejected {
        current: "2".into(),
        incoming: "1".into(),
    }
    .into();
    assert_eq!(exit_code_of(&downgrade), 1);
    let running: anyhow::Error = OpsFail::Running(vec!["pid".into()]).into();
    assert_eq!(exit_code_of(&running), 6);
    let disk: anyhow::Error = OpsFail::DiskSpace { needed: 1, free: 0 }.into();
    assert_eq!(exit_code_of(&disk), 4);
    let cancelled: anyhow::Error = OpsFail::Cancelled.into();
    assert_eq!(exit_code_of(&cancelled), 3);
    // 泛化 anyhow = 通用失败。
    let generic: anyhow::Error = anyhow!("任意失败");
    assert_eq!(exit_code_of(&generic), 1);
}

// ---------- 磁盘预检 ----------

#[test]
fn required_disk_bytes_adds_margin() {
    assert_eq!(required_disk_bytes(0), 0);
    assert_eq!(required_disk_bytes(100), 120);
    assert_eq!(required_disk_bytes(1_000_000), 1_200_000);
}

#[test]
fn free_disk_probes_existing_ancestor() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    // 目录本体存在 → 直接可查。
    let free = free_disk_bytes(tmp.path())?;
    // 深层不存在目录 → 沿祖先查到 tmp（同卷）。
    let deep = tmp.path().join("a/b/c");
    let free2 = free_disk_bytes(&deep)?;
    assert!(free > 0);
    assert_eq!(free2, free, "同卷祖先查询应同一剩余量");
    Ok(())
}

// ---------- PowerShell 引号转义 ----------

#[test]
fn ps_quote_escapes_single_quotes() {
    assert_eq!(ps_quote("plain"), "'plain'");
    assert_eq!(ps_quote("it's"), "'it''s'");
    assert_eq!(ps_quote(""), "''");
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

// ---------- 注册表（测试键，用后即清） ----------

#[test]
fn registry_arp_and_run_roundtrip() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let pid = std::process::id();
    let arp_test = format!("Software\\NemesisBotSetupTest_{pid}\\Uninstall");
    let run_test = format!("Software\\NemesisBotSetupTest_{pid}\\Run");

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

    let run_key = hkcu.open_subkey_with_flags(&run_test, KEY_READ)?;
    let run_value: String = run_key.get_value(APP_NAME)?;
    assert!(
        run_value.contains("dashboard"),
        "Run 值带 dashboard 参数: {run_value}"
    );

    // 清理 + 幂等。
    remove_arp_at(&arp_test)?;
    remove_arp_at(&arp_test)?;
    remove_autostart_at(&run_test)?;
    let root = format!("Software\\NemesisBotSetupTest_{pid}");
    hkcu.delete_subkey_all(&root)?;
    Ok(())
}

// ---------- 进程检测（形状断言） ----------

#[test]
fn running_bots_returns_shape_not_emptiness() -> Result<()> {
    // 本机可能有真实实例在跑（用户自管进程）——只断言成功 + 行格式。
    let list = running_bots()?;
    for line in &list {
        assert!(
            line.to_ascii_lowercase().contains("nemesisbot.exe"),
            "过滤后的行应含镜像名: {line}"
        );
    }
    Ok(())
}

// ---------- 目录定位 ----------

#[test]
fn default_install_dir_under_localappdata() {
    let dir = default_install_dir().expect("本机必有 LOCALAPPDATA/USERPROFILE");
    let s = dir.to_string_lossy();
    assert!(
        s.ends_with("\\NemesisBot"),
        "缺省安装目录以 NemesisBot 收尾: {s}"
    );
    assert!(Path::new(&*s).is_absolute(), "缺省目录绝对路径: {s}");
}

// ---------- Windows API 存活性探针（编译期 feature 闸的直接证据） ----------

#[test]
fn get_disk_free_space_exw_links_and_runs() {
    // windows-sys 0.59 直接 *mut u64（非 ULARGE_INTEGER 结构）。
    let mut free: u64 = 0;
    let mut total: u64 = 0;
    let mut caller: u64 = 0;
    let root: Vec<u16> = "C:\\\0".encode_utf16().collect();
    let ok = unsafe { wfs::GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut total, &mut caller) };
    assert_eq!(ok, 1, "C 盘查询应成功");
    assert!(total > 0);
}

// ---------- 假程序模拟（离线全生命周期：进程检测/ARP 键注入） ----------

use setup_core::pack;

/// 假 tasklist 输出（表头 + 两条 nemesisbot.exe + 噪声行）。
const FAKE_TASKLIST: &str = "\
映像名称                       PID 会话名              会话#       内存使用
========================= ======== ================ =========== ============
nemesisbot.exe               4242 Console                    1     66,808 K
notepad.exe                  1234 Console                    1      1,024 K
nemesisbot.exe               5151 RDP-Tcp#0                  2     91,232 K
chrome.exe                   9999 Console                    1    512,000 K";

#[test]
fn parse_tasklist_filters_fake_process_list() {
    let got = parse_tasklist_output(FAKE_TASKLIST);
    assert_eq!(got.len(), 2, "只留 nemesisbot.exe 行: {got:?}");
    assert!(got.iter().all(|l| l.contains("nemesisbot.exe")));
    // 空输出 / 无匹配形态 → 空清单。
    assert!(parse_tasklist_output("").is_empty());
    assert!(parse_tasklist_output("INFO: 没有运行的任务匹配指定标准。").is_empty());
}

/// 打一个假包：假载荷（假 nemesisbot.exe 字节 + 附属文件）+ 假骨架字节
/// （引擎对骨架只搬运不执行——uninstall.exe 副本 = 假字节，测试里永不运行）。
fn build_fake_package(dir: &Path, version: &str) -> Result<std::path::PathBuf> {
    let payload = dir.join("payload");
    std::fs::create_dir_all(payload.join("share"))?;
    std::fs::write(payload.join("nemesisbot.exe"), b"FAKE-BOT-BYTES")?;
    std::fs::write(payload.join("share/icon.png"), b"png")?;
    std::fs::write(payload.join("IDENTITY.md"), b"# fake identity")?;
    let skeleton = dir.join("skl.bin");
    std::fs::write(&skeleton, b"SKEL")?;
    let out = dir.join("sim.pkg");
    pack::run(&pack::PackOptions {
        payload_dir: payload,
        skeleton,
        out: out.clone(),
        bot_version: version.into(),
        platform: "windows".into(),
        sign_keys: None,
        zstd_level: 1,
    })?;
    Ok(out)
}

/// 模拟安装参数（不碰用户机器面：无快捷方式 / 无自启 / 不启动假 bot）。
fn sim_install_options(package: &Path, target: &Path) -> InstallOptions {
    InstallOptions {
        package: package.to_path_buf(),
        target_dir: target.to_path_buf(),
        accept_risk: false,
        force_downgrade: false,
        desktop_shortcut: false,
        no_shortcuts: true,
        autostart: false,
        launch: false,
    }
}

#[test]
fn simulated_full_lifecycle_with_fake_process_list() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    let target = tmp.path().join("root");
    let pid = std::process::id();
    let arp_test = format!("Software\\NemesisBotSetupTest_{pid}\\SimUninstall");
    let opts = sim_install_options(&package, &target);
    // 注入的假进程清单——不 spawn 任何真进程，机器上有无实例在跑都无关。
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let fake_bots = || -> Result<Vec<String>> {
        Ok(vec![
            "nemesisbot.exe               4242 Console   1   66,808 K".into(),
        ])
    };

    // ① 模拟「无实例」→ 全新安装成功（假载荷逐文件落位 + 卸载副本 + ARP 测试键）。
    let out = run_install_with(&opts, None, &no_bots, &arp_test)?;
    assert_eq!(out.bot_version, "9.9.9");
    assert!(out.plan_text.contains("全新安装"), "{}", out.plan_text);
    assert_eq!(
        out.files_installed, 5,
        "3 载荷文件 + 卸载副本 + install.json"
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert!(target.join("nemesisbot.exe").exists());
    assert!(target.join("share/icon.png").exists());
    assert!(target.join("IDENTITY.md").exists());
    assert!(target.join("uninstall.exe").exists(), "卸载副本落位");
    assert!(target.join("install.json").exists());

    // ② 模拟「有实例」→ 闸拦 exit 6，且闸在引擎之前——安装目录一字节不动。
    let record_before = std::fs::read(target.join("install.json"))?;
    let err =
        run_install_with(&opts, None, &fake_bots, &arp_test).expect_err("假实例清单必须被闸拦");
    assert_eq!(exit_code_of(&err), 6);
    let record_after = std::fs::read(target.join("install.json"))?;
    assert_eq!(record_before, record_after, "闸在引擎之前，目录不被触碰");

    // ③ 模拟「无实例」→ 卸载干净（record.files 载荷清空、标记摘除、空目录剪除；
    //    卸载副本 = 卸载器自身职责——生产走延迟自删，测试宿主不是
    //    uninstall.exe → 副本诚实残留）。
    let un = run_uninstall_with(&target, &no_bots, &arp_test)?;
    assert_eq!(un.deleted, 3, "record.files = 3 个载荷文件");
    assert!(
        !un.self_delete_scheduled,
        "测试二进制不是 uninstall.exe，不排自删"
    );
    assert!(!target.join("install.json").exists(), "标记摘除");
    assert!(!target.join("nemesisbot.exe").exists());
    assert!(!target.join("share").exists(), "空目录剪除");
    assert!(
        target.join("uninstall.exe").exists(),
        "副本残留 = 设计内（生产由延迟自删脚本接手）"
    );

    // ④ 模拟「有实例」→ 卸载同样被拦（闸先于 install.json 缺失检查——exit 6）。
    let err = run_uninstall_with(&target, &fake_bots, &arp_test).expect_err("卸载也要过实例闸");
    assert_eq!(exit_code_of(&err), 6);

    // 注册表测试键收尾（best-effort——失败路径可能从未写入）。
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let _ = RegKey::predef(HKEY_CURRENT_USER)
        .delete_subkey_all(format!("Software\\NemesisBotSetupTest_{pid}"));
    Ok(())
}

#[test]
fn simulated_corrupted_payload_fails_closed_exit2() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let package = build_fake_package(tmp.path(), "9.9.9")?;
    // 翻转 payload 区首字节（释放前 sha256 硬闸必咬）。
    let info = pkgfile::inspect(&package)?;
    let off = info.manifest.payload_offset;
    {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&package)?;
        f.seek(SeekFrom::Start(off))?;
        let mut b = [0u8; 1];
        f.read_exact(&mut b)?;
        b[0] ^= 0xFF;
        f.seek(SeekFrom::Start(off))?;
        f.write_all(&b)?;
    }
    let target = tmp.path().join("root");
    let opts = sim_install_options(&package, &target);
    let no_bots = || -> Result<Vec<String>> { Ok(vec![]) };
    let pid = std::process::id();
    let arp_test = format!("Software\\NemesisBotSetupTest_{pid}\\SimIntegrity");
    let err = run_install_with(&opts, None, &no_bots, &arp_test).expect_err("载荷被改必须硬失败");
    assert_eq!(
        exit_code_of(&err),
        2,
        "完整性域退出码 2（非信任决策，不可知情越过）"
    );
    assert!(
        matches!(
            err.downcast_ref::<TypedFail>(),
            Some(TypedFail::Integrity(_))
        ),
        "类型化完整性失败: {err:#}"
    );
    // fail-closed：目标目录不落任何 bot 文件（staging 已清理）。
    assert!(!target.join("nemesisbot.exe").exists());
    // 注册表测试键收尾（安装失败路径不写 ARP；best-effort）。
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let _ = RegKey::predef(HKEY_CURRENT_USER)
        .delete_subkey_all(format!("Software\\NemesisBotSetupTest_{pid}"));
    Ok(())
}

// ---------- 自删脚本串（纯函数，回归锁：raw_arg 引号纪律） ----------

#[test]
fn self_delete_script_quotes_path_without_escape_artifacts() {
    let p = Path::new(r"C:\Program Files\NemesisBot\uninstall.exe");
    let s = self_delete_script(p);
    assert!(
        s.starts_with(r#"ping -n 3 127.0.0.1 >nul & del /f /q "C:\Program Files"#),
        "del 段引号包裹含空格路径: {s}"
    );
    assert!(
        s.contains(r#"" & rd "C:\Program Files\NemesisBot""#),
        "rd 段引号包裹目录: {s}"
    );
    // 回归锁（2026-11-10 实装测试真 bug）：脚本串经 raw_arg 原样上命令行，
    // 串内不得出现 Rust/MSVCRT 转义形态 `\"`——cmd 收到即字面反斜杠引号
    // = del/rd「文件名、目录名或卷标语法不正确」且 CREATE_NO_WINDOW 无声吞错。
    assert!(
        !s.contains("\\\""),
        "串内出现 \\\" 转义形态（cmd 语法错误根因）: {s}"
    );
    assert!(s.ends_with('"'), "rd 目录收尾引号闭合: {s}");
}
