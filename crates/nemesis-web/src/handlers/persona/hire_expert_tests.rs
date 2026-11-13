//! 雇佣专家（shop.hire_expert，2026-11-13 专家子代理批次③）测试：
//! 载荷构造纯函数（temp workspace 文件级验证，无桥依赖——桥调用面
//! 在 experts::tests 的 hire 契约测试覆盖）。
//!
//! - 未安装诚实拒绝（前置 shop.download）
//! - PERSONA.json name/description 映射 + 缺席回落 id
//! - SOUL.md 全文 → prompt；trim 后空 = 诚实拒绝；缺文件 = 诚实拒绝
//! - slug/hired_from = 人格目录 id；tags 空数组

use super::*;

#[test]
fn hire_payload_rejects_uninstalled_persona() {
    let dir = tempfile::tempdir().unwrap();
    let err = expert_hire_payload(dir.path().to_str().unwrap(), "not-there").unwrap_err();
    assert!(err.contains("not installed"), "实际: {err}");
    assert!(err.contains("shop.download"), "必须指路先下载: {err}");
}

#[test]
fn hire_payload_maps_persona_json_and_soul() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let pdir = ws.join("personas").join("code-master");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(
        pdir.join("PERSONA.json"),
        r#"{"name": "代码大师", "emoji": "🧑‍💻", "description": "writes code"}"#,
    )
    .unwrap();
    std::fs::write(pdir.join("SOUL.md"), "\n# 灵魂\n\n写好代码。\n").unwrap();

    let payload = expert_hire_payload(ws.to_str().unwrap(), "code-master").unwrap();
    assert_eq!(payload["slug"], "code-master");
    assert_eq!(payload["hired_from"], "code-master");
    assert_eq!(payload["name"], "代码大师");
    assert_eq!(payload["summary"], "writes code");
    assert_eq!(payload["tags"], serde_json::json!([]));
    // SOUL trim：前后空白剥离，正文逐字保留。
    assert_eq!(payload["prompt"], "# 灵魂\n\n写好代码。");
}

#[test]
fn hire_payload_falls_back_without_persona_json() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let pdir = ws.join("personas").join("bare-id");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("SOUL.md"), "prompt body").unwrap();

    let payload = expert_hire_payload(ws.to_str().unwrap(), "bare-id").unwrap();
    assert_eq!(payload["name"], "bare-id", "PERSONA.json 缺席 → id 回落");
    assert_eq!(payload["summary"], "");
    assert_eq!(payload["prompt"], "prompt body");
}

#[test]
fn hire_payload_rejects_empty_and_missing_soul() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    // SOUL.md 纯空白（trim 后空）
    let pdir = ws.join("personas").join("blank-soul");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("SOUL.md"), "   \n\r\n  ").unwrap();
    let err = expert_hire_payload(ws.to_str().unwrap(), "blank-soul").unwrap_err();
    assert!(err.contains("SOUL.md 为空"), "实际: {err}");

    // SOUL.md 缺席
    let pdir2 = ws.join("personas").join("no-soul");
    std::fs::create_dir_all(&pdir2).unwrap();
    std::fs::write(pdir2.join("PERSONA.json"), r#"{"name": "x"}"#).unwrap();
    let err = expert_hire_payload(ws.to_str().unwrap(), "no-soul").unwrap_err();
    assert!(err.contains("failed to read SOUL.md"), "实际: {err}");
}
