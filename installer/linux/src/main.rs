//! nbsetup-linux — NemesisBot Linux 安装器（三形态，计划 §5）：
//!
//! | 入口 | 形态 |
//! |------|------|
//! | 直接运行（无参数） | 终端问答向导（缺省值直接回车） |
//! | `--silent` | 静默安装（stdout 摘要 + 退出码契约） |
//! | `--uninstall` | 静默卸载（安装目录内 `uninstall` 副本同形态） |
//!
//! 安装包 = 本可执行文件自身（NBSETUP1 布局）；`--package <file>` 开发覆盖。
//! 非 Linux 平台编译为本形态 stub main（诚实占位——纯内容构造器测试仍
//! 跨平台可跑）。
//!
//! 退出码：0 成功 / 1 通用（含降级被拒）/ 2 完整性 / 3 取消 / 4 磁盘不足
//! / 5 签名风险未确认 / 6 bot 正在运行（ops::exit_code_of 单点映射）。

// 非 Linux 编译 = stub 形态：纯内容构造器与解析器保留可测，真实流程
// 面（cfg(linux)）缺席 → 运营链函数在 stub 构建里自然不可达。压制
// dead-code 是本 crate 平台边界的显式声明（Linux 构建零压制）。
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod ops;

use std::path::PathBuf;

/// 解析后的命令行（与 windows 形态同旗标面——跨平台一致性）。
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
    package: Option<PathBuf>,
    help: bool,
}

/// 纯解析（测试锚点；`--desktop-shortcut` 在 Linux 是 no-op 接受——旗标面
/// 跨平台一致，语义差异由 README 注记）。
fn parse_from(argv: &[String]) -> Args {
    let mut args = Args::default();
    let mut positional: Vec<String> = Vec::new();
    for raw in argv {
        let a = raw.as_str();
        match a {
            "--silent" => args.silent = true,
            "--uninstall" => args.uninstall = true,
            "--accept-signature-risk" => args.accept_risk = true,
            "--force" => args.force = true,
            "--desktop-shortcut" => args.desktop_shortcut = true,
            "--no-shortcuts" => args.no_shortcuts = true,
            "--autostart" => args.autostart = true,
            "--launch" => args.launch = true,
            "--help" | "-h" => args.help = true,
            _ => {
                if let Some(v) = a.strip_prefix("--dir=") {
                    args.dir = Some(PathBuf::from(v));
                } else if let Some(v) = a.strip_prefix("--package=") {
                    args.package = Some(PathBuf::from(v));
                } else if a == "--dir" || a == "--package" {
                    // 裸标志令牌入 positional 队列，值由顺序消费。
                    positional.push(a.to_string());
                } else if !a.starts_with('-') {
                    positional.push(a.to_string());
                } else {
                    eprintln!("[WARN] 未知参数（忽略）: {a}");
                }
            }
        }
    }
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
nbsetup-linux — NemesisBot Linux 安装器

用法:
  nbsetup-linux                        终端安装向导（缺省值回车确认）
  nbsetup-linux --silent [选项]        静默安装
  nbsetup-linux --uninstall            静默卸载（安装目录内运行）
  nbsetup-linux --uninstall --dir D    卸载指定目录

静默安装选项:
  --dir <DIR>                  安装目录（缺省 ~/.local/share/NemesisBot，尊
                               重 XDG_DATA_HOME）
  --autostart                  开机自启（systemd user unit；systemctl 缺席
                               时 WARN 跳过）
  --launch                     安装后启动 Dashboard
  --accept-signature-risk      签名异常（红色级）时知情继续（缺 = 退出码 5）
  --force                      允许版本降级
  --no-shortcuts               不建 ~/.local/bin 符号链接 / .desktop / PATH
                               注记
  --desktop-shortcut           （接受；Linux 形态 .desktop 恒建，此旗标为
                               跨平台旗标面一致性保留）
  --package <FILE>             安装包路径（缺省 = 本可执行文件自身）

退出码: 0 成功 / 1 通用失败 / 2 完整性 / 3 取消 / 4 磁盘不足 / 5 签名风险未确认 / 6 bot 正在运行
";

fn main() {
    #[cfg(target_os = "linux")]
    {
        real_main();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = parse_args(); // 消费 args 使 parse_from 的测试路径与运行路径同源
        eprintln!("nbsetup-linux：Linux 专用安装器（当前平台占位 stub）");
        std::process::exit(1);
    }
}

/// 真实主流程（Linux 专属）。
#[cfg(target_os = "linux")]
fn real_main() {
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
    // 副本直接运行启发式）> 安装模式。
    let is_uninstall = args.uninstall || ops::own_dir_has_install_record();

    let code = if is_uninstall {
        let dir = args
            .dir
            .clone()
            .unwrap_or_else(|| ops::own_install_dir().expect("定位安装目录失败"));
        if args.silent {
            silent_uninstall(&dir)
        } else {
            wizard_uninstall(&dir)
        }
    } else {
        let target = args
            .dir
            .clone()
            .unwrap_or_else(|| ops::default_install_dir().expect("定位缺省安装目录失败"));
        if args.silent {
            silent_install(&package, target, &args)
        } else {
            wizard_install(&package, target, &args)
        }
    };
    std::process::exit(code);
}

/// 静默安装。
#[cfg(target_os = "linux")]
fn silent_install(package: &std::path::Path, target: PathBuf, args: &Args) -> i32 {
    let opts = ops::InstallOptions {
        package: package.to_path_buf(),
        target_dir: target,
        accept_risk: args.accept_risk,
        force_downgrade: args.force,
        no_shortcuts: args.no_shortcuts,
        autostart: args.autostart,
        launch: args.launch,
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
            for n in &o.notes {
                eprintln!("[NOTE] {n}");
            }
            0
        }
        Err(e) => {
            eprintln!("[FAIL] {e:#}");
            ops::exit_code_of(&e)
        }
    }
}

