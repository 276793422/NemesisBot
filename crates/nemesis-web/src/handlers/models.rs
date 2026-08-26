//! Models handler — list/add/delete/set_default/test model configurations.

use crate::handlers::{mask_sensitive, require_home};
#[cfg(feature = "forge")]
use crate::llm_bridge::ForgeProviderBridge;
use crate::llm_bridge::ProviderAdapter;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;
use std::sync::Arc;

pub struct ModelsHandler {
    _priv: (),
}

impl ModelsHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ModelsHandler {
    fn module_name(&self) -> &str {
        "models"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let home = require_home(ctx)?;
        match cmd {
            "list" => self.list(home),
            "add" => {
                let data = data.ok_or("missing data")?;
                self.add(home, &data)
            }
            "delete" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.delete(home, &name)
            }
            "set_default" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.set_default(home, &name, ctx)
            }
            "test" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.test(home, &name)
            }
            // P3-2 (2026-08-24 UI entry gap): model attribute editor.
            "update_field" => {
                let data = data.ok_or("missing data")?;
                self.update_field(home, &data)
            }
            "catalog_info" => self.catalog_info(home),
            "catalog_update" => self.catalog_update(home).await,
            _ => Err(format!("unknown command: models.{}", cmd)),
        }
    }
}

fn config_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("config.json")
}

fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    let path = config_path(home);
    nemesis_config::load_config(&path).map_err(|e| format!("failed to load config: {}", e))
}

/// DISABLED (P3-2, 2026-08-24): typed save is no longer called — every
/// `model_list` mutation here goes through raw RMW (`write_raw_config`)
/// because a typed round-trip DROPS the tier/size/real_name/context_window
/// extras. Kept (not deleted) per the code-change discipline: safe to revive
/// only for sections whose keys the typed `Config` fully models.
/// To restore: route the mutation through this instead of `write_raw_config`.
#[allow(dead_code)]
fn save_config(home: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    if let Some(r) = nemesis_config::save_live(config.clone()) {
        return r.map_err(|e| format!("failed to save config: {}", e));
    }
    let path = config_path(home);
    nemesis_config::save_config(&path, config).map_err(|e| format!("failed to save config: {}", e))
}

/// Read config.json as RAW JSON (preserves keys the typed `Config` does not
/// model — tier/size/real_name/context_window extras on `model_list[]`
/// entries). Every `model_list` mutation in this handler must go through a
/// raw read-modify-write + [`write_raw_config`]: the CLI writes the extras
/// via raw RMW precisely because a typed round-trip DROPS them
/// (nemesisbot/src/commands/model.rs `update_model_entry`).
fn read_raw_config(home: &str) -> Result<serde_json::Value, String> {
    let path = config_path(home);
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read config.json: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse config.json: {e}"))
}

/// Write config.json as RAW JSON, then reconcile the process-global
/// ConfigStore (if installed) from disk — typed readers via `load_live`
/// would otherwise keep serving the pre-write snapshot.
fn write_raw_config(home: &str, cfg: &serde_json::Value) -> Result<(), String> {
    let path = config_path(home);
    let out =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config.json: {e}"))?;
    std::fs::write(&path, out).map_err(|e| format!("write config.json: {e}"))?;
    if let Some(store) = nemesis_config::global() {
        store
            .reload()
            .map_err(|e| format!("reload live config store: {e}"))?;
    }
    Ok(())
}

