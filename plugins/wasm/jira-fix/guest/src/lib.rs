//! jira-fix 插件 guest（组合 world `plugin-panel-flow-step`，0.2.0 自持批）。
//!
//! 自持形态：插件**自带 Jira 直连**（http-send + secret-get），connector.toml
//! 退役——不再依赖宿主 connector 子系统。同一组件双导出，命令词表全自报：
//! - 面板命令（Panel 帧，有写面）：`sync` / `jira_get`（0.1 投影形态保留）
//!   + `save_settings`（config-set ×3 + secret-set pat——实例配置与凭据的
//!   自持写通道；全空入参 = 页面挂载自动探活，诚实无写入应答）。
//! - flow 步骤（FlowStep 帧，无写面）：`fetch`（JQL 检索，entries 直出
//!   raw Jira issue 对象——宿主按模板 key_field/title_field/body_field
//!   物化）/ `comment`（issue 回帖；on-transition 与 report 两路入参同形
//!   `{key, body}`，单 handler 覆盖）。
//!
//! 凭据：PAT 经面板表单 secret-set 写 vault `plugin/jira-fix/pat`，调用
//! 现取现用（Bearer 头），不落日志不出审计值。配置：base_url 无声明缺省
//! （防假缺省 URL）——未设置时各路诚实报错，不猜；egress allowlist =
//! `{{config.base_url}}` 整条目 token，随配置热生效（fresh-store-per-call）。
//!
//! Jira REST v2 形态（沿用 0.1 注释）：search 返回**包裹对象**
//! `{issues:[{key, fields:{summary, description, status:{name}, assignee:{displayName}}}], total}`
//! （非顶层数组）；issue key 是字符串（PROJ-123 形态），无整数序号。

use exports::nemesis::plugin::flow_step::Guest as StepGuest;
use exports::nemesis::plugin::ui::Guest as UiGuest;
use nemesis::plugin::host::{self, HostError};
use serde_json::{json, Value};

struct JiraFixPlugin;

/// 入参投影取 `args.<key>` 串字段（各路入参统一形态 `{"op"?, "args": {...}}`）。
fn arg_str(v: &Value, key: &str) -> Option<String> {
    v.get("args")?.get(key)?.as_str().map(str::to_string)
}

/// trim 后非空才要（空串视作未填写——表单留空 = 保持原值）。
fn trim_nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn err_reply(msg: impl std::fmt::Display) -> Result<String, HostError> {
    Ok(json!({ "error": msg.to_string() }).to_string())
}

/// base_url 统一读实例配置（config-get；声明无缺省 → 只能是操作员条目）。
fn base_url() -> Result<String, String> {
    match host::config_get("base_url") {
        Ok(Some(u)) => trim_nonempty(&u)
            .map(|u| u.trim_end_matches('/').to_string())
            .ok_or_else(|| "base_url 已配置但为空：请在面板「实例设置」重新保存".into()),
        Ok(None) => Err("base_url 未设置：请先在面板「实例设置」表单保存 Jira 基址".into()),
        Err(e) => Err(format!("base_url 读取失败: {e:?}")),
    }
}

/// PAT 现取现用（vault `plugin/jira-fix/pat`；缺失 = 诚实报错不猜）。
fn jira_pat() -> Result<String, String> {
    let pat = host::secret_get("pat")
        .map_err(|e| format!("PAT 未就绪：请先在面板「实例设置」表单保存访问令牌（{e:?}）"))?;
    if pat.trim().is_empty() {
        return Err("PAT 已配置但为空：请在面板「实例设置」重新保存访问令牌".into());
    }
    Ok(pat)
}

/// max_results 条数（操作员条目优先，缺省 20；声明缺省只服务装载期渲染）。
fn config_max_results() -> u32 {
    match host::config_get("max_results") {
        Ok(Some(v)) => v.trim().parse().unwrap_or(20),
        _ => 20,
    }
}

