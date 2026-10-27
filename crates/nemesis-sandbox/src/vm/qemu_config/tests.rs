//! [`qemu_config`] 单测：模板自检（五键/钉值形态/宿主键命中）+ 校验闸三态
//! + 解析链/释放语义（tempdir 实证）。

use super::*;

// ---------------------------------------------------------------------------
// 内嵌模板自检（模板-解析器同编译期产物；改模板必过这组）
// ---------------------------------------------------------------------------

/// 五格矩阵全在（用户拍板：全格填真值，不因 guest 系统面进度留空格）。
#[test]
fn embedded_template_has_all_five_platforms() {
    let m = embedded_matrix();
    for key in [
        "windows-x86_64",
        "windows-aarch64",
        "linux-x86_64",
        "linux-aarch64",
        "android-aarch64",
    ] {
        let e = m
            .platforms
            .get(key)
            .unwrap_or_else(|| panic!("默认矩阵缺平台条目 {key}"));
        assert!(e.produced, "{key} 默认应 produced=true（安装面成立）");
        assert!(e.binaries.len() >= 2, "{key} binaries 至少 system+img");
    }
}

/// 钉值形态（原 install 常量自检测试的矩阵版：钉值现在住模板里）。
#[test]
fn embedded_windows_archive_pin_is_wellformed() {
    let m = embedded_matrix();
    let e = m.platforms.get("windows-x86_64").expect("windows 条目");
    assert_eq!(e.kind, QemuInstallKind::Archive);
    let url = e.url.as_str();
    assert!(url.starts_with("https://qemu.weilnetz.de/w64/"), "{url}");
    assert!(url.ends_with(".exe"));
    assert_eq!(e.sha512.len(), 128);
    assert!(e.sha512.bytes().all(|b| b.is_ascii_hexdigit()));
    // binaries 带 .exe 后缀（Windows 形态）。
    assert!(e.binaries.iter().all(|b| b.ends_with(".exe")));
}

/// 任何编译宿主的键都命中默认矩阵（CI 三平台 + Android NDK 构建全过）。
#[test]
fn host_key_hits_default_matrix() {
    let key = host_key();
    assert!(key.contains('-'), "host_key 形态 {{os}}-{{arch}}：{key}");
    let entry = host_entry(embedded_matrix());
    assert!(
        entry.is_some(),
        "宿主键 {key} 必须在默认矩阵中（新平台先补矩阵再上构建）"
    );
}

// ---------------------------------------------------------------------------
// 校验闸（parse_matrix loud 拒绝面）
// ---------------------------------------------------------------------------

fn base_entry_json(kind: &str, extra: &str) -> String {
    format!(
        r#"{{"version":1,"platforms":{{"test-x86_64":{{"kind":"{kind}","binaries":["qemu-system-x86_64","qemu-img"]{extra}}}}}}}"#
    )
}

#[test]
fn archive_entry_without_url_rejected() {
    let err = parse_matrix(&base_entry_json("archive", "")).expect_err("archive 缺 url 必拒");
    let msg = err.to_string();
    assert!(msg.contains("url 为空"), "{msg}");
}

#[test]
fn packages_entry_without_any_pm_rejected() {
    let err = parse_matrix(&base_entry_json("packages", "")).expect_err("packages 全空必拒");
    assert!(err.to_string().contains("apt/dnf/pacman 全空"), "{}", err);
}

#[test]
fn packages_entry_with_pacman_accepted() {
    let m = parse_matrix(&base_entry_json(
        "packages",
        r#","pacman":"mingw-w64-clang-aarch64-qemu""#,
    ))
    .expect("合法 packages 条目");
    assert_eq!(
        m.platforms["test-x86_64"].pacman,
        "mingw-w64-clang-aarch64-qemu"
    );
}

#[test]
fn unknown_kind_rejected_loudly() {
    // 未知 kind（serde unknown variant）——不做静默纠偏。
    let r = parse_matrix(&base_entry_json("tarball", ""));
    assert!(r.is_err(), "未知 kind 必须拒绝");
}

#[test]
fn unknown_field_rejected_loudly() {
    // deny_unknown_fields：字段名 typo 早炸（静默忽略 = 埋雷）。
    let json = r#"{"version":1,"platforms":{"t":{"kind":"none","binary":["x"]}}}"#;
    let r = parse_matrix(json);
    assert!(r.is_err(), "未知字段必须拒绝");
}

#[test]
fn future_version_rejected() {
    let json = r#"{"version":2,"platforms":{"t":{"kind":"none"}}}"#;
    let err = parse_matrix(json).expect_err("未知版本必须拒绝");
    assert!(err.to_string().contains("version=2"), "{}", err);
}

