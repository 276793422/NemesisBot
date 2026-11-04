//! issue-import 插件 guest（组合 world `plugin-panel-flow-step`，0.2.0 自持批）。
//!
//! 轻量工单导入的自持缩版：插件**自带 Gitea 直连**（http-send + secret-get），
//! 退役「复用 workspace 共享 gitea connector」的 0.1.x 形态——不再依赖宿主
//! connector 子系统，宿主零领域代码。同一组件双导出，命令词表全自报：
//! - 面板命令（Panel 帧，有写面）：`save_settings`（config-set ×4 +
//!   secret-set pat——实例配置与凭据的自持写通道；全空入参 = 页面挂载自动
//!   探活，诚实无写入应答）。
//! - flow 步骤（FlowStep 帧，无写面）：两 op 词表 `fetch` / `comment`：
//!   - `fetch`：仓库 issue 检索，entries 直出 raw Gitea issue 对象（宿主按
//!     模板 key_field/title_field/body_field 物化）。
//!   - `comment`：issue 回帖（on-transition 状态回写单路入参
//!     {owner, repo, index, body}）。
//!
//! 凭据：PAT 经面板表单 secret-set 写 vault `plugin/issue-import/pat`，调用
//! 现取现用（Gitea `token ` 头——**非 Bearer**），不落日志不出审计值。配置：
//! base_url 无声明缺省（防假缺省 URL，含 /api/v1 前缀）——未设置时各路诚实
//! 报错，不猜；egress allowlist = `{{config.base_url}}` 整条目 token，随配置
//! 热生效（fresh-store-per-call）。
//!
//! Gitea REST 形态（与 bugfix-flow 同源照搬）：issues 列表返回**顶层数组**
//!（元素带 number/title/body/state）；index 走整数路径段；分页
//! `?state=&limit=&page=`（从 1 起）。

use exports::nemesis::plugin::flow_step::Guest as StepGuest;
use exports::nemesis::plugin::ui::Guest as UiGuest;
use nemesis::plugin::host::{self, HostError};
use serde_json::{json, Value};

struct IssueImportPlugin;

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
/// Gitea API 根（含 /api/v1 前缀——缺前缀落到 web 路由拿 HTML 非 JSON）。
fn base_url() -> Result<String, String> {
    match host::config_get("base_url") {
        Ok(Some(u)) => trim_nonempty(&u)
            .map(|u| u.trim_end_matches('/').to_string())
            .ok_or_else(|| "base_url 已配置但为空：请在面板「实例设置」重新保存".into()),
        Ok(None) => Err("base_url 未设置：请先在面板「实例设置」表单保存 Gitea API 根（含 /api/v1）".into()),
        Err(e) => Err(format!("base_url 读取失败: {e:?}")),
    }
}

/// PAT 现取现用（vault `plugin/issue-import/pat`；缺失 = 诚实报错不猜）。
fn gitea_pat() -> Result<String, String> {
    let pat = host::secret_get("pat")
        .map_err(|e| format!("PAT 未就绪：请先在面板「实例设置」表单保存访问令牌（{e:?}）"))?;
    if pat.trim().is_empty() {
        return Err("PAT 已配置但为空：请在面板「实例设置」重新保存访问令牌".into());
    }
    Ok(pat)
}

/// owner/repo 参数：入参留空回落实例配置（schema 有缺省，config-get 兜住）；
/// 配置也拿不到 = 诚实报错不猜。
fn cfg_repo_coord(v: &Value, key: &str) -> Result<String, String> {
    if let Some(s) = arg_str(v, key).and_then(|s| trim_nonempty(&s)) {
        return Ok(s);
    }
    match host::config_get(key) {
        Ok(Some(s)) => trim_nonempty(&s)
            .ok_or_else(|| format!("{key} 已配置但为空：请在面板「实例设置」重新保存")),
        Ok(None) => Err(format!(
            "{key} 未设置：请先在面板「实例设置」表单保存仓库坐标"
        )),
        Err(e) => Err(format!("{key} 读取失败: {e:?}")),
    }
}

/// 极简 percent-encode（RFC 3986 unreserved 保留，其余 %XX——query 段用；
/// 无外部 crate）。
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

