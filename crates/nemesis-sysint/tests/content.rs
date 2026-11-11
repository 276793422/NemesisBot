//! content 纯构造器测试（跨平台可跑——CI Windows 与远程 Linux 盒共用同一批断言）。
//!
//! 移植自 installer 两侧 ops/tests.rs（2026-11-10 系统集成面收编 bot）。

use std::path::Path;

use nemesis_sysint::content::{
    self, PATH_MARKER_BEGIN, PATH_MARKER_END, RegValue, arp_values, autostart_run_value,
    desktop_entry_text, lnk_script, needs_path_append, path_snippet, ps_quote, shell_quote,
    systemd_unit_text,
};

// ---------- ARP 内容 ----------

#[test]
fn arp_values_cover_uninstall_panel_contract() {
    let vals = arp_values(Path::new(r"C:\Apps\NemesisBot"), "1.2.3");
    let get =
        |name: &str| -> Option<&RegValue> { vals.iter().find(|(n, _)| *n == name).map(|(_, v)| v) };
    assert_eq!(
        get("DisplayName"),
        Some(&RegValue::Str("NemesisBot".into())),
        "产品名常量与内容层同源"
    );
    assert_eq!(get("DisplayVersion"), Some(&RegValue::Str("1.2.3".into())));
    // UninstallString 指向卸载副本 + --uninstall 旗标，引号包裹含空格路径。
    match get("UninstallString") {
        Some(RegValue::Str(s)) => {
            assert!(
                s.contains("uninstall.exe") && s.contains("--uninstall") && s.contains("\""),
                "UninstallString 契约: {s}"
            );
        }
        other => panic!("UninstallString 应为字符串: {other:?}"),
    }
    // 面板按钮语义为 dword。
    assert_eq!(get("NoModify"), Some(&RegValue::Dword(1)));
    assert_eq!(get("NoRepair"), Some(&RegValue::Dword(1)));
    assert_eq!(
        get("InstallLocation"),
        Some(&RegValue::Str(r"C:\Apps\NemesisBot".into()))
    );
}

// ---------- Run 自启值 ----------

#[test]
fn autostart_run_value_quotes_exe_with_dashboard() {
    let dir = Path::new(r"C:\Apps\NemesisBot");
    let v = autostart_run_value(dir);
    // exe 路径经 PathBuf::join 拼接——分隔符随宿主平台（Linux 盒跑出 `/`），
    // 期望值以同一 join 形态构造，钉住「引号包裹 + dashboard 参数」契约。
    let expected = format!("\"{}\" dashboard", dir.join("nemesisbot.exe").display());
    assert_eq!(v, expected);
}

// ---------- PowerShell / shell 引号 ----------

#[test]
fn ps_quote_escapes_single_quotes() {
    assert_eq!(ps_quote("plain"), "'plain'");
    assert_eq!(ps_quote("it's"), "'it''s'");
    assert_eq!(ps_quote(""), "''");
}

#[test]
fn shell_quote_escapes_single_quotes() {
    assert_eq!(shell_quote("plain"), "'plain'");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote(""), "''");
}

// ---------- .lnk 脚本 ----------

#[test]
fn lnk_script_quotes_all_four_params() {
    let s = lnk_script(
        Path::new(r"C:\Users\u\Desktop\NemesisBot.lnk"),
        Path::new(r"C:\Apps\NemesisBot\nemesisbot.exe"),
        "dashboard",
        Path::new(r"C:\Apps\NemesisBot"),
    );
    assert!(s.contains("WScript.Shell"), "走 WScript.Shell COM: {s}");
    assert!(s.contains("CreateShortcut("), "{s}");
    assert!(s.contains("$s.Save()"), "{s}");
    // 全参数单引号包裹（含空格路径安全）。
    assert!(s.contains(r"'C:\Users\u\Desktop\NemesisBot.lnk'"), "{s}");
    assert!(s.contains(r"'C:\Apps\NemesisBot\nemesisbot.exe'"), "{s}");
    assert!(s.contains("'dashboard'"), "{s}");
}

// ---------- Linux 三件套内容 ----------

#[test]
fn path_snippet_has_sentinels_for_idempotency() {
    let s = path_snippet();
    assert!(s.contains(PATH_MARKER_BEGIN));
    assert!(s.contains(PATH_MARKER_END));
    assert!(s.contains(".local/bin"));
    assert!(s.contains("export PATH"));
}

#[test]
fn needs_path_append_matrix() {
    let bin = "/home/u/.local/bin";
    // 已有哨兵 = 永不重复写。
    let with_marker = format!("foo\n{}\nbar", PATH_MARKER_BEGIN);
    assert!(!needs_path_append("", &with_marker, bin));
    // PATH 已精确含 .local/bin = 不需要。
    assert!(!needs_path_append("/usr/bin:/home/u/.local/bin", "", bin));
    // 缺席 = 需要。
    assert!(needs_path_append("/usr/bin:/bin", "", bin));
    assert!(needs_path_append("", "", bin));
    // 形近路径不算（/usr/.local/bin 不是 ~/.local/bin）。
    assert!(needs_path_append("/usr/.local/bin", "", bin));
    // 哨兵缺席但 PATH 缺席 → 需要。
    assert!(needs_path_append("/usr/bin", "echo hi", bin));
}

#[test]
fn desktop_entry_content_is_valid_xdg() {
    let text = desktop_entry_text(
        Path::new("/opt/NemesisBot/nemesisbot"),
        Path::new("/opt/NemesisBot/share/icon.png"),
    );
    assert!(text.starts_with("[Desktop Entry]\n"));
    assert!(text.contains("Type=Application"));
    assert!(text.contains("Name=NemesisBot"));
    // Exec 路径 shell 引号包裹（含空格路径安全），参数在引号外。
    assert!(
        text.contains("Exec='/opt/NemesisBot/nemesisbot' dashboard"),
        "Exec 引号包裹 + dashboard 参数: {text}"
    );
    assert!(text.contains("Icon=/opt/NemesisBot/share/icon.png"));
    assert!(text.contains("Terminal=false"));
}

#[test]
fn systemd_unit_content_is_valid() {
    let text = systemd_unit_text(Path::new("/opt/NemesisBot/nemesisbot"));
    assert!(text.contains("[Unit]"));
    assert!(text.contains("[Service]"));
    assert!(text.contains("[Install]"));
    assert!(
        text.contains("ExecStart='/opt/NemesisBot/nemesisbot' gateway"),
        "ExecStart 引号包裹 + gateway 参数: {text}"
    );
    assert!(text.contains("Restart=on-failure"));
    assert!(
        text.contains("WantedBy=default.target"),
        "user unit 用 default.target 非 multi-user"
    );
}

// ---------- 内容层常量单一性（产品名与快捷方式参数） ----------

#[test]
fn product_constants_are_stable() {
    assert_eq!(content::APP_NAME, "NemesisBot");
    assert_eq!(content::LNK_ARGUMENTS, "dashboard");
}
