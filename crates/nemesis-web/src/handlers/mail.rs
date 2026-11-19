//! 邮件基础设施 WSAPI（B3，`mail` feature 门控）——`mail.*` 命令薄代理。
//!
//! **零业务逻辑**：每条命令 = 组参数 → `spawn_blocking` 调契约函数
//! （[`nemesis_plugin::mail::MailPluginHandle`]）→ JSON 透传。插件句柄 =
//! `AppState.mail_plugin` 槽（gateway 装载 plugin_mail 后注入，quic_relay
//! 同款形态）；槽空（DLL 缺席 / 总闸关 / 名册开关停用 / 装载失败）= 全部
//! 命令诚实报「mail 插件未装载」（wasm 壳 DLL 缺席同款语义）。
//!
//! 治理面与 agent 面分离（能力矩阵「不可删」）：本模块含 `mail.delete`
//! （人类授权删信路之一）与 `mail.addr_*`（注册制管理面）——agent 工具面
//! 两者都不供给（B4 工具 schema 层面不存在）。
//!
//! 实时性：SMTP 落信 → 插件 notify_mail_in 回调 → 宿主静态槽 → SSE
//! `mailbox.updated` 广播（gateway 注入闭包，本模块不参与）；前端订阅后
//! 刷新列表。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_plugin::mail::MailPluginHandle;

pub struct MailHandler;

#[async_trait::async_trait]
impl ModuleHandler for MailHandler {
    fn module_name(&self) -> &str {
        "mail"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            // 存储/发送面（7）
            "send",
            "list",
            "read",
            "mark_read",
            "delete",
            "stats",
            "record",
            // 地址注册制管理面（3；宿主治理面——agent 工具面不含）
            "addr_add",
            "addr_remove",
            "addr_list",
            // 服务端运行态（SMTP/POP3 running/bind/port；槽空 = loaded:false 非
            // 报错——状态查询的职责就是暴露装载状态，前端降级横幅数据源）
            "status",
            // 配置面（config.mail.json 读写；先于句柄闸——插件缺席也要能配，
            // 空态卡「先配好再装载」引导闭环）
            "config_get",
            "config_set",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // status / config_* 先于句柄闸：槽空照常工作（status 返回 loaded:false；
        // config 读写盘上真相源与插件装载无关——缺席也要能配，装好后重启即用），
        // 其余命令槽空照旧统一诚实报错（不给探测面区分信号差——status 本身就是
        // 那个信号面）。
        if cmd == "status" {
            return match ctx.state.mail_plugin.as_ref() {
                None => Ok(Some(serde_json::json!({
                    "loaded": false,
                    "hint": "mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）",
                }))),
                Some(handle) => {
                    let h = *handle;
                    let raw = tokio::task::spawn_blocking(move || h.server_status())
                        .await
                        .map_err(|e| format!("mail.status: spawn_blocking join 失败: {e}"))??;
                    let mut v: serde_json::Value = serde_json::from_str(&raw)
                        .map_err(|e| format!("mail.status: 返回非 JSON: {e}（raw={raw}）"))?;
                    v["loaded"] = serde_json::json!(true);
                    Ok(Some(v))
                }
            };
        }
        if cmd == "config_get" || cmd == "config_set" {
            return mail_config_cmd(cmd, data, ctx).await;
        }
        let handle = *ctx
            .state
            .mail_plugin
            .as_ref()
            .ok_or("mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）")?;
        match cmd {
            "send" => {
                let d = data.as_ref().ok_or("missing data")?;
                let to = d
                    .get("to")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 to 字段")?
                    .to_string();
                let subject = d
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let body = d
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                proxy(&handle, "mail.send", move |h| h.send(&to, &subject, &body)).await
            }
            "list" => {
                // query 组参数：mailbox/unread_only/limit/offset（缺省 = 插件侧默认）。
                let d = data.unwrap_or(serde_json::json!({}));
                let q = serde_json::json!({
                    "mailbox": d.get("mailbox").and_then(|v| v.as_str()).unwrap_or(""),
                    "unread_only": d.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(false),
                    "limit": d.get("limit").and_then(|v| v.as_u64()).unwrap_or(0),
                    "offset": d.get("offset").and_then(|v| v.as_u64()).unwrap_or(0),
                });
                proxy(&handle, "mail.list", move |h| h.list(&q.to_string())).await
            }
            "read" => {
                let id = require_id(data.as_ref())?;
                proxy(&handle, "mail.read", move |h| h.read(&id)).await
            }
            "mark_read" => {
                let id = require_id(data.as_ref())?;
                proxy(&handle, "mail.mark_read", move |h| h.mark_read(&id)).await
            }
            "delete" => {
                let id = require_id(data.as_ref())?;
                proxy(&handle, "mail.delete", move |h| h.delete(&id)).await
            }
            "stats" => proxy(&handle, "mail.stats", move |h| h.stats()).await,
            "record" => {
                // 收信记录登记：meta JSON 透传（message_id 去重在插件侧；
                // 可选 addr 字段投指定库）。
                let d = data.ok_or("missing data")?;
                proxy(&handle, "mail.record", move |h| h.record(&d.to_string())).await
            }
            "addr_add" => {
                let d = data.as_ref().ok_or("missing data")?;
                let address = d
                    .get("address")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 address 字段")?
                    .to_string();
                proxy(&handle, "mail.addr_add", move |h| h.addr_add(&address)).await
            }
            "addr_remove" => {
                let d = data.as_ref().ok_or("missing data")?;
                let address = d
                    .get("address")
                    .and_then(|v| v.as_str())
                    .ok_or("缺 address 字段")?
                    .to_string();
                proxy(&handle, "mail.addr_remove", move |h| {
                    h.addr_remove(&address)
                })
                .await
            }
            "addr_list" => proxy(&handle, "mail.addr_list", move |h| h.addr_list()).await,
            _ => Err(format!("unknown command: mail.{}", cmd)),
        }
    }
}

