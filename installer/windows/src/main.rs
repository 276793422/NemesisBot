//! nbsetup-win — NemesisBot Windows 安装器（三形态，计划 §5）：
//!
//! | 入口 | 形态 |
//! |------|------|
//! | 双击安装包（无参数） | GUI 向导（egui） |
//! | `--silent` / NSIS 惯例 `/S` | 静默安装（stdout 摘要 + 退出码契约） |
//! | `--uninstall`（ARP 入口） | 静默卸载 |
//! | 双击 `uninstall.exe`（无参数） | GUI 卸载（确认对话框） |
//!
//! 安装包 = 本 exe 自身（NBSETUP1 布局：骨架 + payload + manifest + 尾注）；
//! `--package <file>` 为开发/测试覆盖入口。
//!
//! 子系统：release = GUI（windows_subsystem = "windows"）——双击零参数
//! **无控制台黑框**（2026-11-10 用户诉求）；命令行入口在 main 最前面
//! AttachConsole 回接父控制台（重定向形态不需要——句柄经 STARTUPINFO
//! 直传），debug 构建保持控制台子系统（开发态 println 可见）。
//!
//! 退出码：0 成功 / 1 通用（含降级被拒）/ 2 完整性 / 3 取消 / 4 磁盘不足
//! / 5 签名风险未确认 / 6 bot 正在运行（ops::exit_code_of 单点映射）。

// GUI 子系统（仅 release）：双击无黑框。必须放在所有 item 之前。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;
mod ops;

use std::path::PathBuf;

/// 解析后的命令行。
#[derive(Debug, Default, PartialEq)]
struct Args {
    silent: bool,
    uninstall: bool,
    accept_risk: bool,
    force: bool,
    dir: Option<PathBuf>,
    desktop_shortcut: bool,
    no_shortcuts: bool,
    autostart: bool,
    launch: bool,
    no_taiwei: bool,
    precise_location: bool,
    package: Option<PathBuf>,
    help: bool,
}

/// 纯解析（测试锚点）；`/S` 大小写不敏感预变换为 silent。
fn parse_from(argv: &[String]) -> Args {
    let mut args = Args::default();
    let mut positional: Vec<String> = Vec::new();
    for raw in argv {
        let a = raw.as_str();
        if a.eq_ignore_ascii_case("/S") {
            args.silent = true;
            continue;
        }
        match a {
            "--silent" => args.silent = true,
            "--uninstall" => args.uninstall = true,
            "--accept-signature-risk" => args.accept_risk = true,
            "--force" => args.force = true,
            "--desktop-shortcut" => args.desktop_shortcut = true,
            "--no-shortcuts" => args.no_shortcuts = true,
            "--autostart" => args.autostart = true,
            "--launch" => args.launch = true,
            "--no-taiwei" => args.no_taiwei = true,
            "--precise-location" => args.precise_location = true,
            "--help" | "-h" => args.help = true,
            _ => {
                if let Some(v) = a.strip_prefix("--dir=") {
                    args.dir = Some(PathBuf::from(v));
                } else if let Some(v) = a.strip_prefix("--package=") {
                    args.package = Some(PathBuf::from(v));
                } else if a == "--dir" || a == "--package" {
                    // 后续由 positional 机制消费——标记形式不放这里，
                    // 直接报错形态（帮助信息）由 help 文档说明 = 值形态。
                    positional.push(a.to_string());
                } else if !a.starts_with('-') {
                    positional.push(a.to_string());
                } else {
                    eprintln!("[WARN] 未知参数（忽略）: {a}");
                }
            }
        }
    }
    // 位置参数语义：--dir --package 两个标志令牌后跟值（顺序消费）。
    let mut it = positional.into_iter();
    while let Some(tok) = it.next() {
        match tok.as_str() {
            "--dir" => {
                if let Some(v) = it.next() {
                    args.dir = Some(PathBuf::from(v));
                }
            }
            "--package" => {
                if let Some(v) = it.next() {
                    args.package = Some(PathBuf::from(v));
                }
            }
            _ => {}
        }
    }
    args
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    parse_from(&argv)
}

const HELP: &str = "\
nbsetup-win — NemesisBot Windows 安装器

用法:
  nbsetup-win.exe                      GUI 安装向导
  nbsetup-win.exe --silent [选项]      静默安装
  nbsetup-win.exe --uninstall          静默卸载（卸载副本所在目录）
  nbsetup-win.exe --uninstall --dir D  卸载指定目录

静默安装选项:
  --dir <DIR>                  安装目录（缺省 %LOCALAPPDATA%\\NemesisBot）
  --desktop-shortcut           创建桌面快捷方式（开始菜单恒建）
  --no-shortcuts               不创建任何快捷方式
  --autostart                  开机自启动（HKCU Run）
  --launch                     安装后启动 Dashboard
  --no-taiwei               关闭匿名使用统计（缺省开启；勾选页可随时改）
  --precise-location           开启精确定位（缺省关闭；单独同意 opt-in）
  --accept-signature-risk      签名异常（红色级）时知情继续（缺 = 退出码 5）
  --force                      允许版本降级
  --package <FILE>             安装包路径（缺省 = 本 exe 自身）

