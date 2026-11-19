//! charter 测试（W1 #1）：读空态 / set 盖章 / 损坏响报 / plan 建议投影。
//! 全部用自建 temp workspace 显式传路径（read/write_charter 均按入参
//! 定位），不重定向 home、不触碰真实盘工作区。

use super::{AgentCharter, plan_mode_hint, read_charter, write_charter};

fn temp_workspace(tag: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!(
        "charter-tests-{}-{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("create temp workspace");
    base
}

#[test]
fn absent_charter_reads_as_honest_empty_state() {
    let ws = temp_workspace("absent");
    let (charter, registered) = read_charter(ws.to_str().unwrap()).expect("read absent");
    assert!(!registered, "文件缺席 = registered:false");
    assert_eq!(charter, AgentCharter::default(), "缺席 = 全默认字段");
}

#[test]
fn set_updates_fields_and_stamps_time() {
    let ws = temp_workspace("set");
    let ws_str = ws.to_str().unwrap();
    // 初始登记（缺省字段保留语义：首次全默认起步）。
    write_charter(
        ws_str,
        &AgentCharter {
            purpose: "代码检索与分析".into(),
            scenarios: vec!["仓库审计".into()],
            read_only: true,
            boundaries: vec!["不写任何文件".into()],
            updated_at: 0,
        },
    )
    .expect("first write");
    let (c, registered) = read_charter(ws_str).unwrap();
    assert!(registered);
    assert_eq!(c.purpose, "代码检索与分析");
    assert!(c.updated_at > 0, "写入盖章 updated_at");
    assert!(c.read_only, "read_only 保留");
}

#[test]
fn corrupted_charter_is_loud_error() {
    let ws = temp_workspace("corrupt");
    let p = super::charter_path(ws.to_str().unwrap());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "{ not json").unwrap();
    let err = read_charter(ws.to_str().unwrap()).expect_err("corrupt = Err");
    assert!(err.contains("解析失败"), "损坏 = 诚实报错不静默回落: {err}");
}

#[test]
fn plan_hint_only_fires_for_readonly_in_build() {
    let ws = temp_workspace("hint");
    let ws_str = ws.to_str().unwrap();
    // 未登记：无建议。
    assert!(plan_mode_hint(Some(ws_str), "build").is_none());
    // 登记只读 + build：建议出现。
    write_charter(
        ws_str,
        &AgentCharter {
            purpose: String::new(),
            scenarios: vec![],
            read_only: true,
            boundaries: vec![],
            updated_at: 0,
        },
    )
    .unwrap();
    let hint = plan_mode_hint(Some(ws_str), "build").expect("read_only × build = 建议出现");
    assert!(hint.contains("plan"), "建议指向 plan 模式: {hint}");
    // 同 charter + plan 模式：无建议（已达标）。
    assert!(plan_mode_hint(Some(ws_str), "plan").is_none());
    // 非 read_only charter + build：无建议。
    write_charter(
        ws_str,
        &AgentCharter {
            read_only: false,
            ..AgentCharter::default()
        },
    )
    .unwrap();
    assert!(plan_mode_hint(Some(ws_str), "build").is_none());
    // workspace 未知：无建议（不撒谎）。
    assert!(plan_mode_hint(None, "build").is_none());
}
