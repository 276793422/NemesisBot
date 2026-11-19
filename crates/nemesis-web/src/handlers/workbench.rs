//! 统一工作台 WSAPI（看板人工触点体系 P1；计划 §6.5「两层次展示」的重页）。
//!
//! 展示分工：**列表归看板**（`board.touchpoint.list` + issue.list 活卡徽标
//! 投影——轻量徽标/置顶/过滤片），**页面归工作台**（本模块——文档级重页
//! `/workbench/:kind/:id` 的取数与动作漏斗）。kind 走 URL 段形态
//! `touchpoint-spec` / `touchpoint-prototype`（前端按 kind 选渲染器；后端
//! 映射 [`TouchpointKind`] 后委托 `board::touchpoints` 漏斗——respond/取数
//! 单一真相源，本模块只做路由壳：kind 映射 + 身份固定 + store 装配检查）。
//!
//! 操作者身份：dashboard 登录用户 → `Actor::admin("dashboard")`（与 board
//! 模块同口径——board.db 节点本地单管理员语义，不用每连必换的 session id）。

use crate::handlers::board::touchpoints;
use crate::handlers::get_str;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_board::assignment::Actor;
use nemesis_board::models::TouchpointKind;

pub struct WorkbenchHandler;

/// URL kind 段 → TouchpointKind（值域单一映射点；未知 loud 报错——
/// 与触点动作词表同款「不静默纠偏」纪律）。URL 段带 `touchpoint-` 前缀
/// （`touchpoint-spec` / `touchpoint-prototype`），纯 kind 名 `spec` 也收
/// （手打容错；报错文案列权威形态）。
fn kind_from_str(s: &str) -> Result<TouchpointKind, String> {
    let stripped = s.strip_prefix("touchpoint-").unwrap_or(s);
    TouchpointKind::parse(stripped).ok_or_else(|| {
        format!("未知工作台 kind \"{s}\"（可选：touchpoint-spec / touchpoint-prototype）")
    })
}

fn require_id(data: &serde_json::Value) -> Result<i64, String> {
    data.get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "缺少 id（触点 id）".to_string())
}

/// kind 路由参数与触点实体的一致性检查：stale 链接（触点已换 kind 不可
/// 能、但 kind 段手打/交叉）在取数层诚实报错，不喂错渲染器。
fn require_kind_match(
    kind: TouchpointKind,
    url_kind: &str,
    payload: &serde_json::Value,
    id: i64,
) -> Result<(), String> {
    let tp_kind_str = payload
        .get("touchpoint")
        .and_then(|t| t.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");
    if TouchpointKind::parse(tp_kind_str) != Some(kind) {
        return Err(format!(
            "kind 不匹配：触点 {id} 是 \"{tp_kind_str}\"，路由 kind 段是 \"{url_kind}\"——请用触点自身的 kind 打开工作台"
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl ModuleHandler for WorkbenchHandler {
    fn module_name(&self) -> &str {
        "workbench"
    }

    fn commands(&self) -> &'static [&'static str] {
        // BUG-20261113-156：裸 cmd 名（与全仓 51 模块惯例一致——router 把
        // msg.cmd 原样传进 handle_cmd，前端 request('workbench', 'get') 发
        // 的是裸名；旧清单「workbench.get」与 match 臂全名形态导致全部
        // 请求落 unknown 分支，工作台页面取数与动作全灭）。
        &["get", "respond"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // board service 未注入（board feature 关 / gateway 装配缺席）= 诚实
        // 报错，与 board 模块同语义（不做静默空载荷）。
        let store = ctx
            .state
            .board
            .as_ref()
            .map(|svc| svc.store().clone())
            .ok_or_else(|| "board service not available".to_string())?;
        let data = data.unwrap_or_else(|| serde_json::json!({}));
        match cmd {
            // 页面完整载荷：触点语义 + 承载单全视图 + 默认动作 + 无人值守
            // 解析（见 touchpoints::touchpoint_workbench_payload）。
            // BUG-20261113-156：裸名（router 原样传 msg.cmd；全名旧形态
            // 与前端 request('workbench', 'get') 契约错位，见 commands()）。
            "get" => {
                let kind_str = get_str(&data, "kind")?;
                let kind = kind_from_str(&kind_str)?;
                let id = require_id(&data)?;
                let cfg = touchpoints::live_attendance_config();
                let out = touchpoints::touchpoint_workbench_payload(&store, &cfg, id)?;
                require_kind_match(kind, &kind_str, &out, id)?;
                Ok(Some(out))
            }
            // 人工动作漏斗：confirm / revise_spec / rework_local / cancel /
            // mute（词表与校验在 respond_touchpoint；payload 透传原样）。
            "respond" => {
                let kind_str = get_str(&data, "kind")?;
                kind_from_str(&kind_str)?; // 值域校验（id 才是权威键）
                let id = require_id(&data)?;
                let action = get_str(&data, "action")?;
                let payload = data
                    .get("payload")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let cfg = touchpoints::live_attendance_config();
                let actor = Actor::admin("dashboard");
                let out =
                    touchpoints::respond_touchpoint(&store, &cfg, &actor, id, &action, &payload)?;
                Ok(Some(out))
            }
            other => Err(format!(
                "未知 workbench 命令 \"{other}\"（可选：get / respond）"
            )),
        }
    }
}
