//! jira-gitlab-flow 插件 guest（组合 world `plugin-panel-flow-step`）。
//!
//! 自持形态：插件**自带 Jira（Server/DC，PAT Bearer）+ GitLab（自托管，
//! REST v4，PRIVATE-TOKEN）双直连**（http-send + secret-get），领域知识
//! 全部住在 plugin.toml 声明 + 本组件。同一组件双导出，词表全自报：
//! - 面板命令（Panel 帧，有写面）：`sync` / `jira_get`（Jira 检索与详情）
//!   + `save_settings`（config-set ×8 + secret-set ×2——实例配置与双凭据
//!   的自持写通道；全空入参 = 页面挂载自动探活，诚实无写入应答）。
//! - flow 步骤（FlowStep 帧，无写面）：`fetch`（JQL 检索，entries 直出
//!   raw Jira issue 对象——宿主按模板 key_field/title_field/body_field
//!   物化）/ `comment`（Jira 回帖；on-transition 入口）/ `report`（报告
//!   回帖 + 幂等转状态三步）/ `create_mr`（GitLab MR 创建，done 载荷
//!   顶层展开 iid/web_url——宿主 parse_publish_result 直读）/
//!   `mr_list`（宿主 reconcile 翻页直投，entries 包裹）。
//!
//! 凭据：`jira_pat`（Bearer）与 `pat`（GitLab PRIVATE-TOKEN；名字被宿主
//! publish 推送凭据推导钉死 `plugin/jira-gitlab-flow/pat`，repo_sync
//! 凭据别名同源）经面板表单 secret-set 写 vault，调用现取现用，不落
//! 日志不出审计值。配置：两个 base_url 无声明缺省（防假缺省 URL）——
//! 未设置时各路诚实报错，不猜；egress allowlist = `{{config.jira_base_url}}`
//! + `{{config.gitlab_base_url}}` 整条目 token，随配置热生效
//! （fresh-store-per-call）。
//!
//! REST 形态：Jira v2 search 返回**包裹对象** `{issues:[{key, fields:{...}}]}`
//! （issue key 字符串 PROJ-123 形态，无整数序号）；GitLab v4 项目坐标
//! `{owner}/{repo}` 整体 percent-encode（slash → %2F），MR 列表返回
//! **顶层数组**（非包裹对象）。

use exports::nemesis::plugin::flow_step::Guest as StepGuest;
use exports::nemesis::plugin::ui::Guest as UiGuest;
use nemesis::plugin::host::{self, HostError};
use serde_json::{json, Value};

struct JiraGitlabFlowPlugin;

/// 入参投影取 `args.<key>` 串字段（各路入参统一形态 `{"op"?, "args": {...}}`）。
fn arg_str(v: &Value, key: &str) -> Option<String> {
    v.get("args")?.get(key)?.as_str().map(str::to_string)
}

/// 串或数字双形态兼容（宿主 publish 翻页注入 limit/page 为 JSON 数字，模板
/// 基参写的是字符串——两形态都要认）。
fn arg_any(v: &Value, key: &str) -> Option<String> {
    let x = v.get("args")?.get(key)?;
    if let Some(s) = x.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = x.as_i64() {
        return Some(n.to_string());
    }
    x.as_u64().map(|n| n.to_string())
}

/// trim 后非空才要（空串视作未填写——表单留空 = 保持原值）。
fn trim_nonempty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn err_reply(msg: impl std::fmt::Display) -> Result<String, HostError> {
    Ok(json!({ "error": msg.to_string() }).to_string())
}

/// GitLab 仓库坐标（owner/repo）：模板 args 优先，实例配置兜底（面板
/// 「实例设置」持久条目——缺省值 group/project 只服务装载期渲染）。
fn cfg_repo_coord(v: &Value, arg_key: &str, cfg_key: &str) -> Result<String, String> {
    if let Some(s) = arg_str(v, arg_key).and_then(|s| trim_nonempty(&s)) {
        return Ok(s);
    }
    match host::config_get(cfg_key) {
        Ok(Some(s)) => trim_nonempty(&s)
            .ok_or_else(|| format!("{cfg_key} 已配置但为空：请在面板「实例设置」重新保存")),
        Ok(None) => Err(format!(
            "{cfg_key} 未设置：请先在面板「实例设置」表单保存 GitLab 仓库坐标"
        )),
        Err(e) => Err(format!("{cfg_key} 读取失败: {e:?}")),
    }
}

