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
            "cors.list" => self.cors_list(ctx),
            "cors.add" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_add(ctx, &origin)
            }
            "cors.remove" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_remove(ctx, &origin)
            }
            "cors.toggle" => {
                let data = data.ok_or("missing data")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing or invalid 'enabled' field")?;
                self.cors_toggle(ctx, enabled)
            }
            _ => Err(format!("unknown command: config.{}", cmd)),
        }
    }
}

/// config.json 路径（home 下）。pub(crate)：skins handler 的 set_active
/// 写 `ui.skin` 复用同一装配（live 优先语义统一定义）。
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
/// 裸组合 load+save 编译不过（同类横扫二期收口）。
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
    let raw = nemesis_utils::fsutil::read_config_string(&path)
        .map_err(|e| format!("read config.json: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("invalid config.json: {e}"))
}

/// RAW JSON 落盘 + 进程全局 ConfigStore 对账（typed readers 走 load_live，
/// 不 reload 会一直供着落盘前快照）。update_raw_config 的写半边。
pub(crate) fn write_raw_config(home: &str, cfg: &serde_json::Value) -> Result<(), String> {
    let path = config_path(home);
    let out =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config.json: {e}"))?;
    // 统一原子写入（config.json 含 API key）。
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
/// 「文件缺失从空对象起」）。消费方：connectors.set_master（总闸——
/// 新装配态也允许一键落键，不因缺母版拒绝）。唯一消费方在 connectors
/// feature 闸后——helper 同闸，防最小 feature 集下 dead_code 警告。
#[cfg(feature = "connectors")]
pub(crate) fn update_raw_config_seeded<T>(
    home: &str,
    f: impl FnOnce(&mut serde_json::Value) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let mut cfg = match nemesis_utils::fsutil::read_config_string(config_path(home)) {
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
        let s = nemesis_utils::fsutil::read_config_string(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
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
    // 统一原子写入（家族配置文件含 token/key 类敏感值）。
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
        // get 响应是脱敏形态（api_key/token 等掩码为 `****` 串）：UI 回存
        // 时掩码值必须还原为存量原值，绝不落盘掩码——channels.save 同款
        // 纪律，且在锁内做（杜绝与并发写之间的丢更新）。漏接实证
        // 编辑器原样保存把 model_list[].api_key 覆盖成
        // '****'，真 key 丢失。
        update_config(home, |config| {
            let existing = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize existing config: {}", e))?;
            let mut incoming = data.clone();
            crate::handlers::restore_masked_named_fields(&mut incoming, &existing)?;
            *config = serde_json::from_value(incoming.clone())
                .map_err(|e| format!("invalid config data: {}", e))?;
            // 与 set_field 同款 round-trip 校验（整档版）：typed reparse 会静默
            // 忽略未建模键（serde 默认丢弃未知字段），曾出现「保存成功」但键蒸
            // 发的静默丢数据。这里对 incoming 递归收集「信息承载叶子」在回读中
            // 的丢失/归一——空值叶子豁免（typed config 大量 skip_serializing_if
            // 空段，空值消失是无损语义）。有丢失 = loud 拒绝、闭包 Err 不落盘。
            let reserialized = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize config: {}", e))?;
            let dropped = collect_dropped_paths(&incoming, &reserialized);
            if !dropped.is_empty() {
                let shown = dropped
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = if dropped.len() > 10 {
                    format!(" (+{} more)", dropped.len() - 10)
                } else {
                    String::new()
                };
                return Err(format!(
                    "{} config key(s) would be silently dropped by the typed config: {}{}; \
                     use config.set_field per key, or extend the typed model first",
                    dropped.len(),
                    shown,
                    more
                ));
            }
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
            // persisting nothing (真机验收发现的诚实性缺口， loud 拒绝要求).
            let reserialized = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize config: {}", e))?;
            match json_path_get(&reserialized, path) {
                None => return Err(format!("unknown config field: {}", path)),
                Some(actual) if actual != value && !numeric_normalized_equal(actual, value) => {
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

    /// CORS manager 句柄取用：None = gateway 未注入（headless/relay/测试
    /// 装配）→ 诚实报错，不惰性创建（默认 cors.json 只由 CLI `cors add`
    /// 落盘，WSAPI 面保持与 restrictive 启动态一致）。
    fn cors_manager(
        ctx: &RequestContext,
    ) -> Result<std::sync::Arc<crate::cors::CORSManager>, String> {
        ctx.state
            .cors_manager
            .clone()
            .ok_or_else(|| "CORS manager not connected (gateway did not inject)".to_string())
    }

    /// 生效语义：CorsLayer 是 gateway 启动时静态构建，这里的写盘改动
    /// **重启后生效**（与 CLI `cors` 子命令同语义）。
    fn cors_list(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let mgr = Self::cors_manager(ctx)?;
        let cfg = mgr.config();
        Ok(Some(serde_json::json!({
            "origins": mgr.list_origins(),
            "allowed_origins": cfg.allowed_origins,
            "allowed_methods": cfg.allowed_methods,
            "allowed_headers": cfg.allowed_headers,
            "allow_credentials": cfg.allow_credentials,
            "max_age": cfg.max_age,
            "allow_localhost": cfg.allow_localhost,
            "development_mode": cfg.development_mode,
            "allowed_cdn_domains": cfg.allowed_cdn_domains,
            "allow_no_origin": cfg.allow_no_origin,
        })))
    }

    fn cors_add(
        &self,
        ctx: &RequestContext,
        origin: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        // 校验镜像 CLI `cors add`（commands/cors.rs）：非 CDN 形态必须带
        // scheme 前缀。
        if !origin.starts_with("http://") && !origin.starts_with("https://") {
            return Err(format!(
                "Invalid origin URL '{}'. Must start with http:// or https://",
                origin
            ));
        }
        let mgr = Self::cors_manager(ctx)?;
        mgr.add_origin(origin)
            .map_err(|e| format!("failed to add origin: {}", e))?;
        Ok(Some(serde_json::json!({ "added": true, "origin": origin })))
    }

    fn cors_remove(
        &self,
        ctx: &RequestContext,
        origin: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let mgr = Self::cors_manager(ctx)?;
        mgr.remove_origin(origin)
            .map_err(|e| format!("failed to remove origin: {}", e))?;
        Ok(Some(
            serde_json::json!({ "removed": true, "origin": origin }),
        ))
    }

    fn cors_toggle(
        &self,
        ctx: &RequestContext,
        enabled: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        let mgr = Self::cors_manager(ctx)?;
        mgr.set_development_mode(enabled)
            .map_err(|e| format!("failed to set development mode: {}", e))?;
        Ok(Some(
            serde_json::json!({ "toggled": true, "development_mode": enabled }),
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
        // 父节点是 scalar（bool/string/number）时，
        // serde_json IndexMut 对非 object 键访问直接 panic——路径拼错/
        // 类型撞上（如 `…enabled.x`）会崩掉 WS handler 任务。显式检查，
        // loud Err 替代 panic（Null 保留 auto-vivify 语义）。
        if !current.is_object() && !current.is_null() {
            return Err(format!(
                "config field `{path}`: parent node is not an object"
            ));
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

/// 数值归一等价：typed reparse 会把 JSON 整数合法收敛到浮点字段
/// （`max_order_qty: f64` 收 `1` → 重序列化 `1.0`），serde_json 的
/// `Number(1) != Number(1.0)`（PosInt vs Float）——语义相同的归一不能
/// 误判成 round-trip 失败。仅两侧都是数字且 f64 值相等时放行。
fn numeric_normalized_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    a.is_number() && b.is_number() && a.as_f64() == b.as_f64()
}

/// 「空值叶子」：值为 null / false / 空串的标量。typed config 大量
/// `skip_serializing_if` 空段（空 map/空串/false/None），空值回读消失是
/// 无损语义，不构成丢键。
fn is_empty_leaf(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// 子树里是否含信息承载叶子（非空标量或非空集合）。
fn subtree_has_payload(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(map) => map.values().any(subtree_has_payload),
        serde_json::Value::Array(arr) => arr.iter().any(subtree_has_payload),
        _ => !is_empty_leaf(v),
    }
}

fn join_path(path: &str, seg: &str) -> String {
    if path.is_empty() {
        seg.to_string()
    } else {
        format!("{path}.{seg}")
    }
}

/// 递归收集 `incoming` 中未在 typed 回读 `re` 里保真的「信息承载叶子」
/// dot-path（与 [`json_path_get`] 同款寻址：数组段为数字下标）。对象按键
/// 配对、数组按下标配对；叶子空值豁免（见 [`is_empty_leaf`]），其余叶子
/// 回读缺失或不等 = 收集。用于 config.save 的 round-trip 校验，与
/// set_field 的 "did not round-trip" 严格语义一致。
fn collect_dropped_paths(incoming: &serde_json::Value, re: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_dropped(incoming, re, "", &mut out);
    out
}

fn walk_dropped(
    inc: &serde_json::Value,
    re: &serde_json::Value,
    path: &str,
    out: &mut Vec<String>,
) {
    match inc {
        serde_json::Value::Object(map) => {
            for (key, iv) in map {
                let child = join_path(path, key);
                match re.get(key) {
                    Some(rv) => walk_dropped(iv, rv, &child, out),
                    // 整棵子树被 skip：仅当子树里有信息承载叶子才算丢键。
                    None => {
                        if subtree_has_payload(iv) {
                            out.push(child);
                        }
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            if let Some(rarr) = re.as_array() {
                for (i, iv) in arr.iter().enumerate() {
                    let child = join_path(path, &i.to_string());
                    match rarr.get(i) {
                        Some(rv) => walk_dropped(iv, rv, &child, out),
                        None => {
                            if subtree_has_payload(iv) {
                                out.push(child);
                            }
                        }
                    }
                }
            } else if subtree_has_payload(inc) {
                out.push(path.to_string());
            }
        }
        _ => {
            if is_empty_leaf(inc) {
                return;
            }
            if re != inc {
                out.push(path.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests;
