//! BUG-20261120-030 回归锁：`load_config` 读侧形状闸。
//!
//! Config 全字段带 default，`[]` 文档曾从 serde visit_seq 零元素臂溜过 =
//! 「全默认 Config」合法值——整档用户配置（model_list/API key/channels）
//! 被静默无视、出厂默认启动，零诊断。object 闸必须在 typed 解析前拦下
//! （启动即失败优于带错静跑）。语义单一真相源 = nemesis_utils::json_shape。

use nemesis_config::{ConfigError, load_config};
use std::path::PathBuf;

fn write_file(dir: &tempfile::TempDir, content: &str) -> PathBuf {
    let path = dir.path().join("config.json");
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn load_config_rejects_array_document_loud() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_config(&write_file(&dir, "[]")).unwrap_err();
    let ConfigError::Validation(msg) = err else {
        panic!("expected Validation error, got {err:?}");
    };
    assert!(msg.contains("must be a JSON object"), "msg={msg}");
    assert!(msg.contains("array"), "msg={msg}");
}

#[test]
fn load_config_rejects_scalar_documents_loud() {
    let dir = tempfile::tempdir().unwrap();
    for content in ["null", "\"config\"", "42", "true"] {
        let err = load_config(&write_file(&dir, content)).unwrap_err();
        let ConfigError::Validation(msg) = err else {
            panic!("expected Validation error for {content}, got {err:?}");
        };
        assert!(
            msg.contains("must be a JSON object"),
            "content={content} msg={msg}"
        );
    }
}

#[test]
fn load_config_still_accepts_object_document() {
    let dir = tempfile::tempdir().unwrap();
    // 最小 object 文档（全字段 default，缺键合法）。
    let cfg = load_config(&write_file(&dir, r#"{"agents": {}}"#)).expect("object doc parses");
    // 判别字段确从文档消费（不是 vacuous Ok）。
    assert!(cfg.agents.image_downscale, "default image_downscale=true");
}
