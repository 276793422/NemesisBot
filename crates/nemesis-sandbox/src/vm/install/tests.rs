//! install 模块单测——纯函数（分块调度/sidecar 解析/目录定位/哈希）钉死；
//! 下载器行为面（range 探测失败回落等）不起真实网络。

use super::*;
// ---------------------------------------------------------------------------
// plan_chunks：分块调度（覆盖恰为 total_len；余数摊前块）
// ---------------------------------------------------------------------------

#[test]
fn chunks_exact_division() {
    let chunks = plan_chunks(1200, 12);
    assert_eq!(chunks.len(), 12);
    assert_eq!(chunks[0], (0, 100));
    assert_eq!(chunks[11], (1100, 100));
    assert_eq!(chunks.iter().map(|(_, l)| l).sum::<u64>(), 1200);
}

#[test]
fn chunks_remainder_to_front() {
    // 1000 / 12 = 83 余 4 → 前 4 块 84，后 8 块 83。
    let chunks = plan_chunks(1000, 12);
    assert_eq!(chunks.len(), 12);
    assert_eq!(chunks[0], (0, 84));
    assert_eq!(chunks[3], (252, 84));
    assert_eq!(chunks[4], (336, 83));
    assert_eq!(chunks[11], (917, 83));
    let total: u64 = chunks.iter().map(|(_, l)| l).sum();
    assert_eq!(total, 1000);
    // 相邻块无缝（start 连续）。
    for w in chunks.windows(2) {
        assert_eq!(w[0].0 + w[0].1, w[1].0);
    }
}

#[test]
fn chunks_single_and_tiny() {
    assert_eq!(plan_chunks(500, 1), vec![(0, 500)]);
    assert_eq!(plan_chunks(500, 0), vec![(0, 500)]); // 0 = 单线兜底
    assert_eq!(plan_chunks(3, 12).len(), 3); // 块数收窄到总长
    assert_eq!(plan_chunks(0, 12), vec![(0, 0)]);
    let one = plan_chunks(1, 12);
    assert_eq!(one, vec![(0, 1)]);
}

// ---------------------------------------------------------------------------
// parse_sha512_sidecar：上游 sidecar 格式（`<hex>  <filename>`）
// ---------------------------------------------------------------------------

const KNOWN_SHA512: &str = "5bcf9eed634e8575a37b74f445af41a2fe4106da512d0c30c368301d4c105037fdfab40a5287367a28a957624cddebbc8c07e16c88ab6634f554cdf3d16bf543";

#[test]
fn sidecar_parse_weilnetz_format() {
    let body = format!("{KNOWN_SHA512}  qemu-w64-setup-20260811.exe\n");
    assert_eq!(parse_sha512_sidecar(&body).as_deref(), Some(KNOWN_SHA512));
    // 无尾换行 / 多空格 / 大写 hex 归一。
    assert_eq!(
        parse_sha512_sidecar(KNOWN_SHA512).as_deref(),
        Some(KNOWN_SHA512)
    );
    let upper = KNOWN_SHA512.to_uppercase();
    assert_eq!(parse_sha512_sidecar(&upper).as_deref(), Some(KNOWN_SHA512));
}

#[test]
fn sidecar_parse_rejects_malformed() {
    assert!(parse_sha512_sidecar("").is_none());
    assert!(parse_sha512_sidecar("abc").is_none());
    assert!(parse_sha512_sidecar(&"g".repeat(128)).is_none()); // 非 hex
    assert!(parse_sha512_sidecar(&"a".repeat(127)).is_none()); // 短
}

// ---------------------------------------------------------------------------
// locate_dir_containing：NSIS 布局漂移鲁棒（根 / qemu/ 子目录两形态）
// ---------------------------------------------------------------------------

#[test]
fn locate_finds_nested_qemu_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("qemu")).expect("mkdir");
    std::fs::write(root.join("qemu").join("qemu-system-x86_64.exe"), b"x").expect("write");
    std::fs::write(root.join("unins000.exe"), b"n").expect("write noise");
    let found = locate_dir_containing(root, "qemu-system-x86_64.exe").expect("locate");
    assert_eq!(found, root.join("qemu"));
}

