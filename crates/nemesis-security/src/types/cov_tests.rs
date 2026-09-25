// types.rs 覆盖率补充测试（OperationType::SystemConfig Display 64 /
// validate_path 的 strip_prefix 失败且不前缀匹配的拒绝臂 461 及其收尾
// 468）。

use super::*;

/// Display 全臂抽样：SystemConfig（64）及其邻位，锁定 snake_case 拼写。
#[test]
fn operation_type_display_system_config() {
    assert_eq!(OperationType::SystemConfig.to_string(), "system_config");
    assert_eq!(OperationType::SystemService.to_string(), "system_service");
    assert_eq!(OperationType::RegistryDelete.to_string(), "registry_delete");
}

/// workspace 之外且 strip_prefix 失败（异盘/异树）→ 拒绝（461/468）。
#[test]
fn validate_path_rejects_foreign_tree_outside_workspace() {
    let ws = std::env::temp_dir().join("nmb-types-cov-ws");
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();

    // 同盘不同子树：strip_prefix 失败 → starts_with 也失败 → Err。
    let outside = std::env::temp_dir().join("nmb-types-cov-elsewhere");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("evil.txt");
    std::fs::write(&outside_file, b"x").unwrap();

    let err = validate_path(outside_file.to_str().unwrap(), ws.to_str().unwrap()).unwrap_err();
    assert!(err.contains("outside workspace"), "{err}");

    // workspace 内 → Ok 且返回归一路径（468 收尾臂）。
    let inside = ws.join("ok.txt");
    std::fs::write(&inside, b"x").unwrap();
    let ok = validate_path(inside.to_str().unwrap(), ws.to_str().unwrap()).unwrap();
    assert!(!ok.is_empty());

    // 空 workspace → 不做范围检查。
    assert!(validate_path(outside_file.to_str().unwrap(), "").is_ok());

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}
