//! B7 —— vm_bridge.rs 的 WSL 自动路由伴生测试（零内联纪律）。
//!
//! 覆盖：UTF-16LE 解码三形态 / `--list` 解析 / 工具探测解析与分类 /
//! `/mnt` 路径翻译 / route 六态分类。真 spawn 探测只做环境无关断言
//! （落在四态之一——本机装没装 WSL、缺不缺工具都不该让测试红）。

use std::collections::BTreeMap;
use std::path::Path;

use super::vm_bridge::{
    ImageRouteState, WslProbe, WslState, classify_image_route, classify_wsl_tools,
    decode_wsl_output, is_distro_manifest, parse_tool_probe, parse_wsl_distro_list,
    windows_to_wsl_path,
};

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn tools_from(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn decode_wsl_output_with_bom_utf16le() {
    let raw = utf16le("Ubuntu-22.04\r\nDebian\r\n");
    assert_eq!(decode_wsl_output(&raw), "Ubuntu-22.04\r\nDebian\r\n");
}

#[test]
fn decode_wsl_output_bomless_utf16le_ascii() {
    // 旧版 wsl.exe 无 BOM 的 UTF-16LE 输出——靠奇数位 0 字节启发式认。
    let raw = utf16le("Ubuntu\r\n");
    assert_eq!(decode_wsl_output(&raw), "Ubuntu\r\n");
}

#[test]
fn decode_wsl_output_plain_utf8_passthrough() {
    let text = "qemu-img=yes\nfile=no\n";
    assert_eq!(decode_wsl_output(text.as_bytes()), text);
}

#[test]
fn distro_list_parses_names_and_skips_blanks() {
    assert_eq!(
        parse_wsl_distro_list("Ubuntu-22.04\r\ndebian-13\r\n\r\n"),
        vec!["Ubuntu-22.04".to_string(), "debian-13".to_string()]
    );
    assert!(parse_wsl_distro_list("").is_empty());
    assert!(parse_wsl_distro_list("\r\n  \n").is_empty());
}

#[test]
fn distro_manifest_rejects_error_message_shapes() {
    // 真清单：每行一个注册名（无空白）。
    let manifest = vec!["Ubuntu-22.04".to_string(), "debian-13".to_string()];
    assert!(is_distro_manifest(&manifest));
    // 空 = 非清单。
    assert!(!is_distro_manifest(&[]));
    // 本机实测形态：WSL 功能未启用时输出本地化错误消息（exit 仍为 0）
    // ——消息行含空白，与注册名判别。
    let msg = vec![
        "* 未安装用于 Linux 的 Windows 子系统。可通过运行 wsl.exe --install 进行安装。".to_string(),
    ];
    assert!(!is_distro_manifest(&msg));
    let msg_en = vec!["Windows Subsystem for Linux has no installed distributions.".to_string()];
    assert!(!is_distro_manifest(&msg_en));
}

#[test]
fn tool_probe_parses_yes_no_and_ignores_noise() {
    let tools =
        parse_tool_probe("qemu-img=yes\nfile=yes\ncurl=yes\nwget=no\napt-get=yes\n噪声行\n");
    assert_eq!(tools.get("qemu-img"), Some(&true));
    assert_eq!(tools.get("wget"), Some(&false));
    assert_eq!(tools.get("apt-get"), Some(&true));
    assert!(!tools.contains_key("噪声行"));
}

#[test]
fn classify_tools_all_present_means_ready() {
    let tools = tools_from(&[
        ("qemu-img", true),
        ("file", true),
        ("curl", true),
        ("wget", false),
        ("apt-get", true),
    ]);
    let (missing, has_apt) = classify_wsl_tools(&tools);
    assert!(missing.is_empty());
    assert!(has_apt);
}

#[test]
fn classify_tools_missing_qemu_img_and_file() {
    let tools = tools_from(&[("qemu-img", false), ("file", false), ("wget", true)]);
    let (missing, _) = classify_wsl_tools(&tools);
    // qemu-img 组 + file 各一条；下载器有 wget 满足不再报。
    assert_eq!(missing.len(), 2);
}

#[test]
fn classify_tools_missing_downloader_and_no_apt() {
    let tools = tools_from(&[("qemu-img", true), ("file", true)]);
    let (missing, has_apt) = classify_wsl_tools(&tools);
    assert_eq!(missing, vec!["curl 或 wget".to_string()]);
    assert!(!has_apt);
}

#[test]
fn wsl_path_translates_drive_letters() {
    assert_eq!(
        windows_to_wsl_path(Path::new("C:\\a\\b\\c.qcow2")).unwrap(),
        "/mnt/c/a/b/c.qcow2"
    );
    assert_eq!(
        windows_to_wsl_path(Path::new("D:/x/y")).unwrap(),
        "/mnt/d/x/y"
    );
    assert_eq!(
        windows_to_wsl_path(Path::new("c:\\root")).unwrap(),
        "/mnt/c/root"
    );
    // 盘符根（无子路径）不带尾分隔符。
    assert_eq!(windows_to_wsl_path(Path::new("C:\\")).unwrap(), "/mnt/c");
}

#[test]
fn wsl_path_rejects_unc_relative_and_empty() {
    assert!(windows_to_wsl_path(Path::new("\\\\server\\share\\x")).is_err());
    assert!(windows_to_wsl_path(Path::new("relative/path")).is_err());
    assert!(windows_to_wsl_path(Path::new("")).is_err());
}

#[test]
fn route_classify_all_branches() {
    let ready = WslProbe {
        state: WslState::Ready,
        missing: vec![],
        has_apt: true,
        reason: String::new(),
    };
    let no_distro = WslProbe {
        state: WslState::NoDistro,
        missing: vec![],
        has_apt: false,
        reason: String::new(),
    };
    let missing = WslProbe {
        state: WslState::MissingTools,
        missing: vec!["file".to_string()],
        has_apt: true,
        reason: String::new(),
    };
    let unavail = WslProbe {
        state: WslState::Unavailable,
        missing: vec![],
        has_apt: false,
        reason: "超时".to_string(),
    };

    // 脚本缺席优先于一切（含探针在场的 Windows 面）。
    assert_eq!(classify_image_route(false, None), ImageRouteState::NoScript);
    assert_eq!(
        classify_image_route(false, Some(&ready)),
        ImageRouteState::NoScript
    );
    // 非 Windows 探针缺席 = native。
    assert_eq!(classify_image_route(true, None), ImageRouteState::Native);
    assert_eq!(
        classify_image_route(true, Some(&ready)),
        ImageRouteState::WslReady
    );
    assert_eq!(
        classify_image_route(true, Some(&no_distro)),
        ImageRouteState::WslNoDistro
    );
    assert_eq!(
        classify_image_route(true, Some(&missing)),
        ImageRouteState::WslMissingTools
    );
    assert_eq!(
        classify_image_route(true, Some(&unavail)),
        ImageRouteState::WslUnavailable
    );
}

/// 真 spawn 探测落在四态之一（环境无关——WSL 装没装、缺不缺工具、
/// 甚至 wsl.exe 不在场，测试都不该红；只验「探针不 panic 且结论合法」）。
#[cfg(windows)]
#[test]
fn live_probe_lands_on_a_valid_state() {
    let probe = super::vm_bridge::wsl_spawn_probe();
    assert!(
        matches!(
            probe.state,
            WslState::Ready | WslState::NoDistro | WslState::MissingTools | WslState::Unavailable
        ),
        "probe state = {:?}",
        probe.state
    );
}

/// 镜像换装④臂收尾消息：恢复成功 = 诚实告知落点已恢复（不带手动命令）。
#[test]
fn promote_step4_failure_message_restore_ok() {
    let msg = super::vm_bridge::promote_step4_failure_message(
        Path::new("o:\\vm\\overlays\\a.qcow2"),
        Path::new("o:\\vm\\images\\base.qcow2"),
        None,
    );
    assert!(msg.contains("已自动恢复落点"), "msg = {msg}");
    assert!(msg.contains("该 overlay 可启动"), "msg = {msg}");
    assert!(!msg.contains("手动修复"), "msg = {msg}");
    assert!(msg.contains("重跑换装"), "msg = {msg}");
}

/// 镜像换装④臂收尾消息：恢复失败 = 带手动修复命令（rebase -u + 两路径
/// 在场）——修前文案「文件仍在场，手动转正」在 rename 后是空头指引
/// （base.new.qcow2 已转正不存在），回归锁：失败指引必须指向转正路径。
#[test]
fn promote_step4_failure_message_restore_failed_carries_manual_command() {
    let msg = super::vm_bridge::promote_step4_failure_message(
        Path::new("o:\\vm\\overlays\\a.qcow2"),
        Path::new("o:\\vm\\images\\base.qcow2"),
        Some("copy failed"),
    );
    assert!(msg.contains("落点自动恢复失败"), "msg = {msg}");
    assert!(msg.contains("手动修复"), "msg = {msg}");
    assert!(msg.contains("rebase -u"), "msg = {msg}");
    // 指引指向转正后的 canonical（在场），不是已不存在的 base.new.qcow2
    // （"base.new.qcow2" 不含子串 "base.qcow2"——该断言天然排除旧临时名）。
    assert!(msg.contains("o:\\vm\\images\\base.qcow2"), "msg = {msg}");
    assert!(msg.contains("o:\\vm\\overlays\\a.qcow2"), "msg = {msg}");
}

// ===========================================================================
// plugin-vm 单一依赖包查找链（2026-10-22 拍板：脚本 + guest 产物随
// plugin-vm 插件包分发，bot 不携带——查找顺序部署优先/开发回落的回归锁）。
// ===========================================================================
mod package_lookup {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::super::vm_bridge::{guest_bin_candidates_with, vm_image_script_path_with};

    /// 唯一临时 scratch 目录（按测试名 + pid 隔离，并行不互踩）。
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("vm_bridge_pkg_lookup_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).expect("mkdir scratch");
            Self(base)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn script_lookup_prefers_plugin_dir_then_dev_fallback() {
        let scratch = Scratch::new("script");
        let plugin_dir = scratch.path();
        let dev = scratch.path().join("dev").join("build-vm-image.sh");
        fs::create_dir_all(dev.parent().unwrap()).unwrap();
        // 开发形态脚本先物化（①b/② 的回落臂依赖它在场）。
        fs::write(&dev, b"#!/usr/bin/env bash\n").unwrap();

        // ① 包内脚本在场 → 命中包路径（部署形态优先）。
        let in_pkg = plugin_dir.join("build-vm-image.sh");
        fs::write(&in_pkg, b"#!/usr/bin/env bash\n").unwrap();
        assert_eq!(
            vm_image_script_path_with(Some(plugin_dir), &dev),
            Some(in_pkg.clone())
        );
        // ①b 无 exe 目录语义（plugin_dir=None）→ 直接走开发回落。
        assert_eq!(vm_image_script_path_with(None, &dev), Some(dev.clone()));

        // ② 包内缺席、开发形态在场 → 回落 dev 路径。
        fs::remove_file(&in_pkg).unwrap();
        assert_eq!(
            vm_image_script_path_with(Some(plugin_dir), &dev),
            Some(dev.clone())
        );

        // ③ 双双缺席 → None（no_script 态的来源）。
        fs::remove_file(&dev).unwrap();
        assert_eq!(vm_image_script_path_with(Some(plugin_dir), &dev), None);
        assert_eq!(vm_image_script_path_with(None, &dev), None);
    }

    #[test]
    fn guest_candidates_order_package_first_then_dev_target() {
        let scratch = Scratch::new("guest");
        let plugin_dir = scratch.path();

        // ① 部署形态候选在先：plugin_vm/guest/<短arch>/（与 build-guest.sh 的
        //    bin/bin_guest/<短arch>/ 布局同构，打包直拷），开发形态候选垫后
        //    （target triple 形态）。
        assert_eq!(
            guest_bin_candidates_with(Some(plugin_dir), "x86_64").unwrap(),
            vec![
                plugin_dir
                    .join("guest")
                    .join("x86_64")
                    .join("nemesis-guest"),
                PathBuf::from("target")
                    .join("x86_64-unknown-linux-musl")
                    .join("release")
                    .join("nemesis-guest"),
            ]
        );
        // ② aarch64 同构：短 arch 进包路径，triple 进 dev 路径。
        assert_eq!(
            guest_bin_candidates_with(Some(plugin_dir), "aarch64").unwrap(),
            vec![
                plugin_dir
                    .join("guest")
                    .join("aarch64")
                    .join("nemesis-guest"),
                PathBuf::from("target")
                    .join("aarch64-unknown-linux-musl")
                    .join("release")
                    .join("nemesis-guest"),
            ]
        );
        // ③ 无 exe 目录语义 → 只剩开发形态候选。
        assert_eq!(
            guest_bin_candidates_with(None, "x86_64").unwrap(),
            vec![
                PathBuf::from("target")
                    .join("x86_64-unknown-linux-musl")
                    .join("release")
                    .join("nemesis-guest")
            ]
        );
    }

    #[test]
    fn guest_candidates_reject_unknown_arch() {
        let scratch = Scratch::new("arch");
        let err = guest_bin_candidates_with(Some(scratch.path()), "riscv64")
            .expect_err("非法 arch 必须 Err");
        assert!(err.contains("riscv64"), "err = {err}");
    }
}
