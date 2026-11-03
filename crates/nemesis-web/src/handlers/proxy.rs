//! 独立代理配置 WSAPI（2026-10-31 代理设置三项改造批B）。
//!
//! `config.proxy.json`（`nemesis-config::proxy_file`，与 config.json 分文件的
//! 用户裁决）的 Dashboard 面：
//! - `proxy.get`：文件配置 + 进程当前读数双列（env 段重启生效——重启后
//!   两列一致即可自证闭环）+ selfupdate 段 + `file_exists`/`parse_error`
//!   （损坏诚实投影为未配置 + 错误原因，不阻断）。
//! - `proxy.save`：env 段与 selfupdate 段各自独立保存（read-modify-write
//!   互不覆盖），前缀白名单校验在 `proxy_file` 内 loud 拒绝；返回生效语义
//!   提示（env=重启网关生效；selfupdate=下次下载/检查即生效）。
//!
//! 注意 env 段的进程读数是**只读投影**——本 handler 绝不热改进程环境变量
//! （reqwest client 在构造期快照 env，热改不生效还会造成「看起来改了」的
//! 假象）；生效路径只有 gateway 启动注入一条。
//!
//! `proxy.keys.*`（2026-11-03 LLM代理调用方key管理 P5）：`config.proxy_keys.json`
//! （`nemesis-config::proxy_keys_file`）的调用方 key 管理面——keys.list /
//! keys.add（key 缺省自动生成 `nbk_`+32hex；拒绝与主 token 相同）/ keys.remove /
//! keys.set_enabled。全部写盘即热生效（鉴权回落读点按 mtime 缓存，无需重启）；
//! name/key 查重与非空校验在 config 层 loud 拒绝。命令族在 dashboard 鉴权闸内
//! （主 token 才能调），代理 key 打不进 WSAPI（路径白名单只开两条代理 lane）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_config::{
    proxy_file::{self, EnvProxyConfig, ProxyFileConfig, proxy_config_path},
    proxy_keys_file::{self, ProxyKeyEntry},
};
use serde_json::{Value, json};
use std::path::PathBuf;

fn workspace_of(ctx: &RequestContext) -> Result<PathBuf, String> {
    ctx.workspace
        .clone()
        .map(PathBuf::from)
        .ok_or_else(|| "workspace not configured".to_string())
}

/// env 四键 → JSON（文件配置形态）。
fn env_obj(env: &EnvProxyConfig) -> Value {
    json!({
        "http_proxy": env.http_proxy,
        "https_proxy": env.https_proxy,
        "all_proxy": env.all_proxy,
        "no_proxy": env.no_proxy,
    })
}

/// 进程当前读数：大写优先、小写兜底（gateway 注入大小写双对——hyper-util
/// from_env 读大写、CLI 子进程常读小写，读数以先命中者为准）。
fn env_obj_process() -> Value {
    fn read(key: &str) -> String {
        std::env::var(key.to_uppercase())
            .or_else(|_| std::env::var(key.to_lowercase()))
            .unwrap_or_default()
    }
    json!({
        "http_proxy": read("http_proxy"),
        "https_proxy": read("https_proxy"),
        "all_proxy": read("all_proxy"),
        "no_proxy": read("no_proxy"),
    })
}

/// `config.proxy.json` 的盘上状态 → (file_exists, parse_error, 配置投影)。
/// 文件缺失 = 未配置（静默 None）；损坏 = parse_error 带原因 + 按未配置投影。
fn file_state(config_dir: &std::path::Path) -> (bool, Option<String>, ProxyFileConfig) {
    match proxy_file::load(config_dir) {
        Ok(Some(c)) => (true, None, c),
        Ok(None) => (false, None, ProxyFileConfig::default()),
        Err(e) => (true, Some(e), ProxyFileConfig::default()),
    }
}

/// ProxyKeyEntry → JSON 投影（key 明文回显：Dashboard 管理面需复制给调用方；
/// 本命令族在 dashboard 鉴权闸内，调用者本就是凭据发放者）。
fn key_obj(k: &ProxyKeyEntry) -> Value {
    json!({
        "name": k.name,
        "key": k.key,
        "enabled": k.enabled,
        "created_at": k.created_at,
        "note": k.note,
    })
}

/// 代理配置面（无 feature 闸——独立文件不依赖任何子系统）。
pub struct ProxyHandler;

