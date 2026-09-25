// db.rs 覆盖率补充测试（init_db 建父目录失败的 map_err 臂 347）。

use super::*;

/// 父路径组件是普通文件 → create_dir_all 失败 → Err 带「Failed to create
/// board directory」前缀（347）。
#[test]
fn init_db_fails_when_parent_is_a_file() {
    let dir = std::env::temp_dir().join(format!(
        "nmb-board-dbcov-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();

    let err = init_db(&blocker.join("board.db")).unwrap_err();
    assert!(
        err.contains("Failed to create board directory"),
        "实际错误：{err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// wave6 追加：父目录缺失时的成功创建形态（345-347 的 happy side）。
// ===========================================================================

/// db_path 的父目录不存在 → create_dir_all 现场补建，init_db 成功（347）。
#[test]
fn w6_init_db_creates_missing_parent_dir() {
    let dir = std::env::temp_dir().join(format!(
        "nmb-board-dbcov6-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let db_path = dir.join("fresh").join("nested").join("board.db");
    let conn = init_db(&db_path).expect("init_db 必须补建父目录");
    assert!(db_path.is_file());
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}
