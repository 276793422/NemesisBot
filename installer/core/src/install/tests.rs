use super::*;
use anyhow::Result;

#[test]
fn plan_detects_corrupt_record() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("t");
    std::fs::create_dir_all(&target)?;
    std::fs::write(target.join("install.json"), b"{ not json !!!")?;
    let err = plan(&target, Some("1.0.0")).expect_err("损坏标记必须 loud 报错");
    assert!(err.to_string().contains("install.json"), "{err}");
    // 空文件 = 视为无记录？——read_record 对空文件返回 None 会 panic unwrap
    // 在 plan 里 map().ok_or ——实际：Some 解析失败 → Err。预期就是 Err。
    std::fs::write(target.join("install.json"), b"")?;
    // 空文件路径：read_record 返回 None → plan 把它当损坏（None→err 分支）。
    assert!(plan(&target, Some("1.0.0")).is_err());
    Ok(())
}

#[test]
fn plan_version_uncomparable_is_loud() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("t");
    std::fs::create_dir_all(&target)?;
    std::fs::write(
        target.join("install.json"),
        r#"{"format_version":1,"bot_version":"mystery","install_time":0,"files":[],"uninstall_copy":false}"#,
    )?;
    assert!(
        plan(&target, Some("1.0.0")).is_err(),
        "不可比版本必须报错不猜"
    );
    Ok(())
}

#[test]
fn staging_dir_inside_target_with_pid() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let s = staging_dir(tmp.path())?;
    let name = s.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with(".nbsetup-stage-"), "{name}");
    assert!(
        name.contains(&std::process::id().to_string()),
        "带 pid 防并行撞名: {name}"
    );
    assert!(
        s.starts_with(tmp.path()),
        "staging 必须在目标目录内（同卷 rename）"
    );
    Ok(())
}

#[test]
fn uninstall_requires_record() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("t");
    std::fs::create_dir_all(&target)?;
    assert!(uninstall(&target).is_err(), "无 install.json 不得乱删");
    // 目录里随便有文件但没有标记 = 不动。
    std::fs::write(target.join("user-data.txt"), b"keep me")?;
    assert!(uninstall(&target).is_err());
    assert!(target.join("user-data.txt").exists(), "无标记不得删文件");
    Ok(())
}

#[test]
fn uninstall_absent_files_are_idempotent() -> Result<()> {
    // 清单里有文件已不在（用户手动删过）= 幂等成功，不算失败。
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("t");
    std::fs::create_dir_all(&target)?;
    std::fs::write(
        target.join("install.json"),
        r#"{"format_version":1,"bot_version":"1.0","install_time":0,"files":["gone.txt","present.txt"],"uninstall_copy":false}"#,
    )?;
    std::fs::write(target.join("present.txt"), b"x")?;
    let (deleted, failures) = uninstall(&target)?;
    assert_eq!(deleted, 2, "缺席文件幂等成功");
    assert!(failures.is_empty());
    assert!(!target.join("install.json").exists(), "标记摘除");
    assert!(!target.exists(), "目录空了应被摘除");
    Ok(())
}

#[test]
fn meta_version_projection() {
    assert_eq!(InstallPlan::Fresh.meta_version(), None);
    assert_eq!(InstallPlan::Repair("1".into()).meta_version(), Some("1"));
    assert_eq!(InstallPlan::Upgrade("2".into()).meta_version(), Some("2"));
    assert_eq!(
        InstallPlan::Downgrade {
            current: "3".into(),
            incoming: "2".into()
        }
        .meta_version(),
        Some("3")
    );
}

#[test]
fn typed_fail_display_and_exit_semantics() {
    // 三态类型化失败的 Display 文案（bin 侧打印 + 退出码映射锚点）。
    let e = TypedFail::Integrity("detail".into());
    assert!(e.to_string().contains("完整性"));
    let e = TypedFail::SigRisk("Tampered".into());
    assert!(
        e.to_string().contains("--accept-signature-risk"),
        "要提示豁免旗标: {e}"
    );
    let e = TypedFail::DowngradeRejected {
        current: "2.0".into(),
        incoming: "1.0".into(),
    };
    assert!(e.to_string().contains("--force"), "要提示豁免旗标: {e}");
}
