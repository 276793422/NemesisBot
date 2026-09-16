//! CMD 族攻击矩阵（2026-09-16 第二批·灾难复发批验收）。
//!
//! 装载 `nemesisbot/config/config.security.<平台>.json` **真实出货模板**，
//! 按 gateway 同款方式构造 `SecurityAuditor`（分节规则 + D1 策略 +
//! 自杀形态保护路径），对敌意/良性命令全集断言 ABAC（Layer 3）判定。
//! Guard（Layer 2）层形态在 `src/command/tests.rs` 单独覆盖。
//!
//! 敌意面四族：
//! 1. 直球形态（模板 deny 规则锚定命中）
//! 2. 包装形态（外层 allow/无规则，内层载荷 deny——deny-first + 拆段扫描）
//! 3. 归一化变体（引号/大小写/空白）
//! 4. ask 面（模板 ask 规则 → require approval，不再谎报 deny 也不放行）
//!
//! 良性面：开发工作流常规命令一律放行（防误伤回归线）。

use nemesis_security::auditor::{AuditorConfig, SecurityAuditor};
use nemesis_security::types::{DangerLevel, OperationType, SecurityRule};

fn load_template(platform: &str) -> serde_json::Value {
    let path = format!(
        "{}/../../nemesisbot/config/config.security.{platform}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("出货模板必须存在：{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("模板 JSON 非法：{path}: {e}"))
}

fn op_for(section: &str, op: &str) -> Option<OperationType> {
    match (section, op) {
        ("file_rules", "read") => Some(OperationType::FileRead),
        ("file_rules", "write") => Some(OperationType::FileWrite),
        ("file_rules", "delete") => Some(OperationType::FileDelete),
        ("dir_rules", "read") => Some(OperationType::DirRead),
        ("dir_rules", "create") => Some(OperationType::DirCreate),
        ("dir_rules", "delete") => Some(OperationType::DirDelete),
        ("process_rules", "exec") => Some(OperationType::ProcessExec),
        ("process_rules", "spawn") => Some(OperationType::ProcessSpawn),
        ("process_rules", "kill") => Some(OperationType::ProcessKill),
        ("process_rules", "suspend") => Some(OperationType::ProcessSuspend),
        ("network_rules", "request") => Some(OperationType::NetworkRequest),
        ("network_rules", "download") => Some(OperationType::NetworkDownload),
        ("network_rules", "upload") => Some(OperationType::NetworkUpload),
        ("hardware_rules", "i2c") => Some(OperationType::HardwareI2C),
        ("hardware_rules", "spi") => Some(OperationType::HardwareSPI),
        ("hardware_rules", "gpio") => Some(OperationType::HardwareGPIO),
        ("registry_rules", "read") => Some(OperationType::RegistryRead),
        ("registry_rules", "write") => Some(OperationType::RegistryWrite),
        ("registry_rules", "delete") => Some(OperationType::RegistryDelete),
        _ => None,
    }
}

/// 按 gateway security_setup 同款方式从模板构造 auditor（分节规则 + D1 +
/// 保护路径）。
fn auditor_from_template(cfg: &serde_json::Value, protected: &[&str]) -> SecurityAuditor {
    let auditor = SecurityAuditor::new(AuditorConfig {
        enabled: true,
        default_action: cfg["default_action"]
            .as_str()
            .unwrap_or("allow")
            .to_string(),
        ..Default::default()
    });
    for section in [
        "file_rules",
        "dir_rules",
        "process_rules",
        "network_rules",
        "hardware_rules",
        "registry_rules",
    ] {
        let Some(obj) = cfg[section].as_object() else {
            continue;
        };
        for (op, entries) in obj {
            let Some(op_type) = op_for(section, op) else {
                continue;
            };
            let Some(arr) = entries.as_array() else {
                continue;
            };
            let rules: Vec<SecurityRule> = arr
                .iter()
                .filter_map(|e| {
                    Some(SecurityRule {
                        pattern: e["pattern"].as_str()?.to_string(),
                        action: e["action"].as_str()?.to_string(),
                        comment: e["comment"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect();
            auditor.set_rules(op_type, rules);
        }
    }
    if let Some(p) = cfg["exec_unknown_policy"].as_str() {
        auditor.set_exec_unknown_policy(p);
    }
    auditor.set_protected_paths(protected.iter().map(|s| s.to_string()).collect());
    auditor
}

fn exec(auditor: &SecurityAuditor, id: &str, cmd: &str) -> (bool, Option<String>) {
    let req = nemesis_security::auditor::OperationRequest {
        id: id.to_string(),
        op_type: OperationType::ProcessExec,
        danger_level: DangerLevel::Critical,
        user: "matrix".to_string(),
        source: "test".to_string(),
        target: cmd.to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, err, _) = auditor.request_permission(&req);
    (allowed, err)
}

fn expect_denied(auditor: &SecurityAuditor, cmd: &str) {
    let (allowed, err) = exec(auditor, &format!("deny-{cmd}"), cmd);
    assert!(!allowed, "`{cmd}` must be DENIED, got err={err:?}");
    let err = err.unwrap_or_default();
    assert!(
        err.contains("denied"),
        "`{cmd}` must be a hard deny, got: {err}"
    );
}

fn expect_ask(auditor: &SecurityAuditor, cmd: &str) {
    let (allowed, err) = exec(auditor, &format!("ask-{cmd}"), cmd);
    assert!(!allowed, "`{cmd}` must NOT pass through, got err={err:?}");
    let err = err.unwrap_or_default();
    assert!(
        err.contains("approval required"),
        "`{cmd}` must be an honest ASK, got: {err}"
    );
}

fn expect_allowed(auditor: &SecurityAuditor, cmd: &str) {
    let (allowed, err) = exec(auditor, &format!("allow-{cmd}"), cmd);
    assert!(allowed, "`{cmd}` must be ALLOWED, got err={err:?}");
}

// ---------------------------------------------------------------------------
// Windows 模板
// ---------------------------------------------------------------------------

#[test]
fn windows_direct_hostile_denied() {
    let cfg = load_template("windows");
    let auditor = auditor_from_template(&cfg, &["c:/users/zoo", "c:/users/zoo/proj"]);
    for cmd in [
        "rm -rf d:/work",
        "rm -fr d:/work",
        "rm --recursive d:/work",
        "remove-item -recurse c:/tmp/x",
        "robocopy c:/src d:/dst /MIR",
        "diskpart",
        "cipher /w c:",
        "vssadmin delete shadows /all",
        "wevtutil cl security",
        "schtasks /delete /tn daily",
        "sc delete svc",
        "rd /s d:/legacy",
    ] {
        expect_denied(&auditor, cmd);
    }
}

#[test]
fn windows_wrapped_hostile_inner_payload_denied() {
    // 外层无规则命中（或 allow 包装），内层载荷 deny——deny-first + 拆段。
    let cfg = load_template("windows");
    let auditor = auditor_from_template(&cfg, &["c:/users/zoo", "c:/users/zoo/proj"]);
    for cmd in [
        "powershell -c \"remove-item -recurse c:/tmp/x\"",
        "python -c \"os.system('rm -rf d:/work')\"",
        "node -e \"require('child_process').execSync('rd /s d:/legacy')\"",
    ] {
        expect_denied(&auditor, cmd);
    }
}

#[test]
fn windows_ask_surface_is_honest() {
    // 分层注记（复核 2026-09-16，A-F6）：本文件只测裸 ABAC 层（Layer 3），
    // 不含 Layer 2 Guard blocklist。其中 `set-executionpolicy remotesigned`、
    // `clear-content d:/log.txt` 在真实 8 层管线里会先被 Layer 2 命令守卫
    // 硬拦（command.rs blocklist 的 set_executionpolicy / clear_content），
    // 根本到不了这里的 ask 模板规则——这两条钉的是生产拓扑中不可达的分支，
    // 仅作 ABAC 模板覆盖证据，不构成「ask 面诚实」的强信号。真实链路
    // 能到 Layer 3 的弱 ask 面由前两条（del/rmdir）代表。
    let cfg = load_template("windows");
    let auditor = auditor_from_template(&cfg, &["c:/users/zoo", "c:/users/zoo/proj"]);
    for cmd in [
        "del d:/report.txt",
        "rmdir d:/empty",
        "set-executionpolicy remotesigned",
        "clear-content d:/log.txt",
    ] {
        expect_ask(&auditor, cmd);
    }
}

#[test]
fn windows_benign_workflow_allowed() {
    let cfg = load_template("windows");
    let auditor = auditor_from_template(&cfg, &["c:/users/zoo", "c:/users/zoo/proj"]);
    for cmd in [
        "git status",
        "git push origin main",
        "npm run build",
        "python script.py",
        "node server.js",
        "go run main.go",
        "cargo test --release",
    ] {
        expect_allowed(&auditor, cmd);
    }
}

// ---------------------------------------------------------------------------
// Linux 模板
// ---------------------------------------------------------------------------

#[test]
fn linux_direct_hostile_denied() {
    let cfg = load_template("linux");
    let auditor = auditor_from_template(&cfg, &["/home/zoo", "/home/zoo/proj"]);
    for cmd in [
        "rm -rf /srv/data",
        "find /var/log -name '*.log' -delete",
        "dd of=/dev/sdb if=/dev/zero",
        "mkfs.ext4 /dev/sdb",
        "wipefs /dev/sdb",
        "shred /dev/sdb",
        "chmod -r 000 /etc",
        "sudo apt install curl",
    ] {
        expect_denied(&auditor, cmd);
    }
}

#[test]
fn linux_wrapped_hostile_inner_payload_denied() {
    let cfg = load_template("linux");
    let auditor = auditor_from_template(&cfg, &["/home/zoo", "/home/zoo/proj"]);
    for cmd in [
        "bash -c \"rm -rf /srv/data\"",
        "python -c \"import os; os.system('rm -rf /srv')\"",
        "sh -c \"find /var -delete\"",
    ] {
        expect_denied(&auditor, cmd);
    }
}

#[test]
fn linux_benign_workflow_allowed() {
    let cfg = load_template("linux");
    let auditor = auditor_from_template(&cfg, &["/home/zoo", "/home/zoo/proj"]);
    for cmd in [
        "git status",
        "npm run build",
        "python script.py",
        "node server.js",
        "go run main.go",
        "cargo test --release",
    ] {
        expect_allowed(&auditor, cmd);
    }
}

// ---------------------------------------------------------------------------
// Darwin 模板
// ---------------------------------------------------------------------------

#[test]
fn darwin_disk_surface() {
    let cfg = load_template("darwin");
    let auditor = auditor_from_template(&cfg, &["/users/zoo", "/users/zoo/proj"]);
    expect_denied(&auditor, "diskutil erasedisk disk2");
    // 非 erase 的 diskutil 只 ask（比 deny 温和，但不静默放行）
    expect_ask(&auditor, "diskutil list");
    expect_allowed(&auditor, "git status");
}

// ---------------------------------------------------------------------------
// Other 模板（CFG-03 catch-all 回归）
// ---------------------------------------------------------------------------

#[test]
fn other_catch_all_delete_ask() {
    let cfg = load_template("other");
    let auditor = auditor_from_template(&cfg, &["/home/zoo", "/home/zoo/proj"]);
    // file_rules.delete 的 `*` ask 兜底（CFG-03：模板 delete 缺 catch-all 曾全放行）
    let req = nemesis_security::auditor::OperationRequest {
        id: "other-file-del".to_string(),
        op_type: OperationType::FileDelete,
        danger_level: DangerLevel::High,
        user: "matrix".to_string(),
        source: "test".to_string(),
        target: "/home/zoo/data/important.db".to_string(),
        timestamp: None,
        ..Default::default()
    };
    let (allowed, err) = {
        let (allowed, err, _) = auditor.request_permission(&req);
        (allowed, err)
    };
    assert!(!allowed, "catch-all delete must not pass through");
    assert!(
        err.unwrap_or_default().contains("approval required"),
        "catch-all delete must be ASK"
    );
}

// ---------------------------------------------------------------------------
// 自杀形态 × 模板叠加（硬拦先于一切规则遍，含 allow 模板）
// ---------------------------------------------------------------------------

#[test]
fn self_destruct_beats_template_allow() {
    // `rm -rf ~/proj` 命中保护路径（~/proj 在 home 之下）——即使模板
    // 对 rm 只有 ask 规则（other 模板），硬拦也是 deny 语义。
    let cfg = load_template("other");
    let auditor = auditor_from_template(&cfg, &["/home/zoo"]);
    let (allowed, err) = exec(&auditor, "sd-template", "rm -rf /home/zoo/proj");
    assert!(!allowed);
    assert!(
        err.unwrap_or_default().contains("self-destruct"),
        "protected-path hit must hard-deny regardless of template ask"
    );
}
