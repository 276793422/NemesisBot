// middleware.rs 覆盖率补充测试（request_batch_permission 全臂 + 文件
// read/write 真实 IO + exec/get_output 的 spawn 失败臂 + Windows signal
// 不支持臂 + i2c 读写成功臂（伪造 i2c-tools 进 PATH））。
//
// 平台/环境豁免（Windows 测试运行不可达，仅记录不硬凑）：
// - 1008 / 1046 / 1084 / 1098 / 1106 / 1109 / 1137 / 1139-1156：cfg! 非
//   Windows 分支（kill -9 / ps -p / POSIX signal 体），Windows 上恒假。
// - 1055：terminate 成功臂 —— taskkill /PID（无 /F）对无窗口控制台进程
//   必败（实测 rc=1 "can only be terminated forcefully"），成功需要带
//   消息循环的 GUI 进程，无头测试环境不可得（且禁止弹窗）。
// - 1638-1643 / 1679：GPIO sysfs（/sys/class/gpio）读写成功臂，Linux 专属。
// 伪造 i2c-tools 已覆盖 1550 / 1552 / 1557-1565 / 1618 / 1620。

use super::*;
use std::sync::Arc;

fn allow_all_auditor() -> Arc<SecurityAuditor> {
    let auditor = SecurityAuditor::new(crate::auditor::AuditorConfig {
        enabled: true,
        ..Default::default()
    });
    auditor.set_default_action("allow");
    Arc::new(auditor)
}

fn deny_default_auditor() -> Arc<SecurityAuditor> {
    Arc::new(SecurityAuditor::new(crate::auditor::AuditorConfig {
        enabled: true,
        ..Default::default()
    }))
}

fn mw_with(
    auditor: Arc<SecurityAuditor>,
    preset: PermissionPreset,
    workspace: &str,
) -> SecurityMiddleware {
    SecurityMiddleware::with_preset(auditor, "cov-user", "cov-src", workspace, preset)
}