impl ModelsHandler {
    fn list(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        // 默认模型以 agents.defaults.llm 为权威（启动 get_effective_llm 读的就是它），
        // 不能用 model_list[0] 位置判——CLI 的 `model add --default` 把新模型追加到
        // 末尾、只改 agents.defaults.llm，位置判会把旧模型误标为默认，dashboard 就
        // 显示错了。default_llm 可能是 model_name / vendor/model 串 / 别名（CLI 设别名）。
        let default_llm = config.agents.defaults.llm.clone();
        let first_name = config
            .model_list
            .first()
            .map(|m| m.model_name.clone())
            .unwrap_or_default();
        // P3-2: tier/size/real_name/context_window live as RAW-JSON extra keys
        // the typed ModelConfig deliberately doesn't model (typed round-trip
        // would drop them) — read them from the file for the attribute editor.
        let raw_by_name = read_raw_model_entries(home);
        // P3-2: attach the models.dev catalog hit (same exact-key lookup as
        // `model add` auto-fill) so the UI can show the catalog-provided
        // context_window as a fillable default.
        let catalog = read_catalog(home);
        let models: Vec<_> = config
            .model_list
            .iter()
            .map(|m| {
                let alias = m.model.split('/').next_back().unwrap_or("");
                let is_default = if default_llm.is_empty() {
                    // 回退：老配置没显式默认时，沿用 list[0] 位置默认，保持旧行为。
                    m.model_name == first_name
                } else {
                    m.model_name == default_llm
                        || m.model == default_llm
                        || (!alias.is_empty() && alias == default_llm)
                };
                let raw = raw_by_name
                    .as_ref()
                    .and_then(|map| map.get(&m.model_name).cloned())
                    .unwrap_or(serde_json::Value::Null);
                let catalog_match = catalog.as_ref().and_then(|cat| {
                    cat.entries.iter().find(|e| e.key == m.model).map(|e| {
                        serde_json::json!({
                            "context_window": e.context_window,
                            "max_output_tokens": e.max_output_tokens,
                            "family": e.family,
                        })
                    })
                });
                serde_json::json!({
                    "model_name": m.model_name,
                    "model": m.model,
                    "api_base": m.api_base,
                    "api_key": if m.api_key.is_empty() { String::new() } else { mask_sensitive(&m.api_key) },
                    "proxy": m.proxy,
                    "is_default": is_default,
                    // Raw extras (absent in file → null; frontend treats null as unset).
                    "model_tier": raw.get("model_tier").cloned().unwrap_or(serde_json::Value::Null),
                    "reasoning_effort": raw.get("reasoning_effort").cloned().unwrap_or(serde_json::Value::Null),
                    "model_size_b": raw.get("model_size_b").cloned().unwrap_or(serde_json::Value::Null),
                    "real_name": raw.get("real_name").cloned().unwrap_or(serde_json::Value::Null),
                    "context_window": raw.get("context_window").cloned().unwrap_or(serde_json::Value::Null),
                    "catalog_match": catalog_match,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "models": models })))
    }

    fn add(
        &self,
        home: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let model_name = crate::handlers::get_str(data, "name")?;
        let model = crate::handlers::get_str(data, "model")?;
        let api_key = crate::handlers::get_str(data, "key")?;
        let api_base = crate::handlers::get_opt_str(data, "base_url").unwrap_or_default();
        let proxy = crate::handlers::get_opt_str(data, "proxy").unwrap_or_default();

        // Raw RMW (NOT typed save): preserves the tier/size/real_name/
        // context_window extras other entries may carry — the typed
        // ModelConfig does not model them, so a typed round-trip would drop
        // them (same reason the CLI `model add` is raw JSON).
        let mut cfg = read_raw_config(home)?;
        let list = cfg
            .get_mut("model_list")
            .and_then(|v| v.as_array_mut())
            .ok_or("config.json has no model_list")?;
        if list
            .iter()
            .any(|m| m.get("model_name").and_then(|v| v.as_str()) == Some(&model_name))
        {
            return Err(format!("model '{}' already exists", model_name));
        }

        let mut entry = serde_json::json!({
            // Same field set the old typed push produced (all-default empties).
            "model_name": model_name,
            "model": model,
            "api_base": api_base,
            "api_key": api_key,
            "proxy": proxy,
            "auth_method": "",
            "connect_mode": "",
            "workspace": "",
            "reasoning_effort": "",
            // CLI `model add` parity: tag auto-detect tier explicitly.
            "model_tier": "auto",
        });
        // U16 parity: auto-fill context_window / max_output_tokens from the
        // models.dev catalog cache on an exact-key hit (silent no-op without
        // a cache — `model catalog-update` / the dashboard button fills it).
        if let Some(cat) = read_catalog(home) {
            if let Some(hit) = cat.entries.iter().find(|e| e.key == model) {
                entry["context_window"] = serde_json::Value::Number(hit.context_window.into());
                if let Some(mot) = hit.max_output_tokens {
                    entry["max_output_tokens"] = serde_json::Value::Number(mot.into());
                }
            }
        }

        list.push(entry);
        write_raw_config(home, &cfg)?;
        Ok(Some(
            serde_json::json!({ "added": true, "name": model_name }),
        ))
    }

