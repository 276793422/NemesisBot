//! ExpertsBridge 实现测试（nemesisbot 侧桥适配层）。
//!
//! 覆盖：hire 面必填校验（slug/name/prompt——web 侧 mock 桥不复制这些
//! 校验，真闸在这里）+ hire 落盘往返（source=shop/hired_from/prompt）+
//! create/get/update 的 JSON 投影（list 无 prompt 全文 / get 带全文）。

use crate::experts_store::ExpertsStore;
use nemesis_web::handlers::experts::ExpertsBridge;
use tempfile::TempDir;

fn store() -> (TempDir, ExpertsStore) {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = ExpertsStore::new(tmp.path());
    (tmp, s)
}

#[test]
fn hire_bridge_validates_required_fields() {
    let (_t, s) = store();
    for missing in [
        serde_json::json!({ "name": "n", "prompt": "p" }), // 缺 slug
        serde_json::json!({ "slug": "s", "prompt": "p" }), // 缺 name
        serde_json::json!({ "slug": "s", "name": "n" }),   // 缺 prompt
        serde_json::json!({ "slug": "s", "name": "n", "prompt": "  " }), // prompt 纯空白
    ] {
        let err = ExpertsBridge::hire(&s, &missing).unwrap_err();
        assert!(
            err.contains("missing"),
            "缺字段落必须拒: {missing} 实际: {err}"
        );
    }
}

#[test]
fn hire_bridge_roundtrip_shop_shape() {
    let (_t, s) = store();
    let entry = ExpertsBridge::hire(
        &s,
        &serde_json::json!({
            "slug": "code-master", "name": "代码大师",
            "summary": "writes code", "tags": ["dev", "rust"],
            "prompt": "You code well.", "hired_from": "persona-42",
        }),
    )
    .unwrap();
    assert_eq!(entry["source"], "shop");
    assert_eq!(entry["hired_from"], "persona-42");
    assert!(entry.get("prompt").is_none(), "hire 返回面不带 prompt 全文");
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["model"], "", "雇佣初始无模型覆盖");

    // get 带 prompt 全文 + 字段落盘正确。
    let full = ExpertsBridge::get(&s, "code-master").unwrap();
    assert_eq!(full["prompt"], "You code well.");
    assert_eq!(full["tags"], serde_json::json!(["dev", "rust"]));

    // list 面无 prompt。
    let list = ExpertsBridge::list(&s);
    assert!(list["experts"][0].get("prompt").is_none());
    assert_eq!(list["default_name"], ExpertsStore::FALLBACK_DEFAULT_NAME);
}

#[test]
fn bridge_create_get_update_json_shape() {
    let (_t, s) = store();
    let entry = ExpertsBridge::create(
        &s,
        &serde_json::json!({
            "name": "reviewer", "summary": "reviews", "tags": ["rv"],
            "prompt": "You review.", "key": "noise-ignored",
        }),
    )
    .unwrap();
    assert_eq!(entry["slug"], "reviewer");
    assert_eq!(entry["source"], "manual");

    // update 部分字段（key 字段是 WSAPI 路由参数，桥侧 from_value 忽略）。
    // 返回面不带 prompt（体积面），未提及字段不动经 get 复核。
    let upd = ExpertsBridge::update(
        &s,
        "reviewer",
        &serde_json::json!({ "key": "reviewer", "summary": "v2", "enabled": false }),
    )
    .unwrap();
    assert_eq!(upd["summary"], "v2");
    assert_eq!(upd["enabled"], false);
    assert!(upd.get("prompt").is_none(), "update 返回面不带 prompt");

    let full = ExpertsBridge::get(&s, "reviewer").unwrap();
    assert_eq!(full["prompt"], "You review.", "未提及字段不动");
    assert_eq!(full["summary"], "v2");
}

#[test]
fn bridge_malformed_data_honest_error() {
    let (_t, s) = store();
    // tags 非数组（serde 类型错）→ 参数解析失败诚实报错。
    let err = ExpertsBridge::create(
        &s,
        &serde_json::json!({ "name": "x", "tags": "not-array", "prompt": "p" }),
    )
    .unwrap_err();
    assert!(err.contains("参数解析失败"), "实际: {err}");
}
