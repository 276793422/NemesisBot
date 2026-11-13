//! sc-bugfix 插件 guest（组合 world `plugin-panel-flow-step`）。
//!
//! 自持形态：插件**自带 Jira（Server/DC）+ GitLab（自托管，REST v4）+
//! Gerrit（REST）三直连**（http-send + secret-get），领域知识全部住在
//! plugin.toml 声明 + 本组件。同一组件双导出，词表全自报：
//! - 面板命令（Panel 帧，有写面）：`sync` / `jira_get`（Jira 检索与详情）
//!   + `save_settings`（config-set ×16 + secret-set ×4——实例配置与凭据
//!   的自持写通道；全空入参 = 页面挂载自动探活，诚实无写入应答）。
//! - flow 步骤（FlowStep 帧，无写面）：`fetch`（JQL 检索，entries 直出
//!   raw Jira issue 对象——宿主按模板 key_field/title_field/body_field
//!   物化）/ `comment`（Jira 回帖；on-transition 入口）/ `report`（报告
//!   回帖 + 幂等转状态；目标状态未配置 = 只回帖）/ `create_mr`（GitLab
//!   MR 创建，done 载荷顶层展开 iid/web_url——宿主 parse_publish_result
//!   直读）/ `mr_list`（宿主 reconcile 翻页直投，entries 包裹）/
//!   `change_list`（Gerrit change 检索：query-as-create 与 reconcile
//!   共用；归一投影 ChangeInfo + 首条顶层展开）。
//!
//! 凭据：`jira_pat`（Bearer）/ `jira_password`（Basic）与 `pat`
//! （GitLab PRIVATE-TOKEN；名字被宿主 publish 推送凭据推导钉死
//! `plugin/sc-bugfix/pat`，repo_sync 同源）/ `gerrit_http_pass`
//! （Gerrit HTTP Basic）经面板表单 secret-set 写 vault，调用现取现用，
//! 不落日志不出审计值。配置：三个 base_url 无声明缺省（防假缺省 URL）——
//! 未设置时各路诚实报错，不猜；egress allowlist = 三个 base_url 整条目
//! token，随配置热生效（fresh-store-per-call）。
//!
//! Jira 双认证形态（`jira_auth_mode` 配置）：`pat`（缺省，Bearer）|
//! `basic`（jira_username 配置 + jira_password secret → HTTP Basic
//! base64(user:pass)，编码器 std 内联无外部 crate）。形态未知 = 诚实
//! 报错不猜。
//!
//! REST 形态：Jira v2 search 返回**包裹对象**
//! `{issues:[{key, fields:{...}}]}`（issue key 字符串 PROJ-123 形态）；
//! GitLab v4 项目坐标 `{owner}/{repo}` 整体 percent-encode（slash → %2F），
//! MR 列表返回**顶层数组**；Gerrit REST 响应带 XSSI 魔法前缀 `)]}'`
//! （剥掉再解析），change 检索返回 ChangeInfo 数组——本组件归一投影为
//! {number, change_id, project, branch, subject, status, web_url}
//! （number = _number；web_url REST 无独立字段，按
//! `{base}/c/{project}/+/{_number}` 构造）。

use exports::nemesis::plugin::flow_step::Guest as StepGuest;
use exports::nemesis::plugin::ui::Guest as UiGuest;
use nemesis::plugin::host::{self, HostError};
use serde_json::{json, Value};

struct ScBugfixPlugin;

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

/// base URL 统一读实例配置（声明无缺省 → 只能是操作员条目）。
fn base_url(key: &str, label: &str) -> Result<String, String> {
    match host::config_get(key) {
        Ok(Some(u)) => trim_nonempty(&u)
            .map(|u| u.trim_end_matches('/').to_string())
            .ok_or_else(|| format!("{key} 已配置但为空：请在面板「实例设置」重新保存")),
        Ok(None) => Err(format!(
            "{key} 未设置：请先在面板「实例设置」表单保存 {label} 基址"
        )),
        Err(e) => Err(format!("{key} 读取失败: {e:?}")),
    }
}