    fn delete(&self, home: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        // Raw RMW: a typed round-trip would drop the tier/size/real_name/
        // context_window extras the surviving entries carry.
        let mut cfg = read_raw_config(home)?;

        // 守卫：禁止删除当前默认模型。否则 agents.defaults.llm 变成悬空引用，
        // 下次启动 get_effective_llm → resolve_model_config 找不到模型直接失败。
        // default_llm 可能是 model_name、vendor/model 串或别名（CLI 设的是别名），
        // 故把目标模型的所有标识都拿来比对。同时兜住 list[0] 这个 dashboard 位置默认。
        let default_llm = cfg
            .get("agents")
            .and_then(|a| a.get("defaults"))
            .and_then(|d| d.get("llm"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let list = cfg
            .get_mut("model_list")
            .and_then(|v| v.as_array_mut())
            .ok_or("config.json has no model_list")?;
        let first_name = list
            .first()
            .and_then(|m| m.get("model_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(m) = list.iter().find(|m| {
            m.get("model_name").and_then(|v| v.as_str()) == Some(name)
        }) {
            let model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let alias = model.split('/').next_back().unwrap_or("");
            let is_default = name == default_llm
                || name == first_name
                || model == default_llm
                || (!alias.is_empty() && alias == default_llm);
            if is_default {
                return Err(format!(
                    "cannot delete default model '{}'. Switch the default to another model first.",
                    name
                ));
            }
        }

        let before = list.len();
        list.retain(|m| m.get("model_name").and_then(|v| v.as_str()) != Some(name));
        if list.len() == before {
            return Err(format!("model '{}' not found", name));
        }
        write_raw_config(home, &cfg)?;
        Ok(Some(serde_json::json!({ "deleted": true, "name": name })))
    }

    fn set_default(
        &self,
        home: &str,
        name: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // Raw RMW: a typed round-trip would drop the tier/size/real_name/
        // context_window extras entries carry.
        let mut cfg = read_raw_config(home)?;
        let list = cfg
            .get_mut("model_list")
            .and_then(|v| v.as_array_mut())
            .ok_or("config.json has no model_list")?;
        let idx = list
            .iter()
            .position(|m| m.get("model_name").and_then(|v| v.as_str()) == Some(name))
            .ok_or_else(|| format!("model '{}' not found", name))?;
        let entry = list.remove(idx);
        list.insert(0, entry.clone());
        // 同步 agents.defaults.llm：启动时 get_effective_llm 只读这个字段、不看
        // model_list 顺序。不写这行，dashboard 切模型只在运行时生效（provider 已换），
        // 重启后回退到旧模型；若旧模型随后被删，启动会因 "model not found" 失败。
        {
            let obj = cfg
                .as_object_mut()
                .ok_or("config.json root is not an object")?;
            let agents = obj
                .entry("agents")
                .or_insert_with(|| serde_json::json!({}));
            let agents_obj = agents
                .as_object_mut()
                .ok_or("config.json agents is not an object")?;
            let defaults = agents_obj
                .entry("defaults")
                .or_insert_with(|| serde_json::json!({}));
            let defaults_obj = defaults
                .as_object_mut()
                .ok_or("config.json agents.defaults is not an object")?;
            defaults_obj.insert(
                "llm".to_string(),
                serde_json::Value::String(name.to_string()),
            );
        }
        write_raw_config(home, &cfg)?;

        // Runtime provider swap so the change takes effect immediately
        // (fields read from the raw entry).
        let g = |k: &str| {
            entry
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let model_id = g("model");
        let api_base_raw = g("api_base");
        let api_key = g("api_key");
        let connect_mode = g("connect_mode");

        if let Some(ref agent_loop) = ctx.state.agent_loop.read().as_ref() {
            let api_base = if api_base_raw.is_empty() {
                nemesis_config::get_default_api_base(&nemesis_config::infer_provider_from_model(
                    &model_id,
                ))
                .to_string()
            } else {
                api_base_raw.clone()
            };

            let factory_cfg = nemesis_providers::factory::FactoryConfig {
                llm_ref: model_id.clone(),
                api_key: api_key.clone(),
                api_base,
                workspace: String::new(),
                connect_mode: connect_mode.clone(),
                account_id: String::new(),
                headers: std::collections::HashMap::new(),
            };
            match nemesis_providers::factory::create_provider(&factory_cfg) {
                Ok(provider) => {
                    let adapter = Arc::new(ProviderAdapter::new(
                        provider.clone(),
                        model_id.clone(),
                    ));
                    agent_loop.set_provider_and_model(adapter, model_id.clone());
                    tracing::info!(model = %model_id, "[Models] Runtime provider swapped");

                    // Sync Forge's LLM provider — set_provider cascades to all subsystems.
                    #[cfg(feature = "forge")]
                    {
                        if let Some(ref forge) = ctx.state.forge {
                            let bridge =
                                ForgeProviderBridge::new(provider.clone(), model_id.clone());
                            forge.set_provider(Arc::new(bridge));
                            tracing::info!(model = %model_id, "[Models] Forge provider updated");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Models] Failed to create provider for runtime swap, config saved anyway");
                }
            }
        }

        Ok(Some(
            serde_json::json!({ "set_default": true, "name": name }),
        ))
    }

    fn test(&self, _home: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        // Stub — actual model testing requires provider integration
        Ok(Some(serde_json::json!({
            "name": name,
            "status": "not_implemented",
            "message": "Model test requires provider integration"
        })))
    }

    /// P3-2 (2026-08-24 UI entry gap): per-field attribute editor for one
    /// `model_list[]` entry.
    ///
    /// tier/size/real_name/context_window are RAW-JSON extra keys the typed
    /// `ModelConfig` does not model — a typed load/save round-trip would
    /// silently DROP them (the same reason the CLI's `model set-tier` writes
    /// raw JSON). So this writes raw read-modify-write, preserving every
    /// sibling key. Validation mirrors the CLI `model set-*` commands.
    /// tier/effort are hot (the agent re-reads config.json per LLM round via
    /// `check_config_reload`); size/real_name feed the `auto` tier resolution
    /// on that same re-read.
    fn update_field(
        &self,
        home: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        let field = crate::handlers::get_str(data, "field")?;
        let value = data.get("value").ok_or("missing value")?;

        let normalized: serde_json::Value = match field.as_str() {
            "model_tier" => {
                let s = value.as_str().ok_or("model_tier must be a string")?;
                let tier: nemesis_types::capability::ModelTier =
                    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(
                        |_| format!("Invalid tier '{s}'. Use one of: auto | mini | normal | big"),
                    )?;
                serde_json::Value::String(tier.to_string())
            }
            "reasoning_effort" => {
                let s = value
                    .as_str()
                    .ok_or("reasoning_effort must be a string")?
                    .to_lowercase();
                if !matches!(s.as_str(), "off" | "low" | "medium" | "high") {
                    return Err(format!(
                        "Invalid effort '{s}'. Use one of: off | low | medium | high"
                    ));
                }
                // "off" clears the field (absent/empty = send nothing) — CLI parity.
                serde_json::Value::String(if s == "off" { String::new() } else { s })
            }
            "model_size_b" | "context_window" => {
                let n = value.as_u64().or_else(|| {
                    value
                        .as_str()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                });
                let n = n
                    .ok_or_else(|| format!("{field} must be a positive number"))?;
                if n == 0 {
                    return Err(format!("{field} must be > 0"));
                }
                serde_json::Value::Number(n.into())
            }
            "real_name" => {
                let s = value.as_str().ok_or("real_name must be a string")?;
                if s.trim().is_empty() {
                    return Err("real_name must not be empty".to_string());
                }
                serde_json::Value::String(s.trim().to_string())
            }
            _ => {
                return Err(format!(
                    "unknown field '{field}'. Supported: model_tier | reasoning_effort | model_size_b | real_name | context_window"
                ))
            }
        };

        let mut cfg = read_raw_config(home)?;
        let list = cfg
            .get_mut("model_list")
            .and_then(|v| v.as_array_mut())
            .ok_or("config.json has no model_list")?;
        let mut updated = false;
        for entry in list.iter_mut() {
            let model_name = entry.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
            let model = entry.get("model").and_then(|v| v.as_str()).unwrap_or("");
            if model_name == name || model == name {
                entry[field.as_str()] = normalized.clone();
                updated = true;
                break;
            }
        }
        if !updated {
            return Err(format!("model '{name}' not found"));
        }
        write_raw_config(home, &cfg)?;
        Ok(Some(serde_json::json!({
            "updated": true, "name": name, "field": field, "value": normalized,
        })))
    }

    /// P3-2: catalog cache status for the models page header (no spawn, no
    /// network — reads `<home>/models_catalog.json` directly; the file shape
    /// matches the CLI's `catalog.rs` cache).
    ///
    /// 真相源：CLI `model catalog-update` 写 `catalog_path(cfg_dir)`，其中
    /// cfg_dir = `config.json` 的父目录 = **home 根**（不是 config/ 子目录
    /// ——2026-08-24 L2 真机断言抓到的读写路径分叉，夹具曾跟着错路径写）。
    fn catalog_info(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let path = std::path::PathBuf::from(home)
            .join("models_catalog.json");
        if !path.exists() {
            return Ok(Some(
                serde_json::json!({ "exists": false, "fetched_at": "", "entries": 0 }),
            ));
        }
        let raw =
            std::fs::read_to_string(&path).map_err(|e| format!("read catalog cache: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("parse catalog cache: {e}"))?;
        Ok(Some(serde_json::json!({
            "exists": true,
            "fetched_at": v.get("fetched_at").and_then(|x| x.as_str()).unwrap_or(""),
            "entries": v.get("entries").and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0),
        })))
    }

    /// P3-2: refresh the models.dev catalog by spawning the CLI
    /// (`nemesisbot model catalog-update`) — the fetch/parse/mirror-fallback
    /// logic lives in the binary's `catalog.rs`, so the subprocess is the
    /// single source of truth (same shape as the sandbox handler's
    /// `run_cli_subcmd`). Note NEMESISBOT_HOME semantics: the CLI JOINS
    /// `.nemesisbot` onto the env value, so we pass the PARENT of home.
    async fn catalog_update(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let home_pb = std::path::PathBuf::from(home);
        let env_home = home_pb.parent().unwrap_or(&home_pb).to_path_buf();
        let exe =
            std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(90),
            tokio::process::Command::new(&exe)
                .arg("model")
                .arg("catalog-update")
                .env("NEMESISBOT_HOME", &env_home)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| "model catalog-update timed out (90s)".to_string())?
        .map_err(|e| format!("spawn model catalog-update: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "model catalog-update failed (status {}): {}",
                output.status,
                stderr.trim()
            ));
        }
        // Report the refreshed cache state (catalog_info reads the file the
        // child just wrote — home-side path, not the env-parent path).
        let info = self.catalog_info(home)?.unwrap_or(serde_json::Value::Null);
        Ok(Some(info))
    }
}

// --- P3-2 helpers -----------------------------------------------------------

/// Read config.json as raw JSON and index `model_list[]` entries by
/// `model_name`. Returns None when the file is missing/unparseable (callers
/// treat extras as unset).
fn read_raw_model_entries(home: &str) -> Option<std::collections::HashMap<String, serde_json::Value>> {
    let raw = std::fs::read_to_string(config_path(home)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let arr = v.get("model_list")?.as_array()?;
    let mut map = std::collections::HashMap::new();
    for entry in arr {
        if let Some(name) = entry.get("model_name").and_then(|n| n.as_str()) {
            map.insert(name.to_string(), entry.clone());
        }
    }
    Some(map)
}

/// In-memory shape of the CLI's catalog cache (`{version, fetched_at,
/// entries: [{key, context_window, ...}]}`) — only the fields `list` needs.
struct CatalogLite {
    entries: Vec<CatalogLiteEntry>,
}

struct CatalogLiteEntry {
    key: String,
    context_window: u64,
    max_output_tokens: Option<u64>,
    family: Option<String>,
}

fn read_catalog(home: &str) -> Option<CatalogLite> {
    // 与 catalog_info 同源：`<home>/models_catalog.json`（CLI 的 cfg_dir 是
    // config.json 的父目录 = home 根，见 catalog_info 的真相源注释）。
    let path = std::path::PathBuf::from(home)
        .join("models_catalog.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let arr = v.get("entries")?.as_array()?;
    let mut entries = Vec::new();
    for e in arr {
        let Some(key) = e.get("key").and_then(|k| k.as_str()) else {
            continue;
        };
        entries.push(CatalogLiteEntry {
            key: key.to_string(),
            context_window: e.get("context_window").and_then(|c| c.as_u64()).unwrap_or(0),
            max_output_tokens: e.get("max_output_tokens").and_then(|c| c.as_u64()),
            family: e
                .get("family")
                .and_then(|f| f.as_str())
                .map(|s| s.to_string()),
        });
    }
    Some(CatalogLite { entries })
}

// Tests for this handler live in a separate file per project discipline;
// declared HERE (not in handlers/mod.rs) so the private methods are reachable
// and the module compiles under every feature combo (models is ungated).
#[cfg(test)]
mod tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): pin the DISABLED
// typed-save helper's behavior (see its doc comment above).
#[cfg(test)]
mod s10b_tests;