#[test]
fn locate_finds_at_root_and_skips_noise_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::write(root.join("qemu-system-x86_64.exe"), b"x").expect("write");
    // 噪音目录里也有一份——最浅命中优先（root 层）。
    std::fs::create_dir_all(root.join("$PLUGINSDIR")).expect("mkdir");
    std::fs::write(
        root.join("$PLUGINSDIR").join("qemu-system-x86_64.exe"),
        b"y",
    )
    .expect("write");
    let found = locate_dir_containing(root, "qemu-system-x86_64.exe").expect("locate");
    assert_eq!(found, root);
}

#[test]
fn locate_absent_is_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let err =
        locate_dir_containing(tmp.path(), "qemu-system-x86_64.exe").expect_err("should not find");
    assert!(err.to_string().contains("未找到"));
}

#[cfg(target_os = "windows")]
#[test]
fn locate_accepts_windows_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("a").join("b")).expect("mkdir");
    std::fs::write(root.join("a").join("b").join("qemu-img.exe"), b"x").expect("write");
    let found = locate_dir_containing(root, "qemu-img.exe").expect("locate");
    assert_eq!(found, root.join("a").join("b"));
}

// ---------------------------------------------------------------------------
// 哈希：空文件 SHA-512/SHA-256 已知向量（FIPS 180-4）
// ---------------------------------------------------------------------------

#[test]
fn sha512_empty_file_known_vector() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let p = tmp.path().join("empty.bin");
    std::fs::write(&p, b"").expect("write");
    let got = sha512_hex_file(&p).expect("hash");
    assert_eq!(
        got,
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
}