/// Jira 基址统一读实例配置（声明无缺省 → 只能是操作员条目）。
fn jira_base_url() -> Result<String, String> {
    match host::config_get("jira_base_url") {
        Ok(Some(u)) => trim_nonempty(&u)
            .map(|u| u.trim_end_matches('/').to_string())
            .ok_or_else(|| "jira_base_url 已配置但为空：请在面板「实例设置」重新保存".into()),
        Ok(None) => Err("jira_base_url 未设置：请先在面板「实例设置」表单保存 Jira 基址".into()),
        Err(e) => Err(format!("jira_base_url 读取失败: {e:?}")),
    }
}

/// GitLab 基址统一读实例配置（声明无缺省 → 只能是操作员条目）。
fn gitlab_base_url() -> Result<String, String> {
    match host::config_get("gitlab_base_url") {
        Ok(Some(u)) => trim_nonempty(&u)
            .map(|u| u.trim_end_matches('/').to_string())
            .ok_or_else(|| "gitlab_base_url 已配置但为空：请在面板「实例设置」重新保存".into()),
        Ok(None) => Err(
            "gitlab_base_url 未设置：请先在面板「实例设置」表单保存 GitLab 基址".into(),
        ),
        Err(e) => Err(format!("gitlab_base_url 读取失败: {e:?}")),
    }
}

/// Jira PAT 现取现用（vault `plugin/jira-gitlab-flow/jira_pat`；缺失 = 诚实
/// 报错不猜）。
fn jira_pat() -> Result<String, String> {
    let pat = host::secret_get("jira_pat")
        .map_err(|e| format!("Jira PAT 未就绪：请先在面板「实例设置」表单保存 Jira 访问令牌（{e:?}）"))?;
    if pat.trim().is_empty() {
        return Err("Jira PAT 已配置但为空：请在面板「实例设置」重新保存 Jira 访问令牌".into());
    }
    Ok(pat)
}

/// GitLab PAT 现取现用（vault `plugin/jira-gitlab-flow/pat`——名字被宿主
/// publish 推送凭据推导钉死，repo_sync 同源；缺失 = 诚实报错不猜）。
fn gitlab_pat() -> Result<String, String> {
    let pat = host::secret_get("pat")
        .map_err(|e| format!("GitLab PAT 未就绪：请先在面板「实例设置」表单保存 GitLab 访问令牌（{e:?}）"))?;
    if pat.trim().is_empty() {
        return Err("GitLab PAT 已配置但为空：请在面板「实例设置」重新保存 GitLab 访问令牌".into());
    }
    Ok(pat)
}

/// 极简 percent-encode（RFC 3986 unreserved 保留，其余 %XX——JQL/issue key
/// /项目坐标 进 query/path 段用；无外部 crate）。
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
/// 非 JSON = 业务错误串，由调用方决定报错形态）。PAT Bearer 头。
fn jira_request(method: &str, path_and_query: &str, body: Option<Value>) -> Result<Value, String> {
    let base = jira_base_url()?;
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
    // 2xx 空 body = 合法 no-content（Jira transitions POST 恒 204；RFC 7230
    // 禁 204 带 body，客户端侧恒收到空字节）——不得走 JSON 解析误报 EOF。
    parse_body(&resp.body).map_err(|e| format!("Jira 响应非 JSON: {e}"))
}

/// GitLab REST v4 单发请求（PRIVATE-TOKEN 头；帧闸同上）。
fn gitlab_request(
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let base = gitlab_base_url()?;
    let pat = gitlab_pat()?;
    let mut headers = vec![
        ("PRIVATE-TOKEN".to_string(), pat),
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
    let resp = host::http_send(&req).map_err(|e| format!("GitLab 请求失败: {e:?}"))?;
    if !(200..300).contains(&resp.status) {
        let head: String = String::from_utf8_lossy(&resp.body).chars().take(200).collect();
        return Err(format!("GitLab HTTP {}: {}", resp.status, head));
    }
    parse_body(&resp.body).map_err(|e| format!("GitLab 响应非 JSON: {e}"))
}

/// 2xx body 解析：空/全空白 = `Value::Null`（no-content 2xx 合法，见
/// jira_request 注记）；非空才走 serde。
fn parse_body(body: &[u8]) -> Result<Value, String> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body).map_err(|e| e.to_string())
}