fn jira_base_url() -> Result<String, String> {
    base_url("jira_base_url", "Jira")
}

fn gitlab_base_url() -> Result<String, String> {
    base_url("gitlab_base_url", "GitLab")
}

fn gerrit_base_url() -> Result<String, String> {
    base_url("gerrit_base_url", "Gerrit")
}

/// 串字段统一读实例配置（声明无缺省 → 只能是操作员条目）。
fn cfg_str(key: &str, what: &str) -> Result<String, String> {
    match host::config_get(key) {
        Ok(Some(s)) => trim_nonempty(&s)
            .ok_or_else(|| format!("{key} 已配置但为空：请在面板「实例设置」重新保存")),
        Ok(None) => Err(format!(
            "{key} 未设置：请先在面板「实例设置」表单保存 {what}"
        )),
        Err(e) => Err(format!("{key} 读取失败: {e:?}")),
    }
}

/// 凭据现取现用（vault `plugin/sc-bugfix/<name>`；缺失 = 诚实报错不猜）。
fn secret_ready(name: &str, what: &str) -> Result<String, String> {
    let v = host::secret_get(name)
        .map_err(|e| format!("{what} 未就绪：请先在面板「实例设置」表单保存（{e:?}）"))?;
    if v.trim().is_empty() {
        return Err(format!("{what} 已配置但为空：请在面板「实例设置」重新保存"));
    }
    Ok(v)
}

fn jira_pat() -> Result<String, String> {
    secret_ready("jira_pat", "Jira 访问令牌（PAT）")
}

fn jira_password() -> Result<String, String> {
    secret_ready("jira_password", "Jira 密码")
}

fn gitlab_pat() -> Result<String, String> {
    secret_ready("pat", "GitLab 访问令牌（PAT）")
}

fn gerrit_http_pass() -> Result<String, String> {
    secret_ready("gerrit_http_pass", "Gerrit HTTP Password")
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

/// 极简 base64（std-only；HTTP Basic 认证 `user:pass` 编码用——Jira basic
/// 形态与 Gerrit REST 共用）。
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b1 = chunk[0];
        let b2 = chunk.get(1).copied().unwrap_or(0);
        let b3 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b1) << 16) | (u32::from(b2) << 8) | u32::from(b3);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Jira 认证头（双形态：`pat` = Bearer 令牌（缺省）；`basic` = HTTP Basic
/// 账号密码——jira_username 配置 + jira_password secret）。形态未知 =
/// 诚实报错不猜（凭据面错配宁可响不可静默）。
fn jira_auth_header() -> Result<(String, String), String> {
    let mode = host::config_get("jira_auth_mode")
        .ok()
        .flatten()
        .and_then(|s| trim_nonempty(&s))
        .unwrap_or_else(|| "pat".into());
    match mode.to_ascii_lowercase().as_str() {
        "pat" => {
            let pat = jira_pat()?;
            Ok(("Authorization".to_string(), format!("Bearer {pat}")))
        }
        "basic" => {
            let user = cfg_str("jira_username", "Jira 账号（jira_auth_mode=basic 需要）")?;
            let pass = jira_password()?;
            Ok((
                "Authorization".to_string(),
                format!("Basic {}", b64(format!("{user}:{pass}").as_bytes())),
            ))
        }
        other => Err(format!(
            "jira_auth_mode 值无法识别「{other}」：仅支持 pat（Bearer 令牌）或 basic（账号密码）"
        )),
    }
}

