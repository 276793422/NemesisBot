//! Config handler — get/save/set_field config.json + CORS commands.

use crate::handlers::require_home;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ConfigHandler {
    _priv: (),
}

impl Default for ConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ConfigHandler {
    fn module_name(&self) -> &str {
        "config"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "get",
            "save",
            "set_field",
            "cors.list",
            "cors.add",
            "cors.remove",
            "cors.toggle",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let home = require_home(ctx)?;
        match cmd {
            "get" => self.get(home),
            "save" => {
                let data = data.ok_or("missing data")?;
                self.save(home, &data)
            }
            "set_field" => {
                let data = data.ok_or("missing data")?;
                let path = crate::handlers::get_str(&data, "path")?;
                let value = data
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                self.set_field(home, &path, &value)
            }
            "cors.list" => self.cors_list(),
            "cors.add" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_add(&origin)
            }
            "cors.remove" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_remove(&origin)
            }
            "cors.toggle" => {
                let data = data.ok_or("missing data")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing or invalid 'enabled' field")?;
                self.cors_toggle(enabled)
            }
            _ => Err(format!("unknown command: config.{}", cmd)),
        }
    }
}

/// config.json 路径（home 下）。pub(crate)：skins handler 的 set_active
/// 写 `ui.skin` 复用同一装配（live 优先语义单一真相源）。
pub(crate) fn config_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("config.json")
}

pub(crate) fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    nemesis_config::load_config(&config_path(home))
        .map_err(|e| format!("failed to load config: {}", e))
}

/// config.json RMW 事务唯一入口（typed Config 视图）——写串行锁在函数内
/// 取得，罩「live 优先 load → 闭包改 → 落盘」全程；闭包返回 Err 不落盘
/// （abort 语义）。调用方结构上不可能忘锁：落盘原语要求传入锁 guard，
/// 裸组合 load+save 编译不过（同类横扫二期收口，2026-10-13）。
/// 纪律：闭包内不得再调任何 update_* / [`nemesis_config::config_write_lock`]
/// （Mutex 非重入，嵌套即死锁）。
pub(crate) fn update_config<T>(
    home: &str,
    f: impl FnOnce(&mut nemesis_config::Config) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let mut cfg = load_config(home)?;
    let out = f(&mut cfg)?;
    save_config_to_disk(home, &mut cfg, &_serial)?;
    Ok(out)
}

/// 读 config.json 为 RAW JSON（保留 typed `Config` 不建模的键——model_list
/// 条目上的 tier/size/real_name/context_window extras）。update_raw_config
/// 的读半边；单独 pub(crate) 供只读面（attribute editor）复用。
pub(crate) fn read_raw_config(home: &str) -> Result<serde_json::Value, String> {
    let path = config_path(home);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read config.json: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("invalid config.json: {e}"))
}

/// RAW JSON 落盘 + 进程全局 ConfigStore 对账（typed readers 走 load_live，
/// 不 reload 会一直供着落盘前快照）。update_raw_config 的写半边。
pub(crate) fn write_raw_config(home: &str, cfg: &serde_json::Value) -> Result<(), String> {
    let path = config_path(home);
    let out =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config.json: {e}"))?;
    // REL-002：统一原子写入（config.json 含 API key）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write config.json: {e}"))?;
    if let Some(store) = nemesis_config::global() {
        store
            .reload()
            .map_err(|e| format!("reload live config store: {e}"))?;
    }
    Ok(())
}

/// config.json RMW 事务入口（raw JSON 视图）：model_list 等含未类型化
/// extras 的段必须保真回写——typed round-trip 会丢它们。锁语义同
/// [`update_config`]。
pub(crate) fn update_raw_config<T>(
    home: &str,
    f: impl FnOnce(&mut serde_json::Value) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let mut cfg = read_raw_config(home)?;
    let out = f(&mut cfg)?;
    write_raw_config(home, &cfg)?;
    Ok(out)
}

/// [`update_raw_config`] 的缺失容忍变体：config.json 不存在时从空对象起
/// （NotFound 以外读/解析失败仍 Err；语义对齐 update_json_file 家族的
/// 「文件缺失从空对象起」）。消费方：connectors.set_master（B4 总闸——
/// 新装配态也允许一键落键，不因缺母版拒绝）。唯一消费方在 connectors
/// feature 闸后——helper 同闸，防最小 feature 集下 dead_code 警告。
#[cfg(feature = "connectors")]
pub(crate) fn update_raw_config_seeded<T>(
    home: &str,
    f: impl FnOnce(&mut serde_json::Value) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let mut cfg = match std::fs::read_to_string(config_path(home)) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("invalid config.json: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("read config.json: {e}")),
    };
    let out = f(&mut cfg)?;
    write_raw_config(home, &cfg)?;
    Ok(out)
}

/// 任意 JSON 配置家族文件的事务入口（config.cluster.json / config.forge.json /
/// registry.json 等，与 config.json 同族共用一把写串行锁）：锁罩「读→闭包改→
/// 原子写」全程；文件缺失从空对象起（seed 默认是闭包的事）；读/解析失败
/// = Err 不落盘。闭包 Err 不落盘（abort 语义）。
#[cfg(any(feature = "cluster", feature = "forge"))]
pub(crate) fn update_json_file<T>(
    path: &std::path::Path,
    f: impl FnOnce(&mut serde_json::Value) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let mut v = if path.exists() {
        let s =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        serde_json::from_str(&s).map_err(|e| format!("parse {}: {e}", path.display()))?
    } else {
        serde_json::json!({})
    };
    let out = f(&mut v)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create config dir {}: {e}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(&v)
        .map_err(|e| format!("serialize {}: {e}", path.display()))?;
    // REL-002：统一原子写入（家族配置文件含 token/key 类敏感值）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(out)
}