/// GitLab 项目坐标：`{owner}/{repo}` 整体 percent-encode（slash → %2F，
/// REST v4 项目 path 形态）。
fn project_path(v: &Value) -> Result<String, String> {
    let owner = cfg_repo_coord(v, "owner", "gitlab_owner")?;
    let repo = cfg_repo_coord(v, "repo", "gitlab_repo")?;
    Ok(urlencode(&format!("{owner}/{repo}")))
}

/// JQL 检索（search 包裹对象 → issues 数组直出）。
fn jira_search(jql: &str, max: u32) -> Result<Vec<Value>, String> {
    let reply = jira_request(
        "GET",
        &format!(
            "/rest/api/2/search?jql={}&maxResults={max}&fields=key,summary,status",
            urlencode(jql)
        ),
        None,
    )?;
    Ok(reply
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Jira issue 回帖（POST /issue/{key}/comment，body `{"body": <文本>}`）。
fn jira_comment(key: &str, body: &str) -> Result<Value, String> {
    jira_request(
        "POST",
        &format!("/rest/api/2/issue/{}/comment", urlencode(key)),
        Some(json!({ "body": body })),
    )
}

/// 报告回帖 + 转状态（幂等三步）：① issue 已在目标状态 → 直接 done（不重复
/// 回帖不重复转）；② 回帖（comments 同文去重——上次转状态失败的续跑不刷屏）；
/// ③ 按 transition `name` / `to.name` 不分大小写匹配转状态（无匹配 = failed
/// 并列出可用名单帮配对）。回帖先发、转状态后走：转失败时报告文本已送达
/// （错误注明「回帖已发出」）。
fn jira_report(key: &str, body: &str, target: &str) -> Result<String, String> {
    // ① 幂等闸：已在目标状态 → 不重复回帖不重复转。
    let cur = jira_request(
        "GET",
        &format!("/rest/api/2/issue/{}?fields=status", urlencode(key)),
        None,
    )?;
    let status = cur
        .pointer("/fields/status/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if status.eq_ignore_ascii_case(target) {
        return Ok(format!("{key} 已在目标状态「{target}」，跳过重复报告"));
    }
    // ② 回帖（comments 同文去重）。
    let comments = jira_request(
        "GET",
        &format!("/rest/api/2/issue/{}/comment", urlencode(key)),
        None,
    )?;
    let already_posted = comments
        .get("comments")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .any(|c| c.get("body").and_then(Value::as_str) == Some(body))
        })
        .unwrap_or(false);
    if !already_posted {
        jira_comment(key, body)?;
    }
    // ③ 转状态（name 或 to.name 不分大小写匹配）。
    let trans = jira_request(
        "GET",
        &format!("/rest/api/2/issue/{}/transitions", urlencode(key)),
        None,
    )?;
    let list = trans
        .get("transitions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let found = list.iter().find(|t| {
        let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
        let to = t.pointer("/to/name").and_then(Value::as_str).unwrap_or_default();
        name.eq_ignore_ascii_case(target) || to.eq_ignore_ascii_case(target)
    });
    let Some(t) = found else {
        let names: Vec<String> = list
            .iter()
            .map(|t| {
                let name = t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let to = t
                    .pointer("/to/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if to.is_empty() || to == name {
                    name
                } else {
                    format!("{name} → {to}")
                }
            })
            .collect();
        let available = if names.is_empty() {
            "（接口未返回任何 transition）".to_string()
        } else {
            names.join("、")
        };
        return Err(format!(
            "回帖已发出，但转状态失败：未找到通往「{target}」的 transition（可用：{available}；若工作流需多步跳转请调整 jira_transition_name 或先人工转中间态）"
        ));
    };
    let tid = t
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    jira_request(
        "POST",
        &format!("/rest/api/2/issue/{}/transitions", urlencode(key)),
        Some(json!({ "transition": { "id": tid } })),
    )?;
    Ok(format!("已回帖并转状态 {key} → {target}"))
}