/// query 串拼装（只收有值参数；空参数表 = 空串）。
fn query_params(params: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = params
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", urlencode(v))))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// Gitea REST 单发请求（http-send 过帧闸：egress + 预算 + 审计；非 2xx /
/// 非 JSON = 业务错误串，由调用方决定报错形态）。PAT 头 `Authorization:
/// token <PAT>`（Gitea 约定，非 Bearer）。
fn gitea_request(method: &str, path_and_query: &str, body: Option<Value>) -> Result<Value, String> {
    let base = base_url()?;
    let pat = gitea_pat()?;
    let mut headers = vec![
        ("Authorization".to_string(), format!("token {pat}")),
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
    let resp = host::http_send(&req).map_err(|e| format!("Gitea 请求失败: {e:?}"))?;
    if !(200..300).contains(&resp.status) {
        let head: String = String::from_utf8_lossy(&resp.body).chars().take(200).collect();
        return Err(format!("Gitea HTTP {}: {}", resp.status, head));
    }
    serde_json::from_slice(&resp.body).map_err(|e| format!("Gitea 响应非 JSON: {e}"))
}

/// issue 检索（列表 = 顶层数组）。
fn gitea_issues(
    owner: &str,
    repo: &str,
    state: &str,
    limit: Option<&str>,
    page: Option<&str>,
) -> Result<Vec<Value>, String> {
    let q = query_params(&[
        ("state", Some(state.to_string())),
        ("limit", limit.map(str::to_string)),
        ("page", page.map(str::to_string)),
    ]);
    let reply = gitea_request("GET", &format!("/repos/{owner}/{repo}/issues{q}"), None)?;
    Ok(reply.as_array().cloned().unwrap_or_default())
}

/// issue 回帖（POST /issues/{index}/comments，body `{"body": <文本>}`）。
fn gitea_comment(owner: &str, repo: &str, index: i64, body: &str) -> Result<Value, String> {
    gitea_request(
        "POST",
        &format!("/repos/{owner}/{repo}/issues/{index}/comments"),
        Some(json!({ "body": body })),
    )
}

/// index 入参归一（串或数字双形态；Gitea 路径段走整数）。
fn arg_index(v: &Value, key: &str) -> Option<i64> {
    let x = v.get("args")?.get(key)?;
    if let Some(s) = x.as_str() {
        return s.trim().parse().ok();
    }
    x.as_i64()
}

impl UiGuest for IssueImportPlugin {
    fn handle_ui_command(input: String) -> Result<String, HostError> {
        let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let command = v
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match command.as_str() {
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
                for key in ["owner", "repo"] {
                    if let Some(val) = arg_str(&v, key).and_then(|s| trim_nonempty(&s)) {
                        match host::config_set(key, &val) {
                            Ok(()) => saved.push(key.into()),
                            Err(e) => errors.push(format!("{key} 写入失败: {e:?}")),
                        }
                    }
                }
                if let Some(s) = arg_str(&v, "state").and_then(|s| trim_nonempty(&s)) {
                    if ["open", "closed", "all"].contains(&s.as_str()) {
                        match host::config_set("state", &s) {
                            Ok(()) => saved.push("state".into()),
                            Err(e) => errors.push(format!("state 写入失败: {e:?}")),
                        }
                    } else {
                        errors.push(format!("state 必须是 open/closed/all，收到: {s}"));
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
            other => err_reply(format!("unknown command: {other}（词表 save_settings）")),
        }
    }
}

impl StepGuest for IssueImportPlugin {
    fn get_step_metadata(
    ) -> Result<exports::nemesis::plugin::flow_step::StepMetadata, HostError> {
        Ok(exports::nemesis::plugin::flow_step::StepMetadata {
            // 词表即文档：模板里 `plugin:issue-import:<op>` 只认这两个 op。
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
            // 宿主物化坐标，guest 忽略）。entries 直出 raw Gitea issue 对象。
            "fetch" => {
                let fail = |e: String| json!({ "status": "failed", "error": e }).to_string();
                let owner = match cfg_repo_coord(&v, "owner") {
                    Ok(o) => o,
                    Err(e) => return Ok(fail(e)),
                };
                let repo = match cfg_repo_coord(&v, "repo") {
                    Ok(r) => r,
                    Err(e) => return Ok(fail(e)),
                };
                let state = arg_str(&v, "state")
                    .and_then(|s| trim_nonempty(&s))
                    .unwrap_or_else(|| "open".into());
                let limit = arg_str(&v, "limit").and_then(|s| trim_nonempty(&s));
                match gitea_issues(&owner, &repo, &state, limit.as_deref(), None) {
                    Ok(entries) => Ok(json!({
                        "status": "done",
                        "entries": entries,
                        "message": format!("检索到 {} 条", entries.len()),
                    })
                    .to_string()),
                    Err(e) => Ok(fail(e)),
                }
            }
            // 回帖（on-transition 状态回写单路，入参 {owner, repo, index, body}）。
            "comment" => {
                let fail = |e: String| json!({ "status": "failed", "error": e }).to_string();
                let owner = match cfg_repo_coord(&v, "owner") {
                    Ok(o) => o,
                    Err(e) => return Ok(fail(e)),
                };
                let repo = match cfg_repo_coord(&v, "repo") {
                    Ok(r) => r,
                    Err(e) => return Ok(fail(e)),
                };
                let Some(index) = arg_index(&v, "index") else {
                    return Ok(fail("comment 缺参数或 index 非整数（需 owner/repo/index/body）".into()));
                };
                let Some(body) = arg_str(&v, "body").and_then(|s| trim_nonempty(&s)) else {
                    return Ok(fail("comment 缺参数 body".into()));
                };
                match gitea_comment(&owner, &repo, index, &body) {
                    Ok(_) => Ok(json!({
                        "status": "done",
                        "message": format!("已回帖 {owner}/{repo}#{index}"),
                    })
                    .to_string()),
                    Err(e) => Ok(fail(e)),
                }
            }
            other => Err(HostError::NotFound(format!(
                "op '{other}' not in kinds wordlist（fetch/comment）"
            ))),
        }
    }
}

nemesis_plugin_sdk::export_panel_step!(IssueImportPlugin);