#[test]
fn sha256_empty_file_known_vector() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let p = tmp.path().join("empty.bin");
    std::fs::write(&p, b"").expect("write");
    let got = sha256_hex_file(&p).expect("hash");
    assert_eq!(
        got,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

// ---------------------------------------------------------------------------
// 钉值常量自洽（hex128、与默认 URL 同代）
// ---------------------------------------------------------------------------

#[test]
fn pinned_constants_are_wellformed() {
    assert_eq!(QEMU_WINDOWS_SHA512.len(), 128);
    assert!(QEMU_WINDOWS_SHA512.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(QEMU_WINDOWS_URL.starts_with("https://qemu.weilnetz.de/w64/"));
    assert!(QEMU_WINDOWS_URL.ends_with(".exe"));
}

// ---------------------------------------------------------------------------
// verify_installer：钉值不符的诊断分支（sidecar 可达性不影响错误定性）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verify_installer_empty_file_fails_with_pin() {
    // 空文件 vs 钉值 → 必败；sidecar 分支内含网络调用（失败容忍），只断言
    // 错误定性（「SHA-512」字样在消息里）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let p = tmp.path().join("f.bin");
    std::fs::write(&p, b"").expect("write");
    let opts_expected = Some(KNOWN_SHA512.to_string());
    let err = verify_installer(&p, &opts_expected, "https://invalid.invalid/x.exe")
        .await
        .expect_err("must fail");
    assert!(err.to_string().contains("SHA-512"), "err: {err}");
}

// ---------------------------------------------------------------------------
// QemuInstallOptions 默认值（钉 URL + 钉 SHA + 12 路）
// ---------------------------------------------------------------------------

#[test]
fn options_default_pins_official_source() {
    let o = QemuInstallOptions::default();
    assert_eq!(o.url, QEMU_WINDOWS_URL);
    assert_eq!(o.expected_sha512.as_deref(), Some(QEMU_WINDOWS_SHA512));
    assert_eq!(o.parallel, DEFAULT_PARALLEL);
}

// ---------------------------------------------------------------------------
// B2：DISM 状态解析 / CPU vendor / 引导命令构造（纯函数，不真跑 sudo/DISM）
// ---------------------------------------------------------------------------

#[test]
fn dism_state_parse_all_shapes() {
    use DismFeatureState as S;
    assert_eq!(
        parse_dism_feature_state("State : Enabled"),
        Some(S::Enabled)
    );
    // 启用+禁用挂起（罕见态）按 Enabled 走——DLL 缺席时仍归 BIOS 臂。
    assert_eq!(
        parse_dism_feature_state("State : Enabled with Disable Pending"),
        Some(S::Enabled)
    );
    assert_eq!(
        parse_dism_feature_state("State : Enable Pending"),
        Some(S::EnablePending)
    );
    assert_eq!(
        parse_dism_feature_state("State : Disabled"),
        Some(S::Disabled)
    );
    // 本地化 Windows / 缺 State 行 = None（分类器诚实回落）。
    assert_eq!(parse_dism_feature_state("状态 : 已启用"), None);
    assert_eq!(parse_dism_feature_state(""), None);
    assert_eq!(
        parse_dism_feature_state("FeatureName: HypervisorPlatform\nno state line"),
        None
    );
}

#[test]
fn dism_args_shapes_pinned() {
    let enable = dism_enable_whpx_args();
    assert!(enable.contains(&"/Online".to_string()));
    assert!(enable.contains(&"/NoRestart".to_string()));
    assert!(enable.contains(&"/Enable-Feature".to_string()));
    assert!(enable.contains(&format!("/FeatureName:{DISM_WHPX_FEATURE}")));
    let query = dism_query_state_args();
    assert!(query.contains(&"/Get-FeatureInfo".to_string()));
    assert!(query.contains(&format!("/FeatureName:{DISM_WHPX_FEATURE}")));
    assert!(!query.iter().any(|a| a == "/NoRestart"), "查询不写系统态");
}

#[test]
fn cpu_vendor_parse_and_kvm_module_map() {
    use CpuVendor as V;
    let intel = "processor : 0\nvendor_id\t: GenuineIntel\nflags: fpu vmx";
    assert_eq!(parse_cpu_vendor(intel), Some(V::Intel));
    let amd = "vendor_id\t: AuthenticAMD\nflags: fpu svm";
    assert_eq!(parse_cpu_vendor(amd), Some(V::Amd));
    assert_eq!(parse_cpu_vendor("no vendor line"), None);
    assert_eq!(kvm_module_for_vendor(V::Intel), "kvm_intel");
    assert_eq!(kvm_module_for_vendor(V::Amd), "kvm_amd");
}

#[test]
fn linux_pm_packages_and_commands_pinned() {
    use LinuxPm as P;
    assert_eq!(
        qemu_packages_for_pm(P::Apt),
        Some("qemu-system-x86 qemu-utils")
    );
    assert_eq!(qemu_packages_for_pm(P::Dnf), Some("qemu-kvm qemu-img"));
    assert_eq!(qemu_packages_for_pm(P::Unknown), None);
    let apt = pm_install_qemu_command(P::Apt).unwrap();
    assert!(
        apt.contains("DEBIAN_FRONTEND=noninteractive")
            && apt.contains("apt-get install -y qemu-system-x86 qemu-utils"),
        "{apt}"
    );
    let dnf = pm_install_qemu_command(P::Dnf).unwrap();
    assert!(dnf.contains("dnf install -y qemu-kvm qemu-img"), "{dnf}");
    assert_eq!(pm_install_qemu_command(P::Unknown), None);
    assert_eq!(modprobe_kvm_command("kvm_intel"), "sudo modprobe kvm_intel");
    assert_eq!(usermod_kvm_command("zoo"), "sudo usermod -aG kvm zoo");
}

/// 二进制名单单一真相源：与平台形态一致（Windows 带 .exe），双名非空且不同。
#[test]
fn required_binaries_names_match_platform() {
    let (sys, img) = qemu_required_binaries();
    assert!(!sys.is_empty() && !img.is_empty());
    assert_ne!(sys, img);
    if cfg!(target_os = "windows") {
        assert!(sys.ends_with(".exe") && img.ends_with(".exe"));
        assert_eq!(sys, "qemu-system-x86_64.exe");
        assert_eq!(img, "qemu-img.exe");
    } else {
        assert_eq!(sys, "qemu-system-x86_64");
        assert_eq!(img, "qemu-img");
    }
}

/// Linux 包管理器探测：本机形态断言（测试跑在哪就钉哪——探测函数本身
/// 的真值只有真实宿主能裁）。
#[cfg(target_os = "linux")]
#[test]
fn detect_linux_pm_matches_host_shape() {
    let pm = detect_linux_pm();
    // 任何真实 Linux 宿主至少应识别出三种形态之一，且与 /usr/bin/apt-get
    // 或 dnf 的在场一致（zoo 机 = apt）。
    let has_apt = Path::new("/usr/bin/apt-get").exists();
    let has_dnf = Path::new("/usr/bin/dnf").exists() || Path::new("/usr/bin/yum").exists();
    match pm {
        LinuxPm::Apt => assert!(has_apt, "探测为 Apt 但 apt-get 不在场"),
        LinuxPm::Dnf => assert!(has_dnf, "探测为 Dnf 但 dnf/yum 不在场"),
        LinuxPm::Unknown => {
            assert!(!has_apt && !has_dnf, "apt/dnf 在场却探测为 Unknown");
        }
    }
}
