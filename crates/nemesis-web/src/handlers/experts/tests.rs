//! experts handler 路由面测试（纯函数形态——直注 mock bridge，绕全局
//! OnceLock 首胜语义；plugins_wasm list_impl 同款纪律）。
//!
//! 覆盖：`_default` 保留键三面闸（get 投影默认卡 / update 路由改名 +
//! 字段闸 / remove+set_enabled 诚实拒绝）、普通专家的直通语义、参数
//! 校验（缺 key / 缺 enabled / 保留名 create）。资源层本体（磁盘读写、
//! slug 归一、lookup）在 bot 侧 experts_store_tests 钉死，此处不重复。

use super::{
    DEFAULT_KEY, ExpertsBridge, create_impl, get_impl, list_impl, remove_impl, set_enabled_impl,
    update_impl,
};
use crate::handlers::experts::{ExpertsHandler, set_experts_bridge};
use crate::ws_router::ModuleHandler;
use std::sync::Arc;

/// hire 面只有直通语义（校验归 bot 侧桥实现），用 MockBridge 直测形状
/// 契约（source=shop 投影 + 幂等覆盖 + 必填 slug/name）。
#[test]
fn hire_contract_and_idempotent_overwrite() {
    let b = MockBridge::empty();
    let payload = |summary: &str| {
        serde_json::json!({
            "slug": "code-master", "name": "代码大师", "summary": summary,
            "tags": ["dev"], "prompt": "You code.", "hired_from": "persona-42",
        })
    };
    let e = b.hire(&payload("v1")).unwrap();
    assert_eq!(e["source"], "shop");
    assert_eq!(e["hired_from"], "persona-42");
    b.hire(&payload("v2")).unwrap();
    let all = b.experts.lock();
    assert_eq!(all.len(), 1, "重雇不产生第二条");
    assert_eq!(all[0]["summary"], "v2");
    drop(all);

    // mock 自身契约：slug/name 必填（prompt 必填校验归 bot 侧桥——
    // nemesisbot 侧 experts_bridge_tests 钉死）。
    for missing in [
        serde_json::json!({ "name": "n", "prompt": "p" }),
        serde_json::json!({ "slug": "s", "prompt": "p" }),
    ] {
        assert!(b.hire(&missing).is_err(), "缺字段落必须拒: {missing}");
    }
}

/// 最小内存桥：只复刻 handler 依赖的语义形状（精确 slug 匹配 +
/// 字段合并），数据模型与磁盘细节归 bot 侧 ExpertsStore。
struct MockBridge {
    experts: parking_lot::Mutex<Vec<serde_json::Value>>,
    default_name: parking_lot::Mutex<String>,
}

impl MockBridge {
    fn empty() -> Self {
        Self {
            experts: parking_lot::Mutex::new(Vec::new()),
            default_name: parking_lot::Mutex::new("默认子代理".to_string()),
        }
    }

    fn with_expert(slug: &str, name: &str, prompt: &str, enabled: bool) -> Self {
        let s = Self::empty();
        s.experts.lock().push(serde_json::json!({
            "slug": slug,
            "name": name,
            "summary": "",
            "tags": [],
            "model": "",
            "enabled": enabled,
            "source": "manual",
            "hired_from": "",
            "prompt": prompt,
        }));
        s
    }
}

fn strip_prompt(e: &serde_json::Value) -> serde_json::Value {
    let mut v = e.clone();
    v.as_object_mut().unwrap().remove("prompt");
    v
}

impl ExpertsBridge for MockBridge {
    fn list(&self) -> serde_json::Value {
        let all = self.experts.lock();
        serde_json::json!({
            "experts": all.iter().map(strip_prompt).collect::<Vec<_>>(),
            "default_name": *self.default_name.lock(),
        })
    }

    fn get(&self, key: &str) -> Result<serde_json::Value, String> {
        self.experts
            .lock()
            .iter()
            .find(|e| e["slug"] == key)
            .cloned()
            .ok_or_else(|| format!("Unknown expert '{key}'"))
    }