/// config.json 落盘原语。**要求调用方已持有写串行锁**（proof-of-lock：
/// guard 作参数，裸调用编译不过）——update_config 是 blessed 入口；多文件/
/// 长跨度等不适合闭包事务的逃逸臂（memory config_set 双文件、cluster 身份
/// 双写）经本原语落盘，锁的在场由类型系统强制。
pub(crate) fn save_config_to_disk(
    home: &str,
    config: &mut nemesis_config::Config,
    _write_guard: &parking_lot::MutexGuard<'static, ()>,
) -> Result<(), String> {
    if let Some(r) = nemesis_config::save_live(config.clone()) {
        return r.map_err(|e| format!("failed to save config: {}", e));
    }
    nemesis_config::save_config(&config_path(home), config)
        .map_err(|e| format!("failed to save config: {}", e))
}

impl ConfigHandler {
    fn get(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        let mut json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize config: {}", e))?;
        // Mask sensitive fields
        sanitize_config(&mut json);
        Ok(Some(json))
    }

    fn save(
        &self,
        home: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        // 编辑器整档保存语义：闭包整体替换（锁在 update_config 内）。
        update_config(home, |config| {
            *config = serde_json::from_value(data.clone())
                .map_err(|e| format!("invalid config data: {}", e))?;
            Ok(())
        })?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn set_field(
        &self,
        home: &str,
        path: &str,
        value: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        update_config(home, |config| {
            let mut json = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize config: {}", e))?;

            set_json_path(&mut json, path, value.clone())?;

            *config = serde_json::from_value(json)
                .map_err(|e| format!("invalid config after field update: {}", e))?;

            // Typed reparse silently drops unknown keys (serde ignores unrecognized
            // fields). Verify the field survived the round-trip before claiming
            // success — otherwise an unknown path would answer `updated:true` while
            // persisting nothing (R1 真机验收发现的诚实性缺口，G6 loud 拒绝要求).
            let reserialized = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize config: {}", e))?;
            match json_path_get(&reserialized, path) {
                None => return Err(format!("unknown config field: {}", path)),
                Some(actual) if actual != value => {
                    return Err(format!(
                        "config field {} did not round-trip (value rejected or normalized)",
                        path
                    ));
                }
                _ => {}
            }
            Ok(())
        })?;
        Ok(Some(serde_json::json!({ "updated": true, "path": path })))
    }

    fn cors_list(&self) -> Result<Option<serde_json::Value>, String> {
        // CORSManager access would need to go through AppState
        // For now, read from cors.json directly
        Ok(Some(
            serde_json::json!({ "origins": [], "message": "CORS manager not connected" }),
        ))
    }

    fn cors_add(&self, _origin: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(
            serde_json::json!({ "added": false, "message": "CORS manager not connected" }),
        ))
    }

    fn cors_remove(&self, _origin: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(
            serde_json::json!({ "removed": false, "message": "CORS manager not connected" }),
        ))
    }

    fn cors_toggle(&self, _enabled: bool) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(
            serde_json::json!({ "toggled": false, "message": "CORS manager not connected" }),
        ))
    }
}

/// Mask sensitive fields in a config JSON object.
fn sanitize_config(json: &mut serde_json::Value) {
    if let Some(obj) = json.as_object_mut() {
        for (key, value) in obj.iter_mut() {
            if crate::handlers::is_sensitive_field(key) {
                if let Some(s) = value.as_str()
                    && !s.is_empty()
                {
                    *value = serde_json::Value::String(crate::handlers::mask_sensitive(s));
                }
            } else {
                sanitize_config(value);
            }
        }
    } else if let Some(arr) = json.as_array_mut() {
        for item in arr.iter_mut() {
            sanitize_config(item);
        }
    }
}

/// Set a value at a dot-separated path in a JSON object. Array segments are
/// addressed by numeric index (e.g. `model_list.0.api_key`); out-of-bounds
/// indices are a loud error (no append semantics).
fn set_json_path(
    json: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".to_string());
    }
    let parts: Vec<&str> = path.split('.').collect();
    let last = parts.len() - 1;
    let mut current = json;
    for (i, part) in parts.iter().enumerate() {
        // 数组节点：段必须解析为 usize 下标，越界显式报错。
        if current.is_array() {
            let idx: usize = part.parse().map_err(|_| {
                format!("path segment `{part}` is not a valid array index ({path})")
            })?;
            if current.get(idx).is_none() {
                return Err(format!("array index out of bounds: {path}"));
            }
            if i == last {
                current[idx] = value;
                return Ok(());
            }
            current = &mut current[idx];
            continue;
        }
        if i == last {
            current[part] = value;
            return Ok(());
        }
        if current[part].is_null() {
            current[part] = serde_json::json!({});
        }
        current = &mut current[part];
    }
    Ok(())
}

/// Resolve a dot path on a JSON value; None if any segment is missing.
/// Array nodes take numeric-index segments (mirror of [`set_json_path`]).
fn json_path_get<'a>(json: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = json;
    for part in path.split('.') {
        if current.is_array() {
            current = current.get(part.parse::<usize>().ok()?)?;
        } else {
            current = current.get(part)?;
        }
    }
    Some(current)
}