/// GitLab MR 创建（POST /projects/{enc}/merge_requests；响应顶层含
/// iid/web_url/state——宿主 parse_publish_result 直读）。
fn gitlab_create_mr(
    v: &Value,
    source: &str,
    target: &str,
    title: &str,
    description: Option<&str>,
) -> Result<Value, String> {
    let enc = project_path(v)?;
    let mut payload = json!({
        "source_branch": source,
        "target_branch": target,
        "title": title,
    });
    if let Some(d) = description {
        payload["description"] = json!(d);
    }
    gitlab_request(
        "POST",
        &format!("/api/v4/projects/{enc}/merge_requests"),
        Some(payload),
    )
}

/// MR 列表（宿主 reconcile 翻页直投：state 模板基参字符串 + limit/page 宿主
/// 注入 JSON 数字——arg_any 双形态兼容；GitLab 返回顶层数组，entries 包裹
/// 交宿主页体解析）。
fn gitlab_mr_list(
    v: &Value,
    state: &str,
    limit: Option<&str>,
    page: Option<&str>,
) -> Result<Vec<Value>, String> {
    let enc = project_path(v)?;
    let mut q = format!("/api/v4/projects/{enc}/merge_requests?state={state}");
    if let Some(l) = limit {
        q.push_str(&format!("&per_page={l}"));
    }
    if let Some(p) = page {
        q.push_str(&format!("&page={p}"));
    }
    let reply = gitlab_request("GET", &q, None)?;
    Ok(reply.as_array().cloned().unwrap_or_default())
}

/// 逐字段写配置（有值才写；写失败聚合报错）。
fn save_cfg(v: &Value, key: &str, saved: &mut Vec<String>, errors: &mut Vec<String>) {
    if let Some(s) = arg_str(v, key).and_then(|s| trim_nonempty(&s)) {
        match host::config_set(key, &s) {
            Ok(()) => saved.push(key.to_string()),
            Err(e) => errors.push(format!("{key} 写入失败: {e:?}")),
        }
    }
}

/// base URL 形态字段：去尾斜杠归一后写（拼 path 段不出现 `//`）。
fn save_base_url(v: &Value, key: &str, saved: &mut Vec<String>, errors: &mut Vec<String>) {
    if let Some(u) = arg_str(v, key).and_then(|s| trim_nonempty(&s)) {
        let normalized = u.trim_end_matches('/').to_string();
        match host::config_set(key, &normalized) {
            Ok(()) => saved.push(key.to_string()),
            Err(e) => errors.push(format!("{key} 写入失败: {e:?}")),
        }
    }
}

/// 凭据写 vault（有值才写；字段名 = secret 别名）。
fn save_secret(v: &Value, key: &str, label: &str, saved: &mut Vec<String>, errors: &mut Vec<String>) {
    if let Some(p) = arg_str(v, key).and_then(|s| trim_nonempty(&s)) {
        match host::secret_set(key, &p) {
            Ok(()) => saved.push(label.to_string()),
            Err(e) => errors.push(format!("{label} 写入失败: {e:?}")),
        }
    }
}

