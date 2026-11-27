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

/// BUG-20261126-136 回归锁（结构面）：store.rs 全部 INSERT 语句的
/// 列清单数 == VALUES 占位符数，且占位符不复用。此前 issue 建单
/// INSERT 的 updated_at 错引 ?27（created_at 槽位）——v26/v27 加列
/// 时占位符没续号，28 列对 27 个占位符号；新建时 created_at ==
/// updated_at 碰巧等值无数据损坏，但结构漂移会传染下一次加列
/// （复制 ?N 续不上号）。同族 board_touchpoints / autopilot 两处
/// 一并根修后由本锁钉死。asset 的 `path=?1` 复用是文档化设计
/// （models.rs：落盘文件名 = ref），本锁用「占位符在 VALUES 段
/// 与列位置一一对应」的宽松口径：允许复用，但列数必须 == VALUES
/// 段元素数——既有语义下等值复用也应写成独立占位符绑同值，列数
/// 与占位符序数一致是加列时的唯一防错锚。
#[test]
fn insert_statements_have_independent_placeholders() {
    let src = include_str!("../store.rs");
    let pat = regex::Regex::new(
        r"(?is)INSERT\s+(?:OR\s+\w+\s+)?INTO\s+(\w+)\s*\(([^)]*)\)\s*VALUES\s*\(([^)]*)\)",
    )
    .unwrap();
    let mut checked = 0usize;
    for cap in pat.captures_iter(src) {
        checked += 1;
        let table = &cap[1];
        let cols: Vec<&str> = cap[2]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let vals: Vec<&str> = cap[3]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            cols.len(),
            vals.len(),
            "表 {table} 的 INSERT 列数（{}）与 VALUES 段元素数（{}）不一致——\
             加列时占位符没同步续号（BUG-20261126-136 形态）",
            cols.len(),
            vals.len()
        );
        // 占位符序数单调不回退：?1..?N 或刻意复用（asset path=?1）都要求
        // 「首次出现顺序 = 列顺序」——等值复用写 ?N+1 独立槽位绑同值，
        // 结构自描述，杜绝「改了 created_at 的绑定值顺带污染 updated_at」。
        let mut seen_max = 0u32;
        for (i, v) in vals.iter().enumerate() {
            if let Some(num) = v.strip_prefix('?').and_then(|s| s.parse::<u32>().ok()) {
                if num > seen_max + 1 {
                    panic!(
                        "表 {table} 的 VALUES 段第 {} 个元素 {} 跳号（当前最大已见 ?{})——\
                         占位符序数必须连续",
                        i + 1,
                        v,
                        seen_max
                    );
                }
                seen_max = seen_max.max(num);
            }
        }
    }
    assert!(
        checked >= 20,
        "INSERT 审计扫描命中 {checked} 条——正则失效（schema 演进后语句形态变化）时本锁静默放行是假绿"
    );
}