/// 静默卸载。
#[cfg(target_os = "linux")]
fn silent_uninstall(dir: &std::path::Path) -> i32 {
    match ops::run_uninstall(dir) {
        Ok(o) => {
            println!("卸载完成: 删除 {} 个文件", o.deleted);
            for f in &o.failures {
                eprintln!("[WARN] 删除失败 {}: {}", f.0, f.1);
            }
            for n in &o.notes {
                eprintln!("[NOTE] {n}");
            }
            0
        }
        Err(e) => {
            eprintln!("[FAIL] {e:#}");
            ops::exit_code_of(&e)
        }
    }
}

/// 终端问一句（EOF/stdin 关闭 = None → 取消语义）。
#[cfg(target_os = "linux")]
fn ask(prompt: &str, default_yes: bool) -> Option<bool> {
    print!("{prompt}");
    use std::io::Write;
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    let t = line.trim().to_ascii_lowercase();
    Some(match t.as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    })
}

/// 问目录行（空 = 缺省）。
#[cfg(target_os = "linux")]
fn ask_dir(default: &std::path::Path) -> Option<PathBuf> {
    print!("安装目录 [{}]: ", default.display());
    use std::io::Write;
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    let t = line.trim();
    Some(if t.is_empty() {
        default.to_path_buf()
    } else {
        PathBuf::from(t)
    })
}