/// Jira REST v2 单发请求（http-send 过帧闸：egress + 预算 + 审计；非 2xx /
/// 非 JSON = 业务错误串，由调用方决定报错形态）。认证形态按 jira_auth_mode。
fn jira_request(method: &str, path_and_query: &str, body: Option<Value>) -> Result<Value, String> {
    let base = jira_base_url()?;
    let (auth_key, auth_val) = jira_auth_header()?;
    let mut headers = vec![
        (auth_key, auth_val),
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
        let head: String = String::from_utf8_lossy(&resp.body)
            .chars()
            .take(200)
            .collect();
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
        let head: String = String::from_utf8_lossy(&resp.body)
            .chars()
            .take(200)
            .collect();
        return Err(format!("GitLab HTTP {}: {}", resp.status, head));
    }
    parse_body(&resp.body).map_err(|e| format!("GitLab 响应非 JSON: {e}"))
}

/// Gerrit REST 响应魔法前缀剥离（XSSI 防护：响应以 `)]}'` 开头 + 换行；
/// 无前缀原样返回——JSON 解析器自身容忍前导空白）。
fn strip_gerrit_prefix(body: &[u8]) -> &[u8] {
    if body.starts_with(b")]}'") {
        let rest = &body[4..];
        return match rest.first() {
            Some(b'\n') => &rest[1..],
            Some(b'\r') => {
                if rest.get(1) == Some(&b'\n') {
                    &rest[2..]
                } else {
                    rest
                }
            }
            _ => rest,
        };
    }
    body
}

/// Gerrit REST 单发请求（HTTP Basic：gerrit_username 配置 + gerrit_http_pass
/// secret——HTTP Password 在 Gerrit 设置里生成；帧闸同上）。
fn gerrit_request(
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let base = gerrit_base_url()?;
    let user = cfg_str("gerrit_username", "Gerrit HTTP 凭据用户名")?;
    let pass = gerrit_http_pass()?;
    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Basic {}", b64(format!("{user}:{pass}").as_bytes())),
        ),
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
    let resp = host::http_send(&req).map_err(|e| format!("Gerrit 请求失败: {e:?}"))?;
    if !(200..300).contains(&resp.status) {
        let head: String = String::from_utf8_lossy(&resp.body)
            .chars()
            .take(200)
            .collect();
        return Err(format!("Gerrit HTTP {}: {}", resp.status, head));
    }
    let clean = strip_gerrit_prefix(&resp.body);
    parse_body(clean).map_err(|e| format!("Gerrit 响应非 JSON: {e}"))
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

/// Jira issue 回帖（POST /issue/{key}/comment，body `{"body": <文本>}`）。
fn jira_comment(key: &str, body: &str) -> Result<Value, String> {
    jira_request(
        "POST",
        &format!("/rest/api/2/issue/{}/comment", urlencode(key)),
        Some(json!({ "body": body })),
    )
}

/// 回帖（comments 同文去重——转状态失败的续跑不刷屏）。返回是否新发。
fn jira_comment_dedup(key: &str, body: &str) -> Result<bool, String> {
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
    if already_posted {
        return Ok(false);
    }
    jira_comment(key, body)?;
    Ok(true)
}

