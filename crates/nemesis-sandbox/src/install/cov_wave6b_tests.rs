//! install.rs 覆盖率收尾（Wave6B）：`ensure_installed` 的归属门 foreign
//! bail 分支——5 个 runtime 文件造齐让 verify_runtime 通过后，引擎归属
//! 非本安装（本机未装 Sandboxie → sc 查询 NotFound → engine_owned=false）
//! 时必须拒绝在「别人的 Sandboxie」上动工。
//!
//! 纪律：不装驱动、不起服务、不写 HKLM——bail 在任何 kmdutil 调用之前。

/// runtime 造齐 + 引擎非自有 → ensure_installed 必须以 foreign bail 拒绝。
/// 若本机恰好装着自家引擎（engine_owned=true），本用例无从伪造 foreign
/// 状态（归属门读的是真实 SCM），静默跳过——不 fail。
#[test]
fn ensure_installed_bails_on_foreign_engine() {
    let _logs = crate::test_util::capture_logs();
    let home = tempfile::tempdir().unwrap();
    let paths = crate::SandboxPaths::new(home.path());

    // 造齐 verify_runtime 要求的 5 个文件（哑内容即可——bail 在 spawn 之前）。
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    for f in [
        paths.kmdutil(),
        paths.start_exe(),
        paths.sbiedrv_sys(),
        paths.sbiesvc_exe(),
        paths.sbiemsg_dll(),
    ] {
        std::fs::write(&f, b"stub").unwrap();
    }
    paths
        .verify_runtime()
        .expect("5 个 runtime 文件齐备必须通过校验");

    if crate::status::engine_owned(&paths) {
        // 本机装着自家引擎：归属门放行，后续会真的动驱动/服务——不走。
        return;
    }

    let err = super::ensure_installed(&paths).expect_err("foreign 引擎必须被归属门拦下");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("foreign Sandboxie registered"),
        "bail 文案必须点明 foreign 归属: {msg}"
    );
    assert!(
        msg.contains(&paths.runtime_dir.display().to_string()),
        "bail 文案必须带上本安装 runtime 目录: {msg}"
    );
}
