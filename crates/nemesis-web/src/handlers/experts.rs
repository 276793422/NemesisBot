//! 专家子代理管理面（`experts.*` WSAPI）。
//!
//! 资源层（`<workspace>/experts/` 目录即注册表）住在 bot 侧
//! （nemesisbot::experts_store），本模块经 **静态槽桥**（[`set_experts_bridge`]
//! + [`ExpertsBridge`] trait）访问——plugins_wasm INSTALLER 槽同款先例
//!   （AppState 字面量散布百处测试不可爆破）。槽空 = 专家子系统未装配
//!   （CLI/headless/独立 web 测试形态），命令统一诚实报错。
//!
//! 命令：
//! - `experts.list` — 全量在册 + 默认卡数据（`default_name`；条目不带
//!   prompt 全文，编辑模态走 `get`）。
//! - `experts.get {key}` — 单条（含 prompt 全文）。
//! - `experts.create {name, summary?, tags?, model?, prompt?}` — 手工创建
//!   （prompt 可空草稿——dispatch 面诚实拒绝空提示词专家）。
//! - `experts.update {key, ...}` — 部分更新；`key="_default"` 路由默认卡
//!   （仅接受 name = 默认子代理改名，其余字段诚实拒绝）。
//! - `experts.remove {key}` — 删除（`_default` 诚实拒绝——出厂常驻）。
//! - `experts.set_enabled {key, enabled}` — 启停（`_default` 诚实拒绝——
//!   默认子代理无开关）。
//! - `experts.config_get` — 专家模式策略投影（`{enabled, degrade, notify}`
//!   flat 三键，2026-11-13 总开关 + 自动降级）。
//! - `experts.config_set {enabled?, degrade?, notify?}` — 部分更新（boolean；
//!   raw patch 原子写 + 开启动作 opt-in 结构化日志）。
//!
//! 默认子代理保留键 [`DEFAULT_KEY`]（`_default`）：默认卡不是专家记录，
//! 执行形态零改变（spawn 不点名时的兜底），唯一可编辑字段 = 显示名。

use std::sync::Arc;

use crate::ws_router::{ModuleHandler, RequestContext};

/// 默认子代理保留键（`experts.*` 的 key 参数取此值 = 操作默认卡而非专家）。
pub const DEFAULT_KEY: &str = "_default";

/// 专家管理面桥（bot 侧 ExpertsStore 经 trait object 注入；方法签名
/// JSON 进出——条目数据模型住在 bot 侧，web 侧不复制字段结构）。
pub trait ExpertsBridge: Send + Sync {
    /// 全量在册 + 默认卡：`{"experts": [...], "default_name": "..."}`。
    fn list(&self) -> serde_json::Value;
    fn get(&self, key: &str) -> Result<serde_json::Value, String>;
    fn create(&self, data: &serde_json::Value) -> Result<serde_json::Value, String>;
    fn update(&self, key: &str, data: &serde_json::Value) -> Result<serde_json::Value, String>;
    fn remove(&self, key: &str) -> Result<(), String>;
    fn set_enabled(&self, key: &str, enabled: bool) -> Result<serde_json::Value, String>;
    fn default_name(&self) -> String;
    fn set_default_name(&self, name: &str) -> Result<serde_json::Value, String>;
    /// 超市雇佣落盘（幂等重雇覆盖）。data = `{slug, name, summary, tags,
    /// prompt, hired_from}`（persona.shop.hire_expert 消费）。
    fn hire(&self, data: &serde_json::Value) -> Result<serde_json::Value, String>;
    /// 专家模式策略读（config `agents.experts` 段投影，flat 三键）。
    /// 默认体 = `Default` 值投影（模式关/降级开/通知关——策略面未注入
    /// 的诚实保守形态；bot 侧真实现覆盖）。
    fn config_get(&self) -> serde_json::Value {
        serde_json::json!({ "enabled": false, "degrade": true, "notify": false })
    }
    /// 专家模式策略部分更新（`{enabled?, degrade?, notify?}` boolean；
    /// raw JSON patch 原子写，不经 typed round-trip）。默认体 = 诚实
    /// 拒绝（策略面未注入）。开启动作的后台 opt-in 记录（结构化日志）
    /// 在实现侧。
    fn config_set(&self, _data: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("experts 模式策略面未装配（本进程无 config 写权限）".to_string())
    }
}

static EXPERTS_BRIDGE: std::sync::OnceLock<Arc<dyn ExpertsBridge>> = std::sync::OnceLock::new();

/// gateway 启动装配注入（OnceLock 首写胜出——gateway 生命周期单发语义）。
pub fn set_experts_bridge(bridge: Arc<dyn ExpertsBridge>) {
    let _ = EXPERTS_BRIDGE.set(bridge);
}

