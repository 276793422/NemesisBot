//! 邮件基础设施 WSAPI（B3，`mail` feature 门控）——`mail.*` 命令薄代理。
//!
//! **零业务逻辑**：每条命令 = 组参数 → `spawn_blocking` 调契约函数
//! （[`nemesis_plugin::mail::MailPluginHandle`]）→ JSON 透传。插件句柄 =
//! 进程级热槽 [`MAIL_PLUGIN_SLOT`]（gateway 装载 plugin_mail 后注入；
//! 2026-11-20 热装载通路：槽由 AppState 值槽改静态槽——运行期可写，热
//! 装载成功即生效，mail.* 命令即刻可用，不等重启）；槽空（DLL 缺席 / 总
//! 闸关 / 名册开关停用 / 装载失败）= 全部命令诚实报「mail 插件未装载」
//! （wasm 壳 DLL 缺席同款语义）。
//!
//! 热装载接线（gateway post_agent 装配期注入两闭包，quic relay
//! `set_relay_config_refresh` 同款先例——loader 在宿主 crate，本 crate
//! 依赖方向够不到，注入反转）：
//! - [`set_mail_hot_loader`]：load（互斥）→ server_start → 热槽写入全链；
//!   `config_set` 保存后 enabled=true 且未装载时经 `spawn_blocking` 调用，
//!   装载结果当场回传前端（成功 = 邮箱页即时活；失败 = 诚实报原因，修正
//!   后重新保存即可重试）。
//! - [`set_mail_unloaded_probe`]：未装载精确原因探针（总闸未开 / 名册关
//!   闭 / DLL 缺席 / 装载失败四态）——`status` 槽空 hint 数据源；未注入
//!   （测试形态）= 笼统兜底文案。
//!
//! 停用语义（用户拍板 2026-11-20）：dlopen 进程生命周期不可卸载——
//! `enabled=false` 保存只落盘 + 提示「停用需重启网关生效」，不做热卸载。
//!
//! 治理面与 agent 面分离（能力矩阵「不可删」）：本模块含 `mail.delete`
//! （人类授权删信路之一）与 `mail.addr_*`（注册制管理面）——agent 工具面
//! 两者都不供给（B4 工具 schema 层面不存在）。
//!
//! 实时性：SMTP 落信 → 插件 notify_mail_in 回调 → 宿主静态槽 → SSE
//! `mailbox.updated` 广播（gateway 注入闭包，本模块不参与）；前端订阅后
//! 刷新列表。

use std::sync::Arc;

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_plugin::mail::MailPluginHandle;

// ---------------------------------------------------------------------------
// 进程级热槽 + 注入闭包（热装载通路，2026-11-20）
// ---------------------------------------------------------------------------

type HotLoadFn = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
type ProbeFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

static MAIL_PLUGIN_SLOT: parking_lot::RwLock<Option<MailPluginHandle>> =
    parking_lot::RwLock::new(None);
static HOT_LOADER: parking_lot::RwLock<Option<HotLoadFn>> = parking_lot::RwLock::new(None);
static UNLOADED_PROBE: parking_lot::RwLock<Option<ProbeFn>> = parking_lot::RwLock::new(None);

/// 写热槽（gateway 装配点 + 热装载闭包两个写入点；句柄 fn 指针 Copy）。
pub fn set_mail_plugin_slot(handle: MailPluginHandle) {
    *MAIL_PLUGIN_SLOT.write() = Some(handle);
}

/// 槽当前值快照。
pub fn mail_plugin_slot() -> Option<MailPluginHandle> {
    *MAIL_PLUGIN_SLOT.read()
}

/// 注入热装载闭包（gateway post_agent 装配期）。
pub fn set_mail_hot_loader(f: HotLoadFn) {
    *HOT_LOADER.write() = Some(f);
}

/// 注入未装载原因探针（gateway post_agent 装配期）。
pub fn set_mail_unloaded_probe(f: ProbeFn) {
    *UNLOADED_PROBE.write() = Some(f);
}

/// 热装载执行（spawn_blocking 包闭包全链——FFI 同步）。返回 (装载成功?,
/// 精确错误)。闭包缺席（未注入形态）= Err（诚实告知不可热装载）。
async fn run_hot_loader() -> Result<(), String> {
    let f = HOT_LOADER
        .read()
        .clone()
        .ok_or_else(|| "热装载闭包未注入（本进程非 gateway 装配形态）".to_string())?;
    tokio::task::spawn_blocking(move || f())
        .await
        .map_err(|e| format!("mail 热装载: spawn_blocking join 失败: {e}"))?
}

