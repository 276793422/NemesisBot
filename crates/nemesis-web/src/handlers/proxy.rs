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

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_config::proxy_file::{self, EnvProxyConfig, ProxyFileConfig, proxy_config_path};
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

/// 代理配置面（无 feature 闸——独立文件不依赖任何子系统）。
pub struct ProxyHandler;

#[async_trait::async_trait]
impl ModuleHandler for ProxyHandler {
    fn module_name(&self) -> &str {
        "proxy"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["get", "save"]
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
            _ => Err(format!("proxy 模块未知命令：{cmd}")),
        }
    }
}