fn temp_ws(tag: &str) -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-mw-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_req(op: OperationType, target: &str) -> OperationRequest {
    OperationRequest {
        id: format!("cov-op-{target}"),
        op_type: op,
        danger_level: DangerLevel::Low,
        user: "cov".into(),
        source: "cli".into(),
        target: target.into(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// request_batch_permission 全臂
// ---------------------------------------------------------------------------

#[test]
fn batch_permission_empty_batch_errs() {
    let mw = mw_with(allow_all_auditor(), PermissionPreset::Standard, "");
    let batch = BatchOperationRequest::default();
    let err = mw.request_batch_permission(&batch).unwrap_err();
    assert!(err.contains("no operations in batch"), "{err}");
}

#[test]
fn batch_permission_success_returns_batch_id() {
    let mw = mw_with(allow_all_auditor(), PermissionPreset::Standard, "");
    let batch = BatchOperationRequest {
        id: "cov-batch-1".into(),
        operations: vec![
            file_req(OperationType::FileRead, "a.txt"),
            file_req(OperationType::DirRead, "sub"),
        ],
        ..Default::default()
    };
    let id = mw.request_batch_permission(&batch).unwrap();
    assert_eq!(id, "cov-batch-1");
}

#[test]
fn batch_permission_preset_denies_disallowed_operation() {
    let mw = mw_with(allow_all_auditor(), PermissionPreset::ReadOnly, "");
    let batch = BatchOperationRequest {
        id: "cov-batch-2".into(),
        operations: vec![
            file_req(OperationType::FileRead, "ok.txt"),
            file_req(OperationType::FileWrite, "no.txt"),
        ],
        ..Default::default()
    };
    let err = mw.request_batch_permission(&batch).unwrap_err();
    assert!(err.contains("not allowed under"), "{err}");
}

#[test]
fn batch_permission_summary_denied_by_auditor() {
    let mw = mw_with(deny_default_auditor(), PermissionPreset::Standard, "");
    let batch = BatchOperationRequest {
        id: "cov-batch-3".into(),
        operations: vec![file_req(OperationType::FileRead, "a.txt")],
        ..Default::default()
    };
    // 摘要请求是 ProcessExec，默认 deny → 摘要臂拒绝。
    let err = mw.request_batch_permission(&batch).unwrap_err();
    assert!(err.contains("denied"), "{err}");
}

#[test]
fn batch_permission_individual_operation_denied_after_summary_ok() {
    let auditor = deny_default_auditor();
    // 摘要（ProcessExec）放行，个体（FileRead）走默认 deny。
    auditor.set_rules(
        OperationType::ProcessExec,
        vec![SecurityRule {
            pattern: "*".into(),
            action: "allow".into(),
            comment: "cov allow summary".into(),
        }],
    );
    let mw = mw_with(auditor, PermissionPreset::Standard, "");
    let batch = BatchOperationRequest {
        id: "cov-batch-4".into(),
        operations: vec![file_req(OperationType::FileRead, "blocked.txt")],
        ..Default::default()
    };
    let err = mw.request_batch_permission(&batch).unwrap_err();
    assert!(err.contains("file_read"), "{err}");
}

// ---------------------------------------------------------------------------
// 文件真实 IO（read/write 全链）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_then_read_file_roundtrip_with_parent_creation() {
    let ws = temp_ws("io");
    let mw = mw_with(
        allow_all_auditor(),
        PermissionPreset::Standard,
        ws.to_str().unwrap(),
    );
    let path = ws.join("sub").join("f.txt");

    mw.file()
        .write_file(path.to_str().unwrap(), "cov content")
        .await
        .unwrap();
    let back = mw.file().read_file(path.to_str().unwrap()).await.unwrap();
    assert_eq!(back, "cov content");

    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// exec / get_output 的 spawn 失败臂（内嵌 NUL → CreateProcess 拒绝）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_command_spawn_failure_maps_to_err() {
    let mw = mw_with(allow_all_auditor(), PermissionPreset::Elevated, "");
    let err = mw
        .process()
        .execute_command("echo \0cov", 5)
        .await
        .unwrap_err();
    assert!(err.contains("failed to execute command"), "{err}");
}

#[tokio::test]
async fn get_output_spawn_failure_maps_to_err() {
    let mw = mw_with(allow_all_auditor(), PermissionPreset::Elevated, "");
    let err = mw.process().get_output("echo \0cov", 5).await.unwrap_err();
    assert!(err.contains("failed to execute command"), "{err}");
}

// ---------------------------------------------------------------------------
// Windows 上 POSIX signal 不支持臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signal_reports_unsupported_on_windows() {
    let mw = mw_with(allow_all_auditor(), PermissionPreset::Unrestricted, "");
    let err = mw.process().signal(424242, 15).await.unwrap_err();
    assert!(err.contains("not supported on Windows"), "{err}");
}

// ---------------------------------------------------------------------------
// i2c 成功臂：伪造 i2cget / i2cset 提前注入 PATH（单测内自含、结束还原）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn i2c_read_parses_hex_and_write_succeeds_with_fake_tools() {
    let fake = temp_ws("i2c");
    std::fs::write(fake.join("i2cget.cmd"), "@echo 0x1a\r\n").unwrap();
    std::fs::write(fake.join("i2cset.cmd"), "@rem cov ok\r\n").unwrap();

    let old_path = std::env::var("PATH").unwrap_or_default();
    // edition 2024：set_var 进 unsafe。只前插伪造目录（仅含 i2cget/i2cset），
    // 对并行测试无实质影响。
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("PATH", format!("{};{}", fake.display(), old_path));
    }

    let mw = mw_with(allow_all_auditor(), PermissionPreset::Unrestricted, "");
    let hw = mw.hardware();

    // 读：i2cget 返回 "0x1a" → 解析 26 → 大端填进 length=2 的缓冲。
    let data = hw.i2c_read("1", 0x50, 0x00, 2).await.unwrap();
    assert_eq!(data, vec![0x00, 0x1a]);

    // 写：i2cset 退出 0 → Ok(())。
    hw.i2c_write("1", 0x50, 0x00, &[0xAB]).await.unwrap();

    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("PATH", old_path);
    }
    let _ = std::fs::remove_dir_all(&fake);
}
