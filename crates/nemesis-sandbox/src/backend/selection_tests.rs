//! P1（2026-09-25 能力扩展 WS1）Linux 沙箱网络选型决策表测试。
//!
//! [`super::select_linux_backend`] 是纯函数（平台 × 网络要求 × 可用后端 →
//! 选择），在**所有平台**编译执行——Windows/macOS 开发机也能钉死 Linux 选型
//! 语义。真实后端探测/包装行为由 `tests.rs` 的 linux_live 模块（cfg linux
//! 真机）覆盖，本文件只钉**决策表本身**。

use super::*;

/// 构造 Availability 的便捷函数。
fn full() -> Availability {
    Availability::Full
}
fn unavail() -> Availability {
    Availability::Unavailable("probe says no".to_string())
}
/// Partial 算可用（规则装得上、有能力缺口）。
fn partial() -> Availability {
    Availability::Partial(vec!["kernel ABI older than requested".to_string()])
}

// ---------------------------------------------------------------------------
// 决策表（模块文档那张表的逐行钉死）
// ---------------------------------------------------------------------------

/// 行①：landlock ✅ + bwrap ✅ + 允许网络 → landlock（允许网络场景自装优先）。
#[test]
fn allow_network_prefers_landlock_when_both_available() {
    assert_eq!(
        select_linux_backend(&full(), &full(), true),
        Some(LinuxBackendKind::Landlock)
    );
}

/// 行②（P1 要修的洞）：landlock ✅ + bwrap ✅ + 要求禁网 → **bwrap**
/// （--unshare-net 是唯一真禁网面；旧链恒 landlock 优先 = 形同不禁网）。
#[test]
fn deny_network_selects_bwrap_when_available() {
    assert_eq!(
        select_linux_backend(&full(), &full(), false),
        Some(LinuxBackendKind::Bwrap)
    );
}

/// 行③：landlock ✅ + bwrap ❌ + 要求禁网 → landlock **降级**
/// （gaps 继续诚实标注网络缺口——apply_to_self 侧既有行为，不在本表）。
#[test]
fn deny_network_without_bwrap_degrades_to_landlock() {
    assert_eq!(
        select_linux_backend(&full(), &unavail(), false),
        Some(LinuxBackendKind::Landlock)
    );
    // 允许网络 + 无 bwrap → landlock（旧行为不变）。
    assert_eq!(
        select_linux_backend(&full(), &unavail(), true),
        Some(LinuxBackendKind::Landlock)
    );
}

/// 行④：landlock ❌ + bwrap ✅ → bwrap（任何网络要求；旧行为保留）。
#[test]
fn no_landlock_falls_back_to_bwrap() {
    assert_eq!(
        select_linux_backend(&unavail(), &full(), false),
        Some(LinuxBackendKind::Bwrap)
    );
    assert_eq!(
        select_linux_backend(&unavail(), &full(), true),
        Some(LinuxBackendKind::Bwrap)
    );
}

/// 行⑤：两个都不可用 → None（调用方 warn + 无盒降级）。
#[test]
fn both_unavailable_selects_none() {
    assert_eq!(select_linux_backend(&unavail(), &unavail(), false), None);
    assert_eq!(select_linux_backend(&unavail(), &unavail(), true), None);
}

/// Partial 可用性算「装得上」：landlock Partial + 禁网 + bwrap 可用 → 仍
/// 选 bwrap（禁网判据优先于缺口多少）；landlock Partial + 允许网络 → landlock。
#[test]
fn partial_availability_counts_as_available() {
    assert_eq!(
        select_linux_backend(&partial(), &full(), false),
        Some(LinuxBackendKind::Bwrap)
    );
    assert_eq!(
        select_linux_backend(&partial(), &unavail(), true),
        Some(LinuxBackendKind::Landlock)
    );
    // bwrap 侧 Partial（理论上不出现，判据一致）同样算可用。
    assert_eq!(
        select_linux_backend(&unavail(), &partial(), false),
        Some(LinuxBackendKind::Bwrap)
    );
}

// ---------------------------------------------------------------------------
// 平台路径不受影响（P1 验收：非 Linux 路径行为不变）
// ---------------------------------------------------------------------------

/// Windows：恒 None（Sandboxie 承担，U11 设计契约）——入参两态都 None。
#[cfg(target_os = "windows")]
#[test]
fn detect_backend_none_on_windows_both_network_modes() {
    assert!(detect_backend(false).is_none());
    assert!(detect_backend(true).is_none());
}