/// 未装载 hint（探针注入 = 精确原因；未注入 = 笼统兜底）。恒含「未装载」
/// 前缀（IT mail_ws 契约：hint.contains("未装载")）。
fn unloaded_hint() -> String {
    UNLOADED_PROBE
        .read()
        .clone()
        .and_then(|f| f())
        .unwrap_or_else(|| {
            "mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）".to_string()
        })
}

#[cfg(test)]
pub(crate) fn reset_slots_for_test() {
    *MAIL_PLUGIN_SLOT.write() = None;
    *HOT_LOADER.write() = None;
    *UNLOADED_PROBE.write() = None;
}

/// 当前生效句柄：进程级热槽（运行期热写）优先，回落 AppState 装配期槽
/// （gateway 启动时序注入）。两槽互斥成立：装配期装载成功 → AppState 有值
/// 热槽空（热装载只在未装载时触发）；热装载成功 → 热槽有值。返回 `None`
/// = 插件未装载。
fn effective_handle(ctx: &RequestContext) -> Option<MailPluginHandle> {
    mail_plugin_slot().or(ctx.state.mail_plugin)
}

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
            return match effective_handle(ctx) {
                None => Ok(Some(serde_json::json!({
                    "loaded": false,
                    "hint": unloaded_hint(),
                }))),
                Some(handle) => {
                    let raw = tokio::task::spawn_blocking(move || handle.server_status())
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
        let handle = effective_handle(ctx).ok_or_else(unloaded_hint)?;
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
/// - **热装载通路（2026-11-20，用户拍板「不能让用户重启 gateway」）**：
///   保存后 `enabled=true` 且热槽空 → 经 [`run_hot_loader`] 热装载（load +
///   server_start + 热槽写入全链，装载互斥在 loader 侧）——结果当场回传
///   前端（`loaded` + `load_error`），成功 = 邮箱页即时活。已装载 = 维持
///   重启语义（配置刷新不做热重载——init 重入面未开）。`enabled=false` =
///   落盘 + 提示停用需重启（dlopen 不可卸载，用户拍板的重启语义）。
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
            let enabled = config
                .get("enabled")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            // 槽态合流判定（装配期槽 + 热槽——只查热槽会把「启动时已装载」
            // 误判成未装载 → 已装载态重复热装载；effective_handle 单点裁决）
            let was_loaded = effective_handle(ctx).is_some();
            let content = serde_json::to_string_pretty(config)
                .map_err(|e| format!("config 序列化失败: {e}"))?;
            tokio::task::spawn_blocking(move || {
                nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            })
            .await
            .map_err(|e| format!("mail.config_set: spawn_blocking join 失败: {e}"))??;
            // ---- 热装载分支（只在未装载且总闸开时；槽态以落槽事实为准） ----
            if enabled && !was_loaded {
                let load_result = run_hot_loader().await;
                let loaded_now = effective_handle(ctx).is_some();
                return Ok(Some(match load_result {
                    Ok(()) => serde_json::json!({
                        "ok": true,
                        "loaded": loaded_now,
                        "note": "已保存并即时装载（热生效，无需重启网关）",
                    }),
                    Err(e) => serde_json::json!({
                        "ok": true,
                        "loaded": loaded_now,
                        "load_error": e,
                        "note": format!("已保存；即时装载失败：{e}（修正后重新保存即可重试）"),
                    }),
                }));
            }
            if !enabled && was_loaded {
                return Ok(Some(serde_json::json!({
                    "ok": true,
                    "loaded": true,
                    "note": "已保存。邮箱插件已装载，停用需重启网关生效（插件库进程生命周期不可卸载）",
                })));
            }
            Ok(Some(serde_json::json!({
                "ok": true,
                "loaded": was_loaded,
                "note": if was_loaded {
                    "已保存；运行中插件的配置刷新需重启网关生效".to_string()
                } else {
                    "已保存；重启网关后生效（SMTP/POP3/客户端配置在网关启动时装配）".to_string()
                },
            })))
        }
        _ => Err(format!("unknown command: mail.{}", cmd)),
    }
}

#[cfg(all(test, feature = "mail"))]
mod tests;