fn require_bridge() -> Result<Arc<dyn ExpertsBridge>, String> {
    EXPERTS_BRIDGE
        .get()
        .cloned()
        .ok_or_else(|| "experts 子系统未装配（本进程未启用专家子代理面）".to_string())
}

/// 桥只读访问（跨 handler 消费：persona.shop.hire_expert 经它落专家盘）。
pub(crate) fn experts_bridge() -> Result<std::sync::Arc<dyn ExpertsBridge>, String> {
    require_bridge()
}

pub struct ExpertsHandler;

#[async_trait::async_trait]
impl ModuleHandler for ExpertsHandler {
    fn module_name(&self) -> &str {
        "experts"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "get",
            "create",
            "update",
            "remove",
            "set_enabled",
            "config_get",
            "config_set",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let bridge = require_bridge()?;
        let data = data.unwrap_or(serde_json::Value::Null);
        match cmd {
            "list" => Ok(Some(list_impl(bridge.as_ref()))),
            "get" => Ok(Some(get_impl(bridge.as_ref(), &data)?)),
            "create" => Ok(Some(create_impl(bridge.as_ref(), &data)?)),
            "update" => Ok(Some(update_impl(bridge.as_ref(), &data)?)),
            "remove" => {
                remove_impl(bridge.as_ref(), &data).map(|_| Some(serde_json::json!({ "ok": true })))
            }
            "set_enabled" => Ok(Some(set_enabled_impl(bridge.as_ref(), &data)?)),
            // 模式策略面（2026-11-13 总开关 + 自动降级）：get = flat 三键
            // 投影；set = 部分更新（raw patch 原子写 + opt-in 后台日志）。
            "config_get" => Ok(Some(bridge.config_get())),
            "config_set" => Ok(Some(bridge.config_set(&data)?)),
            _ => Err(format!("unknown command: experts.{}", cmd)),
        }
    }
}

// ---------------------------------------------------------------------------
// 纯函数本体（测试直注 bridge，绕全局槽 OnceLock 首胜语义——
// plugins_wasm list_impl 同款形态：并行用例共享槽 + TempDir 落盘即删，
// 后到者会读到空目录）。
// ---------------------------------------------------------------------------

fn list_impl(b: &dyn ExpertsBridge) -> serde_json::Value {
    b.list()
}

fn key_of(data: &serde_json::Value) -> Result<String, String> {
    data.get("key")
        .and_then(|k| k.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing key（专家 slug 或显示名）".to_string())
}

fn get_impl(b: &dyn ExpertsBridge, data: &serde_json::Value) -> Result<serde_json::Value, String> {
    let key = key_of(data)?;
    if key == DEFAULT_KEY {
        return Ok(serde_json::json!({
            "key": DEFAULT_KEY,
            "name": b.default_name(),
            "is_default": true,
        }));
    }
    b.get(&key)
}

fn create_impl(
    b: &dyn ExpertsBridge,
    data: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    if data.get("name").and_then(|n| n.as_str()).map(str::trim) == Some(DEFAULT_KEY) {
        return Err(format!(
            "'{DEFAULT_KEY}' 是默认子代理保留键，不能作为专家名"
        ));
    }
    b.create(data)
}

/// 默认卡字段闸：默认子代理唯一可编辑字段 = 显示名。其余字段（summary/
/// tags/model/enabled/prompt）在场即诚实拒绝——它执行默认子代理形态
/// （无独立提示词/无模型覆盖/无开关），不是专家记录。
fn default_card_editable_fields_ok(data: &serde_json::Value) -> Result<(), String> {
    for f in ["summary", "tags", "model", "enabled", "prompt"] {
        if data.get(f).map(|v| !v.is_null()).unwrap_or(false) {
            return Err(format!(
                "默认子代理仅支持改名：'{f}' 不可编辑（默认卡执行默认子代理形态，无独立字段）"
            ));
        }
    }
    Ok(())
}

fn update_impl(
    b: &dyn ExpertsBridge,
    data: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let key = key_of(data)?;
    if key == DEFAULT_KEY {
        let name = data
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("默认子代理改名需要 name 字段（非空）")?;
        default_card_editable_fields_ok(data)?;
        return b.set_default_name(name);
    }
    b.update(&key, data)
}

fn remove_impl(b: &dyn ExpertsBridge, data: &serde_json::Value) -> Result<(), String> {
    let key = key_of(data)?;
    if key == DEFAULT_KEY {
        return Err("默认子代理不可删除（出厂常驻，仅可改名）".to_string());
    }
    b.remove(&key)
}

fn set_enabled_impl(
    b: &dyn ExpertsBridge,
    data: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let key = key_of(data)?;
    let enabled = data
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or("missing enabled（boolean）")?;
    if key == DEFAULT_KEY {
        return Err("默认子代理无启停开关（常驻可用）".to_string());
    }
    b.set_enabled(&key, enabled)
}

#[cfg(test)]
mod tests;