/// macOS：Seatbelt 恒被选中（可用时）——其 profile 本身按 allow_network 生成
/// `(deny network*)`，禁网是真强制、无需换挡。可用性随机器变，只断言
/// 「选择不因 allow_network 翻转」这一 P1 不变量。
#[cfg(target_os = "macos")]
#[test]
fn detect_backend_seatbelt_unaffected_by_network_mode() {
    let a = detect_backend(false).map(|b| b.name().to_string());
    let b = detect_backend(true).map(|b| b.name().to_string());
    assert_eq!(a, b, "macOS 选型不随 allow_network 翻转");
}

// ---------------------------------------------------------------------------
// P24（2026-09-25）：Windows 用户态轻量档选型决策表（executor.backend）
// ---------------------------------------------------------------------------

fn acl_full() -> Availability {
    Availability::Full
}
fn acl_partial() -> Availability {
    Availability::Partial(vec!["experimental gaps".to_string()])
}

/// auto + Sandboxie 就绪 → Sandboxie（就绪优先，ACL 只是回落）。
#[test]
fn windows_auto_prefers_sandboxie_when_ready() {
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Auto, true, &acl_full()),
        Some(WindowsBackendKind::Sandboxie)
    );
    // 就绪优先不看 acl 侧可用性（Partial/Unavailable 均然）。
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Auto, true, &unavail()),
        Some(WindowsBackendKind::Sandboxie)
    );
}

/// auto + Sandboxie 未就绪 + acl 可用 → acl（Full 与 Partial 都算可用，
/// 与 Linux 表同判据）。
#[test]
fn windows_auto_falls_back_to_acl() {
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Auto, false, &acl_full()),
        Some(WindowsBackendKind::Acl)
    );
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Auto, false, &acl_partial()),
        Some(WindowsBackendKind::Acl)
    );
}

/// auto + 两皆不可用 → None（调用方 warn + 无盒降级）。
#[test]
fn windows_auto_none_when_both_unavailable() {
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Auto, false, &unavail()),
        None
    );
}

/// 显式 sandboxie：就绪 → Sandboxie；未就绪 → None（诚实失败，**不悄悄
/// 改道 acl**——显式选择是用户意图）。
#[test]
fn windows_explicit_sandboxie_never_rewrites_to_acl() {
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Sandboxie, true, &acl_full()),
        Some(WindowsBackendKind::Sandboxie)
    );
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Sandboxie, false, &acl_full()),
        None
    );
}

/// 显式 acl：可用 → Acl；不可用 → None（同理不改道 Sandboxie）。
#[test]
fn windows_explicit_acl_never_rewrites_to_sandboxie() {
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Acl, true, &acl_full()),
        Some(WindowsBackendKind::Acl)
    );
    assert_eq!(
        select_windows_backend(&ExecutorBackendChoice::Acl, false, &unavail()),
        None
    );
}

/// 未知值 → 恒 None（诚实拒绝，不静默猜测；原文保留供调用方 warn）。
#[test]
fn windows_unknown_choice_is_none() {
    let other = ExecutorBackendChoice::Other("docker".to_string());
    assert_eq!(select_windows_backend(&other, true, &acl_full()), None);
    assert_eq!(select_windows_backend(&other, false, &unavail()), None);
}

/// parse_executor_backend 值域表：None/空/auto（大小写不敏感）→ Auto；
/// 显式两值；未知 → Other(小写原文)。
#[test]
fn parse_executor_backend_value_domain() {
    assert_eq!(parse_executor_backend(None), ExecutorBackendChoice::Auto);
    assert_eq!(
        parse_executor_backend(Some("")),
        ExecutorBackendChoice::Auto
    );
    assert_eq!(
        parse_executor_backend(Some("auto")),
        ExecutorBackendChoice::Auto
    );
    assert_eq!(
        parse_executor_backend(Some("  AUTO ")),
        ExecutorBackendChoice::Auto
    );
    assert_eq!(
        parse_executor_backend(Some("sandboxie")),
        ExecutorBackendChoice::Sandboxie
    );
    assert_eq!(
        parse_executor_backend(Some("ACL")),
        ExecutorBackendChoice::Acl
    );
    assert_eq!(
        parse_executor_backend(Some("Docker")),
        ExecutorBackendChoice::Other("docker".to_string())
    );
}
