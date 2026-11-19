//! BUG-20261118-169 回归：apply_effort_autofix 的 effort_probe 非 object 守卫。
//!
//! 旧守卫 `entry.get("effort_probe").is_some()` 对 false/字符串/数字恒真
//! （JSON 值存在即 Some），后续嵌套可变索引 `entry["effort_probe"]["auto_fix"]`
//! 对非容器值 panic（serde_json index_or_insert 只容忍 Null 升级为 object，
//! Bool/String/Number/Array 字符串索引直接 panic）——探测自动修正链整段
//! 炸栈。触发面：Dashboard attr 编辑器与手改 config.json 可把 effort_probe
//! 写成任意 JSON 值；probe_effort_on_save 默认开，保存即触发探测 → 修正。
//!
//! 修复 = `is_some_and(|v| v.is_object())`；非 object 跳过注记（附注语义，
//! reasoning_effort 修正本体照常落盘；write_effort_probe 重探时整体覆盖）。
//!
//! 无进程级单例依赖：read_raw_config 直读磁盘（不走 load_live），
//! write_raw_config 的 global() 未装配时 None——纯 tempdir 即可。

use super::config::read_raw_config;
use super::models::apply_effort_autofix;

/// 播种 config.json：model_list 单条目，effort_probe 按用例给定。
fn seed_config(home: &std::path::Path, effort_probe: serde_json::Value) {
    let cfg = serde_json::json!({
        "model_list": [
            {
                "model_name": "m",
                "reasoning_effort": "high",
                "effort_probe": effort_probe,
            }
        ]
    });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn temp_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    (dir, home)
}

#[test]
fn autofix_skips_annotation_when_effort_probe_not_object() {
    let (_dir, home) = temp_home();
    // 修复前本测试 panic："cannot access key \"auto_fix\" in JSON boolean"。
    seed_config(&home, serde_json::json!(false));

    let applied = apply_effort_autofix(home.to_string_lossy().as_ref(), "m", "high", "medium")
        .expect("非 object effort_probe 必须跳过注记而非 panic");
    assert!(applied, "修正本体照常落盘");

    let cfg = read_raw_config(home.to_string_lossy().as_ref()).unwrap();
    let entry = &cfg["model_list"][0];
    assert_eq!(entry["reasoning_effort"], "medium", "修正本体生效");
    assert_eq!(
        entry["effort_probe"],
        serde_json::json!(false),
        "非 object effort_probe 原样保留（注记跳过，不破坏用户字段）"
    );
}

#[test]
fn autofix_writes_annotation_into_existing_object() {
    let (_dir, home) = temp_home();
    seed_config(&home, serde_json::json!({"accepted": ["minimal", "high"]}));

    let applied = apply_effort_autofix(home.to_string_lossy().as_ref(), "m", "high", "medium")
        .expect("object effort_probe 正常臂");
    assert!(applied);

    let cfg = read_raw_config(home.to_string_lossy().as_ref()).unwrap();
    let probe = &cfg["model_list"][0]["effort_probe"];
    assert_eq!(probe["auto_fix"]["from"], "high");
    assert_eq!(probe["auto_fix"]["to"], "medium");
    assert!(probe["auto_fix"]["at"].is_string(), "at 时间戳存在");
    assert_eq!(
        probe["accepted"],
        serde_json::json!(["minimal", "high"]),
        "兄弟键不动"
    );
}

#[test]
fn autofix_does_not_create_probe_when_missing() {
    let (_dir, home) = temp_home();
    // 缺失 effort_probe：is_some()=false 不进注记臂（修复前后同语义，钉住）。
    let cfg = serde_json::json!({
        "model_list": [{"model_name": "m", "reasoning_effort": "high"}]
    });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();

    let applied = apply_effort_autofix(home.to_string_lossy().as_ref(), "m", "high", "medium")
        .expect("缺失 effort_probe 正常臂");
    assert!(applied);

    let cfg = read_raw_config(home.to_string_lossy().as_ref()).unwrap();
    let entry = &cfg["model_list"][0];
    assert_eq!(entry["reasoning_effort"], "medium");
    assert!(
        entry.get("effort_probe").is_none(),
        "缺失时不创建 effort_probe 键"
    );
}
