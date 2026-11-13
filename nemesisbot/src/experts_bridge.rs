//! 专家管理面 WSAPI 桥——[`ExpertsStore`](crate::experts_store::ExpertsStore)
//! （资源层）到 `nemesis_web::handlers::experts::ExpertsBridge` trait 的适配。
//!
//! 职责单一：ExpertFull → JSON 投影（`list` 面不带 prompt 全文——体积面，
//! 编辑模态走 `get`；`get` 带 prompt 全文）+ ExpertUpsert 反序列化容错。
//! gateway 启动装配注入静态槽（`agent_build.rs`）；CLI/headless 不注入 =
//! WSAPI 诚实报错，无旁路。

use crate::experts_store::{ExpertFull, ExpertUpsert, ExpertsStore};
use nemesis_web::handlers::experts::ExpertsBridge;

/// 条目 JSON 投影（字段序 = 前端契约；with_prompt 控制体积面）。
fn entry_json(e: &ExpertFull, with_prompt: bool) -> serde_json::Value {
    let mut v = serde_json::json!({
        "slug": e.slug,
        "name": e.name,
        "summary": e.summary,
        "tags": e.tags,
        "model": e.model,
        "enabled": e.enabled,
        "source": e.source,
        "hired_from": e.hired_from,
    });
    if with_prompt {
        v["prompt"] = serde_json::Value::String(e.prompt.clone());
    }
    v
}

fn upsert_from(data: &serde_json::Value) -> Result<ExpertUpsert, String> {
    serde_json::from_value(data.clone()).map_err(|e| format!("参数解析失败: {e}"))
}

impl ExpertsBridge for ExpertsStore {
    fn list(&self) -> serde_json::Value {
        let experts: Vec<serde_json::Value> =
            self.list().iter().map(|e| entry_json(e, false)).collect();
        serde_json::json!({
            "experts": experts,
            "default_name": ExpertsStore::default_name(self),
        })
    }

    fn get(&self, key: &str) -> Result<serde_json::Value, String> {
        self.get(key).map(|e| entry_json(&e, true))
    }

    fn create(&self, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let up = upsert_from(data)?;
        self.create(&up).map(|e| entry_json(&e, false))
    }

    fn update(&self, key: &str, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let up = upsert_from(data)?;
        self.update(key, &up).map(|e| entry_json(&e, false))
    }

    fn remove(&self, key: &str) -> Result<(), String> {
        ExpertsStore::remove(self, key)
    }

    fn set_enabled(&self, key: &str, enabled: bool) -> Result<serde_json::Value, String> {
        self.set_enabled(key, enabled)
            .map(|e| entry_json(&e, false))
    }

    fn default_name(&self) -> String {
        ExpertsStore::default_name(self)
    }

    fn set_default_name(&self, name: &str) -> Result<serde_json::Value, String> {
        ExpertsStore::set_default_name(self, name)?;
        Ok(serde_json::json!({
            "key": nemesis_web::handlers::experts::DEFAULT_KEY,
            "name": self.default_name(),
            "is_default": true,
        }))
    }

    fn hire(&self, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        let field_str = |f: &str| -> Result<String, String> {
            data.get(f)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("missing {f}"))
        };
        let slug = field_str("slug")?;
        let name = field_str("name")?;
        let prompt = data
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or("missing prompt（SOUL.md 缺失或为空？）")?;
        let summary = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let tags: Vec<String> = data
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let hired_from = data
            .get("hired_from")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        self.hire(&slug, &name, &summary, tags, prompt, &hired_from)
            .map(|e| entry_json(&e, false))
    }

    fn config_get(&self) -> serde_json::Value {
        ExpertsStore::mode_policy_json(self)
    }

    fn config_set(&self, data: &serde_json::Value) -> Result<serde_json::Value, String> {
        ExpertsStore::set_mode_policy(self, data)
    }
}