fn require_id(data: Option<&serde_json::Value>) -> Result<String, String> {
    data.and_then(|d| d.get("id").and_then(|v| v.as_str()))
        .map(str::to_string)
        .ok_or_else(|| "缺 id 字段".to_string())
}

/// 统一代理形态：spawn_blocking + JSON 解析透传（`call_mail` 同源纪律——
/// 句柄 fn 指针 Copy，闭包持值移动）。
async fn proxy(
    handle: &MailPluginHandle,
    what: &'static str,
    f: impl FnOnce(MailPluginHandle) -> Result<String, String> + Send + 'static,
) -> Result<Option<serde_json::Value>, String> {
    let h = *handle;
    let raw = tokio::task::spawn_blocking(move || f(h))
        .await
        .map_err(|e| format!("{what}: spawn_blocking join 失败: {e}"))??;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("{what}: 返回非 JSON: {e}（raw={raw}）"))?;
    Ok(Some(v))
}

/// config.mail.json 读写（`config_get`/`config_set`；先于句柄闸——插件缺席
/// 也要能配，与空态卡引导闭环）。
///
/// - 读：BOM 容忍；缺文件 = `{}`（全默认档）；**解析失败 = 诚实报错**（不
///   静默落空对象——loader 读损坏 fail-closed 全关，配置页必须让用户看见
///   损坏才能修正）。
/// - 写：`data.config` 必须是 object，整体落盘（pretty + `0o600`——config
///   家族惯例，含 password 敏感值；与 handlers/config.rs 同款原子写）。
/// - **保存后重启 gateway 生效**（loader `mail_init_json` 在 gateway 启动
///   消费；与 plugin enable CLI 同语义：盘上真相 + 重启装配），返回 note
///   提示前端转告用户。
async fn mail_config_cmd(
    cmd: &str,
    data: Option<serde_json::Value>,
    ctx: &RequestContext,
) -> Result<Option<serde_json::Value>, String> {
    let workspace = super::require_workspace(ctx)?;
    let path = nemesis_path::resolve_mail_config_path_in_workspace(std::path::Path::new(workspace));
    match cmd {
        "config_get" => {
            let path_str = path.to_string_lossy().into_owned();
            let v = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
                match std::fs::read_to_string(&path) {
                    Ok(raw) => {
                        let raw = raw.trim_start_matches('\u{feff}');
                        serde_json::from_str(raw).map_err(|e| {
                            format!(
                                "config.mail.json 解析失败（损坏态：插件侧 fail-closed 全关）: {e}"
                            )
                        })
                    }
                    Err(_) => Ok(serde_json::json!({})), // 缺文件 = 空对象（全默认）
                }
            })
            .await
            .map_err(|e| format!("mail.config_get: spawn_blocking join 失败: {e}"))??;
            Ok(Some(serde_json::json!({
                "config": v,
                "path": path_str,
            })))
        }
        "config_set" => {
            let d = data.ok_or("missing data")?;
            let config = d.get("config").ok_or("缺 config 字段")?;
            if !config.is_object() {
                return Err("config 必须是 JSON object".to_string());
            }
            let content = serde_json::to_string_pretty(config)
                .map_err(|e| format!("config 序列化失败: {e}"))?;
            tokio::task::spawn_blocking(move || {
                nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            })
            .await
            .map_err(|e| format!("mail.config_set: spawn_blocking join 失败: {e}"))??;
            Ok(Some(serde_json::json!({
                "ok": true,
                "note": "已保存；重启网关后生效（SMTP/POP3/客户端配置在网关启动时装配）",
            })))
        }
        _ => Err(format!("unknown command: mail.{}", cmd)),
    }
}

#[cfg(all(test, feature = "mail"))]
mod tests;