    fn create(&self, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let name = data["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("专家名不能为空")?;
        let e = serde_json::json!({
            "slug": name,
            "name": name,
            "summary": data["summary"].as_str().unwrap_or(""),
            "tags": data["tags"].clone(),
            "model": data["model"].as_str().unwrap_or(""),
            "enabled": data["enabled"].as_bool().unwrap_or(true),
            "source": "manual",
            "hired_from": "",
            "prompt": data["prompt"].as_str().unwrap_or(""),
        });
        self.experts.lock().push(e.clone());
        Ok(e)
    }

    fn update(&self, key: &str, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut all = self.experts.lock();
        let e = all
            .iter_mut()
            .find(|e| e["slug"] == key)
            .ok_or_else(|| format!("Unknown expert '{key}'"))?;
        if let Some(obj) = data.as_object() {
            for (f, v) in obj {
                if f != "key" && !v.is_null() {
                    e[f] = v.clone();
                }
            }
        }
        Ok(e.clone())
    }

    fn remove(&self, key: &str) -> Result<(), String> {
        let mut all = self.experts.lock();
        let before = all.len();
        all.retain(|e| e["slug"] != key);
        if all.len() == before {
            return Err(format!("Unknown expert '{key}'"));
        }
        Ok(())
    }

    fn set_enabled(&self, key: &str, enabled: bool) -> Result<serde_json::Value, String> {
        let mut all = self.experts.lock();
        let e = all
            .iter_mut()
            .find(|e| e["slug"] == key)
            .ok_or_else(|| format!("Unknown expert '{key}'"))?;
        e["enabled"] = serde_json::Value::Bool(enabled);
        Ok(e.clone())
    }

    fn default_name(&self) -> String {
        self.default_name.lock().clone()
    }

    fn set_default_name(&self, name: &str) -> Result<serde_json::Value, String> {
        *self.default_name.lock() = name.to_string();
        Ok(serde_json::json!({
            "key": DEFAULT_KEY,
            "name": name,
            "is_default": true,
        }))
    }

    fn hire(&self, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let slug = data["slug"].as_str().ok_or("missing slug")?;
        let name = data["name"].as_str().ok_or("missing name")?;
        let e = serde_json::json!({
            "slug": slug,
            "name": name,
            "summary": data["summary"].as_str().unwrap_or(""),
            "tags": data["tags"].clone(),
            "model": "",
            "enabled": true,
            "source": "shop",
            "hired_from": data["hired_from"].as_str().unwrap_or(""),
            "prompt": data["prompt"].as_str().unwrap_or(""),
        });
        // 幂等重雇：同 slug 覆盖。
        let mut all = self.experts.lock();
        match all.iter_mut().find(|e| e["slug"] == slug) {
            Some(slot) => *slot = e.clone(),
            None => all.push(e.clone()),
        }
        Ok(e)
    }
}

// ---------------------------------------------------------------------------
// list / get
// ---------------------------------------------------------------------------

#[test]
fn list_returns_experts_and_default_name() {
    let b = MockBridge::with_expert("writer", "Writer", "You write.", true);
    let out = list_impl(&b);
    let arr = out["experts"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(arr[0].get("prompt").is_none(), "list 面不带 prompt 全文");
    assert_eq!(out["default_name"], "默认子代理");
}

#[test]
fn get_expert_includes_prompt_default_key_projects_card() {
    let b = MockBridge::with_expert("writer", "Writer", "You write.", true);
    let e = get_impl(&b, &serde_json::json!({ "key": "writer" })).unwrap();
    assert_eq!(e["prompt"], "You write.");

    let card = get_impl(&b, &serde_json::json!({ "key": DEFAULT_KEY })).unwrap();
    assert_eq!(card["is_default"], true);
    assert_eq!(card["name"], "默认子代理");
}

#[test]
fn missing_or_blank_key_rejected() {
    let b = MockBridge::empty();
    for data in [
        serde_json::json!({}),
        serde_json::json!({ "key": "" }),
        serde_json::json!({ "key": "   " }),
        serde_json::json!({ "key": 42 }),
    ] {
        let err = get_impl(&b, &data).unwrap_err();
        assert!(err.contains("missing key"), "data={data} 实际: {err}");
    }
}

// ---------------------------------------------------------------------------
// create：保留名闸 + 直通
// ---------------------------------------------------------------------------

#[test]
fn create_rejects_reserved_default_key_as_name() {
    let b = MockBridge::empty();
    let err = create_impl(&b, &serde_json::json!({ "name": "_default" })).unwrap_err();
    assert!(err.contains("保留键"), "实际: {err}");
    assert!(b.experts.lock().is_empty());

    let e = create_impl(
        &b,
        &serde_json::json!({ "name": "writer", "prompt": "You write." }),
    )
    .unwrap();
    assert_eq!(e["slug"], "writer");
    assert_eq!(b.experts.lock().len(), 1);
}

// ---------------------------------------------------------------------------
// update：普通专家直通 / _default 三面（改名路由 / 字段闸 / 缺 name）
// ---------------------------------------------------------------------------

#[test]
fn update_expert_passes_through_partial_data() {
    let b = MockBridge::with_expert("writer", "Writer", "old", true);
    let e = update_impl(
        &b,
        &serde_json::json!({ "key": "writer", "summary": "docs", "enabled": false }),
    )
    .unwrap();
    assert_eq!(e["summary"], "docs");
    assert_eq!(e["enabled"], false);
    assert_eq!(e["prompt"], "old", "未提及字段不动");
}

#[test]
fn update_default_routes_to_rename() {
    let b = MockBridge::empty();
    let card = update_impl(
        &b,
        &serde_json::json!({ "key": DEFAULT_KEY, "name": "总管" }),
    )
    .unwrap();
    assert_eq!(card["is_default"], true);
    assert_eq!(b.default_name(), "总管");
}

#[test]
fn update_default_rejects_non_name_fields() {
    let b = MockBridge::empty();
    for extra in [
        serde_json::json!({ "key": DEFAULT_KEY, "name": "总管", "prompt": "x" }),
        serde_json::json!({ "key": DEFAULT_KEY, "name": "总管", "enabled": false }),
        serde_json::json!({ "key": DEFAULT_KEY, "name": "总管", "model": "m" }),
        serde_json::json!({ "key": DEFAULT_KEY, "name": "总管", "tags": ["a"] }),
        serde_json::json!({ "key": DEFAULT_KEY, "name": "总管", "summary": "s" }),
    ] {
        let err = update_impl(&b, &extra).unwrap_err();
        assert!(err.contains("仅支持改名"), "data={extra} 实际: {err}");
    }
    assert_eq!(b.default_name(), "默认子代理", "拒绝路径不落盘");
}

#[test]
fn update_default_requires_name() {
    let b = MockBridge::empty();
    for data in [
        serde_json::json!({ "key": DEFAULT_KEY }),
        serde_json::json!({ "key": DEFAULT_KEY, "name": "" }),
        serde_json::json!({ "key": DEFAULT_KEY, "name": "   " }),
    ] {
        let err = update_impl(&b, &data).unwrap_err();
        assert!(err.contains("name 字段"), "data={data} 实际: {err}");
    }
}

// ---------------------------------------------------------------------------
// remove / set_enabled：_default 双闸
// ---------------------------------------------------------------------------

#[test]
fn remove_expert_ok_default_rejected() {
    let b = MockBridge::with_expert("gone", "Gone", "p", true);
    remove_impl(&b, &serde_json::json!({ "key": "gone" })).unwrap();
    assert!(b.experts.lock().is_empty());

    let err = remove_impl(&b, &serde_json::json!({ "key": DEFAULT_KEY })).unwrap_err();
    assert!(err.contains("不可删除"), "实际: {err}");
}

#[test]
fn set_enabled_expert_ok_default_rejected_missing_bool_rejected() {
    let b = MockBridge::with_expert("writer", "Writer", "p", true);
    let e = set_enabled_impl(
        &b,
        &serde_json::json!({ "key": "writer", "enabled": false }),
    )
    .unwrap();
    assert_eq!(e["enabled"], false);

    let err = set_enabled_impl(
        &b,
        &serde_json::json!({ "key": DEFAULT_KEY, "enabled": false }),
    )
    .unwrap_err();
    assert!(err.contains("无启停开关"), "实际: {err}");

    let err = set_enabled_impl(&b, &serde_json::json!({ "key": "writer" })).unwrap_err();
    assert!(err.contains("missing enabled"), "实际: {err}");
}

// ---------------------------------------------------------------------------
// 全局槽：注入后 handle_cmd 走通 list（OnceLock 首胜——本文件唯一注入点，
// 并行用例直注纯函数不受影响）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handle_cmd_routes_via_global_slot() {
    set_experts_bridge(Arc::new(MockBridge::with_expert(
        "reviewer",
        "Reviewer",
        "You review.",
        true,
    )));

    // ctx 参数在 handle_cmd 实现中未被读取（_ctx）——复用
    // roles_spawn_tests 的测试态字面量（pub(crate) 共享）。
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::handlers::roles_spawn_tests::make_ctx(&dir, None);

    let out = ExpertsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let arr = out["experts"].as_array().unwrap();
    assert!(arr.iter().any(|e| e["slug"] == "reviewer"));

    let err = ExpertsHandler
        .handle_cmd("nope", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown command"), "实际: {err}");
}
