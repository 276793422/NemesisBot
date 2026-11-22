//! 会话资源盘点聚合命令（`session.resources`，2026-11-17 会话资源盘点
//! 看板计划 B2）。**实时聚合视图，不是数据源**：每次请求现场遍历注册表
//! → 各 provider 到既有真相源收集 → 按 session_key 过滤 → 组装返回 →
//! 用完即弃。零新增存储、零缓存、零双写。
//!
//! - registry 走静态槽注入（gateway 装配点一行一 provider 后
//!   `install_session_resources` 一次性注入；session_allow_all /
//!   approval_auto 同款双注入形态——不走 AppState 字段，后者有 183 处
//!   测试字面量装配，加字段即面扩大，79b49a25 教训）；槽 None
//!   （headless/relay/测试装配）= 诚实报「未装配」。
//! - 类目响应随行 meta（label/order/transient）——前端零硬编码清单；
//!   未知类目键前端兜底渲染（新 provider 上线看板自动多卡）。
//! - PerLoop provider 先 `resolve_session_loop` 单一裁决点路由（硬编码主
//!   loop = 项目会话假成功的根因模式），loop 缺席统一 Unavailable（原因
//!   透传 resolver 的诚实文案）。
//! - revision = EventHub 全局 seq 现值（既有 L2 基建）——B4 活态更新用
//!   它丢弃乱序事件。
//! - 空 sid → legacy 键（口径地雷 2：前端默认会话形态；agent.rs 特判同源）。

use crate::handlers::projects::resolve_session_loop;
use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::cc_hooks::WEB_SESSION_KEY_PREFIX;
use nemesis_types::resources::{CategoryView, ResourceCtx, ResourceProvider};
use serde_json::{Value, json};

pub struct ResourcesHandler;

/// 会话资源盘点注册表槽（gateway 生命周期单次注入，install_session_allow_all
/// 同款形态：统一定义 Arc，gateway 装配处一次性安装）。
static SESSION_RESOURCES: std::sync::RwLock<
    Option<std::sync::Arc<nemesis_types::resources::SessionResourceRegistry>>,
> = std::sync::RwLock::new(None);

/// gateway 装配处（nemesisbot post_agent）调用；未注入 = headless/relay/
/// 测试装配形态，`session.resources` 诚实报「未装配」。
pub fn install_session_resources(
    registry: std::sync::Arc<nemesis_types::resources::SessionResourceRegistry>,
) {
    *SESSION_RESOURCES
        .write()
        .expect("session resources registry lock poisoned") = Some(registry);
}

/// 槽置换口（仅测试，set_session_allow_all_for_test 同款）。
#[cfg(test)]
pub(crate) fn set_session_resources_for_test(
    registry: Option<std::sync::Arc<nemesis_types::resources::SessionResourceRegistry>>,
) {
    *SESSION_RESOURCES
        .write()
        .expect("session resources registry lock poisoned") = registry;
}

/// 槽触碰类测试的共享串行锁（SESSION_ALLOW_ALL_TEST_LOCK 同款纪律；
/// tokio Mutex——异步用例持锁跨 await）。
#[cfg(test)]
pub(crate) static SESSION_RESOURCES_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

fn session_resources() -> Option<std::sync::Arc<nemesis_types::resources::SessionResourceRegistry>>
{
    SESSION_RESOURCES
        .read()
        .expect("session resources registry lock poisoned")
        .clone()
}

#[async_trait::async_trait]
impl ModuleHandler for ResourcesHandler {
    fn module_name(&self) -> &str {
        "session"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["resources"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        if cmd != "resources" {
            return Err(format!("unknown session command: {cmd}"));
        }
        let registry =
            session_resources().ok_or_else(|| "资源盘点未装配（本网关形态未启用）".to_string())?;

        let raw_sid = data
            .as_ref()
            .and_then(|d| d.get("session_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        // 空 sid = legacy 键（口径地雷 2，agent.rs 特判同源）；非空过
        // sanitize（`:`→`_`，sessions.list 同款公式）。
        let safe_sid =
            nemesis_agent::session::SessionStore::sanitize_session_id(if raw_sid.is_empty() {
                "legacy"
            } else {
                raw_sid
            });
        let session_key = format!("{}{}", WEB_SESSION_KEY_PREFIX, safe_sid);
        let workspace = require_workspace(ctx)?.to_string();
        let rc = ResourceCtx {
            session_key: &session_key,
            bare_sid: &safe_sid,
            workspace_root: std::path::Path::new(&workspace),
        };

        // PerLoop 路由一次（非每 provider 一次）；Err（loop 未运行/项目不
        // 可用）不炸整命令——按类目 Unavailable，原因透传诚实文案。
        let loop_api = resolve_session_loop(ctx, &session_key).ok();

        let mut categories = serde_json::Map::new();
        for provider in registry.providers() {
            let meta = provider.meta();
            let view = match provider {
                ResourceProvider::Global(p) => p.collect(&rc),
                ResourceProvider::PerLoop(p) => match &loop_api {
                    Some(loop_ref) => p.collect(&rc, loop_ref.as_ref()),
                    None => CategoryView::unavailable("会话执行引擎未运行"),
                },
            };
            let mut entry = serde_json::Map::new();
            entry.insert("label".into(), json!(meta.label));
            entry.insert("order".into(), json!(meta.order));
            entry.insert("transient".into(), json!(meta.transient));
            entry.insert("status".into(), json!(view.status.as_str()));
            if let Some(reason) = view.status.unavailable_reason() {
                entry.insert("reason".into(), json!(reason));
            }
            entry.insert("total".into(), json!(view.total));
            entry.insert("summary".into(), view.summary);
            categories.insert(meta.category_id.to_string(), Value::Object(entry));
        }

        Ok(Some(json!({
            "session_id": raw_sid,
            "session_key": session_key,
            "revision": ctx.state.event_hub.latest_seq(),
            "categories": Value::Object(categories),
        })))
    }
}