#[async_trait::async_trait]
impl ModuleHandler for ProxyHandler {
    fn module_name(&self) -> &str {
        "proxy"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "get",
            "save",
            "keys.list",
            "keys.add",
            "keys.remove",
            "keys.set_enabled",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "get" => {
                let config_dir = workspace_of(ctx)?.join("config");
                let (file_exists, parse_error, cfg) = file_state(&config_dir);
                Ok(Some(json!({
                    "file_exists": file_exists,
                    "parse_error": parse_error,
                    "path": proxy_config_path(&config_dir).to_string_lossy(),
                    "env_file": env_obj(&cfg.env),
                    "env_process": env_obj_process(),
                    "selfupdate": { "proxy": cfg.selfupdate.proxy },
                })))
            }
            "save" => {
                let config_dir = workspace_of(ctx)?.join("config");
                let data = data.ok_or_else(|| "缺少保存数据".to_string())?;
                let mut saved: Vec<&str> = Vec::new();
                if let Some(env_v) = data.get("env") {
                    let env: EnvProxyConfig = serde_json::from_value(env_v.clone())
                        .map_err(|e| format!("env 段格式错误：{e}"))?;
                    proxy_file::save_env(&config_dir, &env)?;
                    saved.push("env");
                }
                if let Some(su) = data.get("selfupdate") {
                    let proxy = su
                        .get("proxy")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "selfupdate 段缺少 proxy 字符串".to_string())?;
                    proxy_file::save_selfupdate(&config_dir, proxy)?;
                    saved.push("selfupdate");
                }
                if saved.is_empty() {
                    return Err("没有可保存的段（env / selfupdate 至少传一个）".to_string());
                }
                let note = match (saved.contains(&"env"), saved.contains(&"selfupdate")) {
                    (true, true) => {
                        "已保存。环境变量代理：重启网关后生效；更新下载代理：下次下载/检查即生效"
                    }
                    (true, false) => "已保存，重启网关后生效",
                    (false, true) => "已保存，下次下载/检查即生效",
                    (false, false) => unreachable!(),
                };
                Ok(Some(json!({ "saved": saved, "note": note })))
            }
            // --- 调用方 key 管理（写盘即热生效，无需重启） ---
            "keys.list" => {
                let config_dir = workspace_of(ctx)?.join("config");
                let path = proxy_keys_file::proxy_keys_config_path(&config_dir);
                let keys = proxy_keys_file::list(&config_dir);
                Ok(Some(json!({
                    "file_exists": path.exists(),
                    "path": path.to_string_lossy(),
                    "keys": keys.iter().map(key_obj).collect::<Vec<_>>(),
                })))
            }
            "keys.add" => {
                let config_dir = workspace_of(ctx)?.join("config");
                let data = data.ok_or_else(|| "缺少 key 数据".to_string())?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if name.is_empty() {
                    return Err("调用方名称（name）不能为空".to_string());
                }
                let key = match data
                    .get("key")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(k) => k.to_string(),
                    // key 缺省自动生成：nbk_ + 32 hex。
                    None => format!("nbk_{}", uuid::Uuid::new_v4().simple()),
                };
                // key ≠ 主 token：相同 = 等效把主凭据发给调用方（防呆；主
                // token 为空的免闸部署下 key 恒非空，不会误伤）。
                let master = ctx.state.auth_token.trim();
                if !master.is_empty() && key == master {
                    return Err("key 不能与主 token 相同（等效于把主凭据发给调用方）".to_string());
                }
                let note = data
                    .get("note")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                proxy_keys_file::add_key(
                    &config_dir,
                    ProxyKeyEntry {
                        name: name.clone(),
                        key,
                        enabled: true,
                        created_at: 0,
                        note,
                    },
                )?;
                let saved = proxy_keys_file::list(&config_dir)
                    .into_iter()
                    .find(|k| k.name == name);
                Ok(Some(json!({
                    "key": saved.as_ref().map(key_obj),
                    "note": "已保存，即刻生效：调用方持此 key 打代理 lane（/v1/chat/completions、/api/chat/stream）即放行",
                })))
            }
            "keys.remove" => {
                let config_dir = workspace_of(ctx)?.join("config");
                let data = data.ok_or_else(|| "缺少 name".to_string())?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "缺少 name".to_string())?;
                let removed = proxy_keys_file::remove_key(&config_dir, name)?;
                if !removed {
                    return Err(format!("调用方不存在：{name}"));
                }
                Ok(Some(json!({
                    "removed": name,
                    "note": "已删除，即刻生效（该调用方的 key 不再放行）",
                })))
            }
            "keys.set_enabled" => {
                let config_dir = workspace_of(ctx)?.join("config");
                let data = data.ok_or_else(|| "缺少参数".to_string())?;
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "缺少 name".to_string())?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| "缺少 enabled 布尔值".to_string())?;
                let changed = proxy_keys_file::set_enabled(&config_dir, name, enabled)?;
                if !changed {
                    return Err(format!("调用方不存在：{name}"));
                }
                let saved = proxy_keys_file::list(&config_dir)
                    .into_iter()
                    .find(|k| k.name == name);
                Ok(Some(json!({
                    "key": saved.as_ref().map(key_obj),
                    "note": if enabled {
                        "已启用，即刻生效"
                    } else {
                        "已禁用，即刻生效（该调用方的 key 不再放行，重启后仍为禁用）"
                    },
                })))
            }
            _ => Err(format!("proxy 模块未知命令：{cmd}")),
        }
    }
}

#[cfg(test)]
mod tests;
