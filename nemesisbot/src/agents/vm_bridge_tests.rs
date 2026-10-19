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
    let text = "genisoimage=yes\nfile=no\n";
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
    let tools = parse_tool_probe(
        "genisoimage=yes\nxorriso=no\nfile=yes\ncurl=yes\nwget=no\napt-get=yes\n噪声行\n",
    );
    assert_eq!(tools.get("genisoimage"), Some(&true));
    assert_eq!(tools.get("xorriso"), Some(&false));
    assert_eq!(tools.get("wget"), Some(&false));
    assert_eq!(tools.get("apt-get"), Some(&true));
    assert!(!tools.contains_key("噪声行"));
}

#[test]
fn classify_tools_all_present_means_ready() {
    let tools = tools_from(&[
        ("genisoimage", true),
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
fn classify_tools_missing_iso_and_file() {
    let tools = tools_from(&[
        ("xorriso", false),
        ("mkisofs", false),
        ("genisoimage", false),
        ("file", false),
        ("wget", true),
    ]);
    let (missing, _) = classify_wsl_tools(&tools);
    // ISO 组 + file 各一条；下载器有 wget 满足不再报。
    assert_eq!(missing.len(), 2);
}

#[test]
fn classify_tools_missing_downloader_and_no_apt() {
    let tools = tools_from(&[("genisoimage", true), ("file", true)]);
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