/// 极简 percent-encode（RFC 3986 unreserved 保留，其余 %XX——JQL/issue key
/// 进 query/path 段用；无外部 crate）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Jira REST v2 单发请求（http-send 过帧闸：egress + 预算 + 审计；非 2xx /
/// 非 JSON = 业务错误串，由调用方决定报错形态）。
fn jira_request(method: &str, path_and_query: &str, body: Option<Value>) -> Result<Value, String> {
    let base = base_url()?;
    let pat = jira_pat()?;
    let mut headers = vec![
        ("Authorization".to_string(), format!("Bearer {pat}")),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    if body.is_some() {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let req = host::HttpRequest {
        method: method.to_string(),
        url: format!("{base}{path_and_query}"),
        headers,
        body: body.map(|b| b.to_string().into_bytes()),
    };
    let resp = host::http_send(&req).map_err(|e| format!("Jira 请求失败: {e:?}"))?;
    if !(200..300).contains(&resp.status) {
        let head: String = String::from_utf8_lossy(&resp.body).chars().take(200).collect();
        return Err(format!("Jira HTTP {}: {}", resp.status, head));
    }
    serde_json::from_slice(&resp.body).map_err(|e| format!("Jira 响应非 JSON: {e}"))
}

/// JQL 检索（search 包裹对象 → issues 数组直出）。
/// R9 复评整改：fields 取回面带 description——模板 body_field 物化坐标
/// `$.fields.description` 此前从未被取回（fields 只有 key,summary,status），
/// 无正文的 issue 与「字段未取回」不可区分（物化正文恒空）。
/// R10 复评整改：startAt 递进翻页——maxResults 受服务端上限钳制（Jira
/// 常见上限远小于调用方请求值），旧单发会把超过单页的条目静默截断。
/// 翻到取尽（累计 ≥ total）、补足请求量（≥ max）或页预算（32 页）止；
/// 首页响应缺 total 时按页预算兜底（不猜服务端上限）。
fn jira_search(jql: &str, max: u32) -> Result<Vec<Value>, String> {
    let max = max.max(1) as usize;
    let mut all: Vec<Value> = Vec::new();
    let mut total = usize::MAX;
    let mut start_at = 0usize;
    for _page in 0..32 {
        let reply = jira_request(
            "GET",
            &format!(
                "/rest/api/2/search?jql={}&startAt={start_at}&maxResults={max}&fields=key,summary,status,description",
                urlencode(jql)
            ),
            None,
        )?;
        if let Some(t) = reply.get("total").and_then(Value::as_u64) {
            total = t as usize;
        }
        let page = reply
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let got = page.len();
        all.extend(page);
        if got == 0 || all.len() >= total || all.len() >= max {
            break;
        }
        start_at += got;
    }
    Ok(all)
}

/// issue 回帖（POST /issue/{key}/comment，body `{"body": <文本>}`）。
fn jira_comment(key: &str, body: &str) -> Result<Value, String> {
    jira_request(
        "POST",
        &format!("/rest/api/2/issue/{}/comment", urlencode(key)),
        Some(json!({ "body": body })),
    )
}

impl UiGuest for JiraFixPlugin {
    fn handle_ui_command(input: String) -> Result<String, HostError> {
        let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let command = v
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match command.as_str() {
            "sync" => {
                let jql = arg_str(&v, "jql")
                    .and_then(|s| trim_nonempty(&s))
                    .unwrap_or_else(|| "ORDER BY created DESC".into());
                match jira_search(&jql, config_max_results()) {
                    Ok(issues) => {
                        let rows: Vec<Value> = issues
                            .iter()
                            .filter_map(|it| {
                                let key = it.get("key")?.as_str()?.to_string();
                                let summary = it
                                    .pointer("/fields/summary")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                let status = it
                                    .pointer("/fields/status/name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                Some(json!({ "key": key, "summary": summary, "status": status }))
                            })
                            .collect();
                        let total = rows.len();
                        Ok(json!({
                            "rows": rows,
                            "total": total,
                            "message": format!("检索到 {total} 条"),
                        })
                        .to_string())
                    }
                    Err(e) => err_reply(e),
                }
            }
            "jira_get" => {
                let Some(key) = arg_str(&v, "key")
                    .map(|s| s.trim().to_uppercase())
                    .filter(|s| !s.is_empty())
                else {
                    return err_reply("缺少参数 key（issue 键，如 PROJ-123）");
                };
                match jira_request(
                    "GET",
                    &format!(
                        "/rest/api/2/issue/{}?fields=summary,status,assignee",
                        urlencode(&key)
                    ),
                    None,
                ) {
                    Ok(reply) => {
                        let summary = reply
                            .pointer("/fields/summary")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let status = reply
                            .pointer("/fields/status/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let assignee = reply
                            .pointer("/fields/assignee/displayName")
                            .and_then(Value::as_str)
                            .unwrap_or("未分配")
                            .to_string();
                        Ok(json!({
                            "key": key,
                            "summary": summary,
                            "status": status,
                            "assignee": assignee,
                            "message": format!("{key}: {summary}（{status}）"),
                        })
                        .to_string())
                    }
                    Err(e) => err_reply(e),
                }
            }
            "save_settings" => {
                let mut saved: Vec<String> = Vec::new();
                let mut errors: Vec<String> = Vec::new();
                if let Some(u) = arg_str(&v, "base_url").and_then(|s| trim_nonempty(&s)) {
                    // 去尾斜杠归一（拼 path 段不出现 `//`）。
                    let normalized = u.trim_end_matches('/').to_string();
                    match host::config_set("base_url", &normalized) {
                        Ok(()) => saved.push("base_url".into()),
                        Err(e) => errors.push(format!("base_url 写入失败: {e:?}")),
                    }
                }
                if let Some(k) = arg_str(&v, "project_key").and_then(|s| trim_nonempty(&s)) {
                    match host::config_set("project_key", &k) {
                        Ok(()) => saved.push("project_key".into()),
                        Err(e) => errors.push(format!("project_key 写入失败: {e:?}")),
                    }
                }
                if let Some(m) = arg_str(&v, "max_results").and_then(|s| trim_nonempty(&s)) {
                    if m.parse::<u32>().is_ok() {
                        match host::config_set("max_results", &m) {
                            Ok(()) => saved.push("max_results".into()),
                            Err(e) => errors.push(format!("max_results 写入失败: {e:?}")),
                        }
                    } else {
                        errors.push(format!("max_results 必须是非负整数，收到: {m}"));
                    }
                }
                if let Some(p) = arg_str(&v, "pat").and_then(|s| trim_nonempty(&s)) {
                    match host::secret_set("pat", &p) {
                        Ok(()) => saved.push("PAT".into()),
                        Err(e) => errors.push(format!("PAT 写入失败: {e:?}")),
                    }
                }
                if !errors.is_empty() {
                    return err_reply(errors.join("；"));
                }
                if saved.is_empty() {
                    // 页面挂载自动探活（loadView 以空 args 触发）落这里。
                    Ok(json!({ "ok": true, "message": "未填写任何字段，未改动" }).to_string())
                } else {
                    Ok(json!({
                        "ok": true,
                        "message": format!("已保存: {}（下帧生效）", saved.join("/")),
                    })
                    .to_string())
                }
            }
            other => err_reply(format!(
                "unknown command: {other}（词表 sync/jira_get/save_settings）"
            )),
        }
    }
}

impl StepGuest for JiraFixPlugin {
    fn get_step_metadata(
    ) -> Result<exports::nemesis::plugin::flow_step::StepMetadata, HostError> {
        Ok(exports::nemesis::plugin::flow_step::StepMetadata {
            // 词表即文档：模板里 `plugin:jira-fix:<op>` 只认这两个 op。
            kinds: vec!["fetch".into(), "comment".into()],
        })
    }

    fn run_step(input: String) -> Result<String, HostError> {
        let v: Value = serde_json::from_str(&input)
            .map_err(|e| HostError::PolicyDenied(format!("input not json: {e}")))?;
        let op = v.get("op").and_then(Value::as_str).unwrap_or_default();
        match op {
            // 取数（宿主 plugin_fetch_template 直投；args = 模板首步声明，
            // 装载期已渲染 {{config.*}}；key_field/title_field/body_field 是
            // 宿主物化坐标，guest 忽略）。entries 直出 raw Jira issue 对象。
            "fetch" => {
                let jql = arg_str(&v, "jql")
                    .and_then(|s| trim_nonempty(&s))
                    .unwrap_or_else(|| "ORDER BY created DESC".into());
                let max = arg_str(&v, "max_results")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or_else(config_max_results);
                match jira_search(&jql, max) {
                    Ok(entries) => Ok(json!({
                        "status": "done",
                        "entries": entries,
                        "message": format!("检索到 {} 条", entries.len()),
                    })
                    .to_string()),
                    Err(e) => Ok(json!({ "status": "failed", "error": e }).to_string()),
                }
            }
            // 回帖（on-transition 与 report 两路入参同形 {key, body}）。
            "comment" => {
                let key = arg_str(&v, "key").and_then(|s| trim_nonempty(&s));
                let body = arg_str(&v, "body").and_then(|s| trim_nonempty(&s));
                let (Some(key), Some(body)) = (key, body) else {
                    return Ok(json!({
                        "status": "failed",
                        "error": "comment 缺参数（需 key + body）",
                    })
                    .to_string());
                };
                match jira_comment(&key, &body) {
                    Ok(_) => Ok(json!({
                        "status": "done",
                        "message": format!("已回帖 {key}"),
                    })
                    .to_string()),
                    Err(e) => Ok(json!({ "status": "failed", "error": e }).to_string()),
                }
            }
            other => Err(HostError::NotFound(format!(
                "op '{other}' not in kinds wordlist（fetch/comment）"
            ))),
        }
    }
}

nemesis_plugin_sdk::export_panel_step!(JiraFixPlugin);