/// 报告回帖 + 转状态（幂等）。目标状态 `None`（jira_transition_name 未配置）
/// = 只回帖不转状态，诚实报「未配置」；`Some(target)` 走三步：① issue 已在
/// 目标状态 → 直接 done（不重复回帖不重复转）；② 回帖（同文去重）；
/// ③ 按 transition `name` / `to.name` 不分大小写匹配转状态（无匹配 = failed
/// 并列出可用名单帮配对）。回帖先发、转状态后走：转失败时报告文本已送达
/// （错误注明「回帖已发出」）。
fn jira_report(key: &str, body: &str, target: Option<&str>) -> Result<String, String> {
    let Some(target) = target else {
        jira_comment_dedup(key, body)?;
        return Ok(format!(
            "{key} 已回帖（未转状态：jira_transition_name 未配置）"
        ));
    };
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
    // ② 回帖（同文去重）。
    jira_comment_dedup(key, body)?;
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
        let to = t
            .pointer("/to/name")
            .and_then(Value::as_str)
            .unwrap_or_default();
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

/// ChangeInfo → 归一投影（number/change_id/project/branch/subject/status/
/// web_url）。`_number` → `number`（宿主 result_ref 直读面）；web_url REST
/// 无独立字段，按 `{base}/c/{project}/+/{_number}` 构造（project/number
/// 缺失 = Null 诚实降级）。
fn normalize_change(base: &str, c: &Value) -> Value {
    let number = c.get("_number").cloned().unwrap_or(Value::Null);
    let num_str = number.as_i64().map(|n| n.to_string()).unwrap_or_default();
    let project = c
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let web_url = if project.is_empty() || num_str.is_empty() {
        Value::Null
    } else {
        json!(format!("{base}/c/{project}/+/{num_str}"))
    };
    json!({
        "number": number,
        "change_id": c.get("change_id").cloned().unwrap_or(Value::Null),
        "project": project,
        "branch": c.get("branch").cloned().unwrap_or(Value::Null),
        "subject": c.get("subject").cloned().unwrap_or(Value::Null),
        "status": c.get("status").cloned().unwrap_or(Value::Null),
        "web_url": web_url,
    })
}

/// Gerrit change 检索（query-as-create 与 reconcile 翻页共用）：query 必填
/// （Gerrit search 语法，如 `change:I8a3f...`）；limit/page 宿主注入 JSON
/// 数字（arg_any 双形态）映射 Gerrit 分页参数 n（页长）/ S（起始偏移 =
/// (page-1)×limit）。返回 ChangeInfo 数组（顶层数组形态）。
fn gerrit_change_list(
    query: &str,
    limit: Option<&str>,
    page: Option<&str>,
) -> Result<Vec<Value>, String> {
    let base = gerrit_base_url()?;
    let n = limit
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(25);
    let page_n = page
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|p| *p > 0)
        .unwrap_or(1);
    let start = page_n.saturating_sub(1).saturating_mul(n);
    let reply = gerrit_request(
        "GET",
        &format!("/a/changes/?q={}&n={n}&S={start}", urlencode(query)),
        None,
    )?;
    let raw = reply.as_array().cloned().unwrap_or_default();
    Ok(raw.iter().map(|c| normalize_change(&base, c)).collect())
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
fn save_secret(
    v: &Value,
    key: &str,
    label: &str,
    saved: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    if let Some(p) = arg_str(v, key).and_then(|s| trim_nonempty(&s)) {
        match host::secret_set(key, &p) {
            Ok(()) => saved.push(label.to_string()),
            Err(e) => errors.push(format!("{label} 写入失败: {e:?}")),
        }
    }
}

impl UiGuest for ScBugfixPlugin {
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
                // 枚举形态字段先验（值域钉死；留空 = 保持原值不验）。
                if let Some(s) = arg_str(&v, "code_host").and_then(|s| trim_nonempty(&s)) {
                    if s != "gitlab" && s != "gerrit" {
                        return err_reply(format!(
                            "code_host 值无法识别「{s}」：仅支持 gitlab 或 gerrit"
                        ));
                    }
                }
                if let Some(s) = arg_str(&v, "jira_auth_mode").and_then(|s| trim_nonempty(&s)) {
                    let m = s.to_ascii_lowercase();
                    if m != "pat" && m != "basic" {
                        return err_reply(format!(
                            "jira_auth_mode 值无法识别「{s}」：仅支持 pat 或 basic"
                        ));
                    }
                }
                let mut saved: Vec<String> = Vec::new();
                let mut errors: Vec<String> = Vec::new();
                save_cfg(&v, "code_host", &mut saved, &mut errors);
                save_base_url(&v, "jira_base_url", &mut saved, &mut errors);
                save_cfg(&v, "jira_project_key", &mut saved, &mut errors);
                save_cfg(&v, "jira_auth_mode", &mut saved, &mut errors);
                save_cfg(&v, "jira_username", &mut saved, &mut errors);
                save_cfg(&v, "jira_transition_name", &mut saved, &mut errors);
                save_base_url(&v, "gitlab_base_url", &mut saved, &mut errors);
                save_cfg(&v, "gitlab_owner", &mut saved, &mut errors);
                save_cfg(&v, "gitlab_repo", &mut saved, &mut errors);
                save_cfg(&v, "gitlab_target_branch", &mut saved, &mut errors);
                save_base_url(&v, "gerrit_base_url", &mut saved, &mut errors);
                save_cfg(&v, "gerrit_username", &mut saved, &mut errors);
                save_cfg(&v, "gerrit_project", &mut saved, &mut errors);
                save_cfg(&v, "gerrit_target_branch", &mut saved, &mut errors);
                save_cfg(&v, "board_project", &mut saved, &mut errors);
                save_cfg(&v, "proxy", &mut saved, &mut errors);
                save_secret(&v, "jira_pat", "Jira PAT", &mut saved, &mut errors);
                save_secret(&v, "jira_password", "Jira 密码", &mut saved, &mut errors);
                save_secret(&v, "pat", "GitLab PAT", &mut saved, &mut errors);
                save_secret(
                    &v,
                    "gerrit_http_pass",
                    "Gerrit HTTP Password",
                    &mut saved,
                    &mut errors,
                );
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

impl StepGuest for ScBugfixPlugin {
    fn get_step_metadata() -> Result<exports::nemesis::plugin::flow_step::StepMetadata, HostError> {
        Ok(exports::nemesis::plugin::flow_step::StepMetadata {
            // 词表即文档：模板里 `plugin:sc-bugfix:<op>` 只认这六个 op。
            kinds: vec![
                "fetch".into(),
                "comment".into(),
                "report".into(),
                "create_mr".into(),
                "mr_list".into(),
                "change_list".into(),
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
            // 报告回帖 + 幂等转状态：jira_transition_name 未配置 = 只回帖
            // 不转状态（诚实跳过）；配置了 = 同文去重回帖 → transition
            // 匹配转状态，转失败 = failed 注明「回帖已发出」。
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
                    .and_then(|s| trim_nonempty(&s));
                match jira_report(&key, &body, target.as_deref()) {
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
            // Gerrit change 检索（query-as-create 与 reconcile 翻页共用）：
            // query 必填；project 仅投影保留（web_url 构造优先用返回体
            // project 字段）；limit/page 宿主注入 JSON 数字（arg_any）映射
            // Gerrit n / S。归一投影 + **首条顶层展开**——宿主
            // parse_publish_result 直读 $.number/$.web_url。空结果不展开、
            // 状态仍 done：create 臂解析落空 → 对账保守分支；reconcile 臂
            // 空页 = Absent 诚实扫尽（与 mr_list 同形态）。
            "change_list" => {
                let fail = |e: String| json!({ "status": "failed", "error": e }).to_string();
                let Some(query) = arg_str(&v, "query").and_then(|s| trim_nonempty(&s)) else {
                    return Ok(fail("change_list 缺参数 query（Gerrit search 语法）".into()));
                };
                let limit = arg_any(&v, "limit").and_then(|s| trim_nonempty(&s));
                let page = arg_any(&v, "page").and_then(|s| trim_nonempty(&s));
                match gerrit_change_list(&query, limit.as_deref(), page.as_deref()) {
                    Ok(entries) => {
                        let mut out = serde_json::Map::new();
                        out.insert("status".into(), json!("done"));
                        out.insert("entries".into(), json!(entries));
                        out.insert(
                            "message".into(),
                            json!(format!("检索到 {} 条 change", entries.len())),
                        );
                        if let Some(fields) = entries.first().and_then(Value::as_object) {
                            for (k, val) in fields {
                                out.insert(k.clone(), val.clone());
                            }
                        }
                        Ok(Value::Object(out).to_string())
                    }
                    Err(e) => Ok(fail(e)),
                }
            }
            other => Err(HostError::NotFound(format!(
                "op '{other}' not in kinds wordlist（fetch/comment/report/create_mr/mr_list/change_list）"
            ))),
        }
    }
}

nemesis_plugin_sdk::export_panel_step!(ScBugfixPlugin);