/// 终端安装向导。
#[cfg(target_os = "linux")]
fn wizard_install(package: &std::path::Path, default_dir: PathBuf, args: &Args) -> i32 {
    let info = match ops::inspect_package(package) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[FAIL] 安装包无法读取: {e:#}");
            return ops::exit_code_of(&e);
        }
    };
    println!("NemesisBot 安装向导");
    println!("  版本: {}", info.manifest.bot_version);
    println!(
        "  载荷: {} 个文件（安装约需 {}）",
        info.manifest.files.len(),
        fmt_mb(ops::required_disk_bytes(info.manifest.total_size))
    );
    // 签名分级横幅（§4a：绿静默 / 黄提示 / 红警示 + 知情确认）。
    match info.sig.grade {
        setup_core::sig::SigGrade::Green => println!("  签名: 有效"),
        setup_core::sig::SigGrade::Yellow => {
            println!("  签名: 未签名安装包（开发构建常态），请确认来源可信")
        }
        setup_core::sig::SigGrade::Red => {
            println!(
                "  ⚠ 签名异常: {}（请勿信任来路不明的安装包）",
                info.sig.text
            );
        }
    }
    // 外壳自检行（§4a 第二条信任链：安装器本体防改）。
    let shell = setup_core::sig::shell_self_check();
    if shell.grade != setup_core::sig::SigGrade::Green {
        println!("  安装器本体: {}", shell.text);
    }

    let Some(dir) = ask_dir(&default_dir) else {
        eprintln!("[FAIL] 输入已关闭（非交互终端请用 --silent）");
        return 3;
    };
    let mut autostart = args.autostart;
    if !args.silent && !args.autostart {
        match ask("开机自启 (systemd user) [y/N]: ", false) {
            Some(v) => autostart = v,
            None => {
                eprintln!("[FAIL] 输入已关闭");
                return 3;
            }
        }
    }
    // 红色级知情确认（向导形态 = 显式 y；manifest 红或外壳红任一命中）。
    let accept_risk = if info.sig.grade == setup_core::sig::SigGrade::Red
        || shell.grade == setup_core::sig::SigGrade::Red
    {
        match ask("⚠ 签名异常——我已知晓风险仍要继续 [y/N]: ", false) {
            Some(true) => true,
            _ => {
                eprintln!("[FAIL] 未确认签名风险，已取消（退出码 5）");
                return 5;
            }
        }
    } else {
        args.accept_risk
    };

    match ask(&format!("确认安装到 {}? [Y/n]: ", dir.display()), true) {
        Some(true) => {}
        _ => {
            eprintln!("已取消");
            return 3;
        }
    }

    let opts = ops::InstallOptions {
        package: package.to_path_buf(),
        target_dir: dir,
        accept_risk,
        force_downgrade: args.force,
        no_shortcuts: args.no_shortcuts,
        autostart,
        launch: false,
    };
    let progress = |p: setup_core::pkgfile::Progress| match p {
        setup_core::pkgfile::Progress::Hashing(f) => {
            print!("\r校验载荷 {:.0}%", f * 100.0);
        }
        setup_core::pkgfile::Progress::Extracting(d, t) => {
            print!("\r释放文件 {d}/{t}          ");
        }
    };
    match ops::run_install(&opts, Some(&progress)) {
        Ok(o) => {
            println!();
            println!(
                "安装完成: {}（{}）— {} 个文件 → {}",
                o.bot_version,
                o.plan_text,
                o.files_installed,
                o.install_dir.display()
            );
            for n in &o.notes {
                eprintln!("[NOTE] {n}");
            }
            println!(
                "启动: {} dashboard 或应用菜单「NemesisBot」",
                o.install_dir.join("nemesisbot").display()
            );
            0
        }
        Err(e) => {
            println!();
            eprintln!("[FAIL] {e:#}");
            ops::exit_code_of(&e)
        }
    }
}

/// 终端卸载向导（uninstall 副本直接运行形态）。
#[cfg(target_os = "linux")]
fn wizard_uninstall(dir: &std::path::Path) -> i32 {
    println!("NemesisBot 卸载");
    println!("  安装目录: {}", dir.display());
    println!("  用户数据（~/.nemesisbot）保留");
    match ask("确定卸载? [y/N]: ", false) {
        Some(true) => {}
        _ => {
            eprintln!("已取消");
            return 3;
        }
    }
    silent_uninstall(dir)
}

/// 体积人读文本。
#[cfg(target_os = "linux")]
fn fmt_mb(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KB", bytes as f64 / 1_024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
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
        ]));
        assert!(a.silent && a.uninstall && a.accept_risk && a.force);
        assert!(a.desktop_shortcut && a.no_shortcuts && a.autostart && a.launch);
    }

    #[test]
    fn dir_package_value_forms() {
        let a = parse_from(&s(&["--dir", "/opt/x y", "--package=/tmp/p.run"]));
        assert_eq!(a.dir, Some(PathBuf::from("/opt/x y")));
        assert_eq!(a.package, Some(PathBuf::from("/tmp/p.run")));
        let a2 = parse_from(&s(&["--dir=/opt/z"]));
        assert_eq!(a2.dir, Some(PathBuf::from("/opt/z")));
    }

    #[test]
    fn unknown_and_flaglike_tokens_ignored_with_warning() {
        let a = parse_from(&s(&["--wat", "-x", "/loose"]));
        assert!(a.dir.is_none() && a.package.is_none());
    }

    #[test]
    fn defaults_are_empty() {
        assert_eq!(parse_from(&s(&[])), Args::default());
    }

    #[test]
    fn help_flag_recognized() {
        assert!(parse_from(&s(&["--help"])).help);
        assert!(parse_from(&s(&["-h"])).help);
    }
}