退出码: 0 成功 / 1 通用失败 / 2 完整性 / 3 取消 / 4 磁盘不足 / 5 签名风险未确认 / 6 bot 正在运行
";

fn main() {
    // GUI 子系统下回接父控制台（双击 = 父无控制台，静默失败无害；
    // --silent/--uninstall 从控制台启动 = stdout/stderr 落回原终端）。
    // 必须先于任何 println!——Rust std 首次用 stdout 才取句柄，attach
    // 后取到的才是控制台句柄。
    setup_core::console::attach_parent_console();
    // 控制台 UTF-8（中文系统 GBK 控制台会把 UTF-8 stdout 打成乱码）。
    setup_core::console::set_utf8_codepage();
    let args = parse_args();
    if args.help {
        print!("{HELP}");
        return;
    }
    let package = args
        .package
        .clone()
        .unwrap_or_else(|| std::env::current_exe().expect("定位自身失败"));

    // 卸载模式判定：显式 --uninstall > 自身目录带 install.json（uninstall
    // 副本双击启发式）> 安装模式。
    let is_uninstall = args.uninstall || ops::own_dir_has_install_record();

    let code = if is_uninstall {
        let dir = args.dir.clone().map_or_else(
            || ops::own_install_dir().expect("定位安装目录失败"),
            PathBuf::from,
        );
        if args.silent {
            match ops::run_uninstall(&dir) {
                Ok(o) => {
                    println!(
                        "卸载完成: 删除 {} 个文件{}",
                        o.deleted,
                        if o.self_delete_scheduled {
                            "（卸载器将自行清理）"
                        } else {
                            ""
                        }
                    );
                    for w in &o.warnings {
                        eprintln!("[WARN] {w}");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("[FAIL] {e:#}");
                    ops::exit_code_of(&e)
                }
            }
        } else {
            gui::run_uninstall_wizard(&dir)
        }
    } else {
        let target = args.dir.clone().map_or_else(
            || ops::default_install_dir().expect("定位缺省安装目录失败"),
            PathBuf::from,
        );
        if args.silent {
            let opts = ops::InstallOptions {
                package,
                target_dir: target,
                accept_risk: args.accept_risk,
                force_downgrade: args.force,
                desktop_shortcut: args.desktop_shortcut,
                no_shortcuts: args.no_shortcuts,
                autostart: args.autostart,
                launch: args.launch,
                taiwei_opt_in: !args.no_taiwei,
                precise_opt_in: args.precise_location,
            };
            match ops::run_install(&opts, None) {
                Ok(o) => {
                    println!(
                        "安装完成: {}（{}）— {} 个文件 → {}",
                        o.bot_version,
                        o.plan_text,
                        o.files_installed,
                        o.install_dir.display()
                    );
                    if o.launched {
                        println!("Dashboard 已启动");
                    }
                    for w in &o.warnings {
                        eprintln!("[WARN] {w}");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("[FAIL] {e:#}");
                    ops::exit_code_of(&e)
                }
            }
        } else {
            gui::run_install_wizard(package, target)
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn slash_s_transforms_to_silent_case_insensitive() {
        // NSIS 惯例 /S = 精确两字符令牌，大小写不敏感；非精确令牌不触发。
        assert!(parse_from(&s(&["/S"])).silent);
        assert!(parse_from(&s(&["/s"])).silent);
        assert!(!parse_from(&s(&["/sx"])).silent, "非精确 /S 令牌不触发");
        assert!(
            !parse_from(&s(&["/sILENT"])).silent,
            "非精确 /S 令牌不触发（/S 不是 /silent 缩写）"
        );
        assert!(!parse_from(&s(&[])).silent);
    }

    #[test]
    fn flag_matrix_parses() {
        let a = parse_from(&s(&[
            "--silent",
            "--uninstall",
            "--accept-signature-risk",
            "--force",
            "--desktop-shortcut",
            "--no-shortcuts",
            "--autostart",
            "--launch",
            "--no-taiwei",
            "--precise-location",
        ]));
        assert!(a.silent && a.uninstall && a.accept_risk && a.force);
        assert!(a.desktop_shortcut && a.no_shortcuts && a.autostart && a.launch);
        assert!(a.no_taiwei, "--no-taiwei 旗标解析生效");
        assert!(a.precise_location, "--precise-location 旗标解析生效");
    }

    #[test]
    fn dir_package_value_forms() {
        let a = parse_from(&s(&["--dir", "C:/x y", "--package=C:/pkg.exe"]));
        assert_eq!(a.dir, Some(PathBuf::from("C:/x y")));
        assert_eq!(a.package, Some(PathBuf::from("C:/pkg.exe")));
        // = 值形态。
        let a2 = parse_from(&s(&["--dir=C:/z"]));
        assert_eq!(a2.dir, Some(PathBuf::from("C:/z")));
    }

    #[test]
    fn unknown_and_flaglike_tokens_ignored_with_warning() {
        // 未知长参数警告忽略；未知单横线同理；位置参数只消费 --dir/--package。
        let a = parse_from(&s(&["--wat", "-x", "C:/loose"]));
        assert!(a.dir.is_none() && a.package.is_none());
    }

    #[test]
    fn defaults_are_empty() {
        assert_eq!(parse_from(&s(&[])), Args::default());
    }
}