impl UiGuest for JiraGitlabFlowPlugin {
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
                match jira_search(&jql, 50) {
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
                save_base_url(&v, "jira_base_url", &mut saved, &mut errors);
                save_cfg(&v, "jira_project_key", &mut saved, &mut errors);
                save_cfg(&v, "jira_transition_name", &mut saved, &mut errors);
                save_base_url(&v, "gitlab_base_url", &mut saved, &mut errors);
                save_cfg(&v, "gitlab_owner", &mut saved, &mut errors);
                save_cfg(&v, "gitlab_repo", &mut saved, &mut errors);
                save_cfg(&v, "gitlab_target_branch", &mut saved, &mut errors);
                save_cfg(&v, "board_project", &mut saved, &mut errors);
                save_secret(&v, "jira_pat", "Jira PAT", &mut saved, &mut errors);
                save_secret(&v, "pat", "GitLab PAT", &mut saved, &mut errors);
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

impl StepGuest for JiraGitlabFlowPlugin {
    fn get_step_metadata(
    ) -> Result<exports::nemesis::plugin::flow_step::StepMetadata, HostError> {
        Ok(exports::nemesis::plugin::flow_step::StepMetadata {
            // 词表即文档：模板里 `plugin:jira-gitlab-flow:<op>` 只认这五个 op。
            kinds: vec![
                "fetch".into(),
                "comment".into(),
                "report".into(),
                "create_mr".into(),
                "mr_list".into(),
            ],
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
                    .unwrap_or(20);
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
            // 回帖（on-transition 入口；入参 {key, body}）。
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
            // 报告回帖 + 幂等转状态（目标状态 = jira_transition_name 配置，
            // 缺省 "Done"；声明缺省只服务装载期渲染，运行期 guest 自兜底）。
            "report" => {
                let fail = |e: String| json!({ "status": "failed", "error": e }).to_string();
                let Some(key) = arg_str(&v, "key").and_then(|s| trim_nonempty(&s)) else {
                    return Ok(fail("report 缺参数 key（Jira issue 键）".into()));
                };
                let Some(body) = arg_str(&v, "body").and_then(|s| trim_nonempty(&s)) else {
                    return Ok(fail("report 缺参数 body（报告正文）".into()));
                };
                let target = host::config_get("jira_transition_name")
                    .ok()
                    .flatten()
                    .and_then(|s| trim_nonempty(&s))
                    .unwrap_or_else(|| "Done".into());
                match jira_report(&key, &body, &target) {
                    Ok(msg) => Ok(json!({ "status": "done", "message": msg }).to_string()),
                    Err(e) => Ok(fail(e)),
                }
            }
            // 建 MR（done 载荷 = status + raw MR 对象字段顶层展开——宿主
            // JSON path 直读 iid/web_url/state）。
            "create_mr" => {
                let fail = |e: String| json!({ "status": "failed", "error": e }).to_string();
                let source = arg_str(&v, "source_branch").and_then(|s| trim_nonempty(&s));
                let target = arg_str(&v, "target_branch").and_then(|s| trim_nonempty(&s));
                let title = arg_str(&v, "title").and_then(|s| trim_nonempty(&s));
                let (Some(source), Some(target), Some(title)) = (source, target, title) else {
                    return Ok(fail(
                        "create_mr 缺参数（需 owner/repo/source_branch/target_branch/title）".into(),
                    ));
                };
                let description = arg_str(&v, "description").and_then(|s| trim_nonempty(&s));
                match gitlab_create_mr(&v, &source, &target, &title, description.as_deref()) {
                    Ok(mr) => {
                        // 顶层展开：status 打头，raw MR 对象字段随行（宿主
                        // JSON path 直读 iid/web_url）。
                        let mut out = serde_json::Map::new();
                        out.insert("status".into(), json!("done"));
                        if let Some(fields) = mr.as_object() {
                            for (k, val) in fields {
                                out.insert(k.clone(), val.clone());
                            }
                        } else {
                            return Ok(fail("create_mr 响应非对象（无法展开 MR 字段）".into()));
                        }
                        Ok(Value::Object(out).to_string())
                    }
                    Err(e) => Ok(fail(e)),
                }
            }
            // MR 列表（宿主 reconcile 翻页直投：owner/repo/state 模板基参
            // 字符串 + limit/page 宿主注入 JSON 数字——arg_any 双形态兼容）。
            "mr_list" => {
                let fail = |e: String| json!({ "status": "failed", "error": e }).to_string();
                let state = arg_str(&v, "state")
                    .and_then(|s| trim_nonempty(&s))
                    .unwrap_or_else(|| "all".into());
                let limit = arg_any(&v, "limit").and_then(|s| trim_nonempty(&s));
                let page = arg_any(&v, "page").and_then(|s| trim_nonempty(&s));
                match gitlab_mr_list(&v, &state, limit.as_deref(), page.as_deref()) {
                    Ok(entries) => Ok(json!({
                        "status": "done",
                        "entries": entries,
                        "message": format!("检索到 {} 条 MR", entries.len()),
                    })
                    .to_string()),
                    Err(e) => Ok(fail(e)),
                }
            }
            other => Err(HostError::NotFound(format!(
                "op '{other}' not in kinds wordlist（fetch/comment/report/create_mr/mr_list）"
            ))),
        }
    }
}

nemesis_plugin_sdk::export_panel_step!(JiraGitlabFlowPlugin);