#[test]
fn binaries_below_two_rejected() {
    // packages 形态触发 binaries 下限（none 豁免——见下一条）。
    let json = r#"{"version":1,"platforms":{"t":{"kind":"packages","apt":"q","binaries":["only-system"]}}}"#;
    let err = parse_matrix(json).expect_err("binaries <2 必拒");
    assert!(err.to_string().contains("至少 2 项"), "{}", err);
}

/// none = 登记在案形态，无产物可断言 → binaries 下限豁免（合法）。
#[test]
fn binaries_rule_exempt_for_none_kind() {
    let json = r#"{"version":1,"platforms":{"t":{"kind":"none","binaries":["only-one"]}}}"#;
    parse_matrix(json).expect("none 豁免 binaries 下限");
}

/// produced=false 是合法配置（登记在案形态）——解析接受，执行面拒。
#[test]
fn produced_false_parses_ok() {
    let json = r#"{"version":1,"platforms":{"t":{"kind":"none","produced":false}}}"#;
    let m = parse_matrix(json).expect("produced=false 合法");
    assert!(!m.platforms["t"].produced);
}

// ---------------------------------------------------------------------------
// 自服务报错文案
// ---------------------------------------------------------------------------

#[test]
fn unavailable_error_lists_known_keys_and_note() {
    let m = embedded_matrix();
    let err = unavailable_error(m, "riscv-future", None);
    let msg = err.to_string();
    assert!(msg.contains("riscv-future"), "{msg}");
    assert!(msg.contains("android-aarch64"), "已知键清单: {msg}");
    assert!(msg.contains(CONFIG_FILE_NAME), "配置文件指路: {msg}");
    // 命中已有条目时 note 透出。
    let err2 = unavailable_error(m, "windows-x86_64", None);
    assert!(
        err2.to_string().contains("weilnetz"),
        "命中条目的 note 应透出: {err2}"
    );
}

// ---------------------------------------------------------------------------
// 解析链三态 + 释放语义（tempdir 实证）
// ---------------------------------------------------------------------------

#[test]
fn resolve_none_falls_back_to_embedded() {
    let (r, source) = resolve_matrix(None);
    assert!(r.is_ok());
    assert_eq!(source, MatrixSource::Embedded);
}

#[test]
fn resolve_existing_file_takes_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join(CONFIG_FILE_NAME);
    std::fs::write(&cfg, EMBEDDED_TEMPLATE).expect("write");
    let (r, source) = resolve_matrix(Some(&cfg));
    r.expect("同源模板解析必过");
    assert_eq!(source, MatrixSource::WorkspaceFile);
}

#[test]
fn resolve_corrupt_file_is_loud_not_silent_fallback() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join(CONFIG_FILE_NAME);
    std::fs::write(&cfg, "{ not json").expect("write");
    let (r, source) = resolve_matrix(Some(&cfg));
    assert!(
        r.is_err(),
        "损坏文件必须 loud 报错（静默回内嵌 = 用户改了不生效的假象）"
    );
    assert_eq!(source, MatrixSource::WorkspaceFile);
}

#[test]
fn resolve_missing_file_falls_back_to_embedded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join(CONFIG_FILE_NAME);
    let (r, source) = resolve_matrix(Some(&cfg));
    r.expect("NotFound 回内嵌不报错");
    assert_eq!(source, MatrixSource::Embedded);
}

#[test]
fn ensure_released_writes_once_and_never_overwrites() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 首次：写入 + true。
    assert!(ensure_released(tmp.path()).expect("release"));
    let cfg = tmp.path().join(CONFIG_FILE_NAME);
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read"),
        EMBEDDED_TEMPLATE
    );
    // 用户改动后：不覆盖（「有则直接用」）。
    std::fs::write(&cfg, "{\"user-edited\":true}").expect("edit");
    assert!(
        !ensure_released(tmp.path()).expect("second"),
        "已存在不重写"
    );
    assert_eq!(
        std::fs::read_to_string(&cfg).expect("read"),
        "{\"user-edited\":true}",
        "用户改动必须保留"
    );
}

// ---------------------------------------------------------------------------
// 安装消费辅助
// ---------------------------------------------------------------------------

#[test]
fn required_binaries_pair_from_matrix_entry() {
    let m = embedded_matrix();
    let e = m.platforms.get("linux-x86_64").expect("linux 条目");
    let (sys, img) = required_binaries_for(e).expect("binaries 对");
    assert_eq!(sys, "qemu-system-x86_64");
    assert_eq!(img, "qemu-img");
    // 缺项诚实报错。
    let bad = QemuPlatformEntry {
        kind: QemuInstallKind::None,
        produced: true,
        url: String::new(),
        sha512: String::new(),
        layout: String::new(),
        apt: String::new(),
        dnf: String::new(),
        pacman: String::new(),
        binaries: vec!["only-one".to_string()],
        note: String::new(),
    };
    assert!(required_binaries_for(&bad).is_err());
}
