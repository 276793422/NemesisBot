use super::*;

const VALID_REST: &str = r#"
[connector]
id = "paper-broker"
name = "纸面券商"
version = "0.1.0"
kind = "rest"
description = "零账号纸面下单 demo"

[connector.http]
base = "https://quotes.example.com"

[connector.credentials]
api_key = { required = false, into = "header:X-API-Key" }

[[op]]
id = "get_quote"
intent = ["查询行情", "现在多少钱"]
risk = "low"
class = "read"
http = { method = "GET", path = "/quote?symbol={symbol}" }

[[op]]
id = "place_order"
intent = ["买入", "卖出"]
risk = "critical"
class = "write"
confirm = "required"
budget = "finance"
receipt = "order_id"
http = { method = "POST", path = "/orders" }
args_schema = { type = "object", required = ["symbol", "qty"] }
"#;

const VALID_LOCAL: &str = r#"
[connector]
id = "local-fs"
name = "本地文件整理"
version = "0.1.0"
kind = "local"

[[op]]
id = "scan"
risk = "low"
class = "read"
tool = "exec"

[[op]]
id = "apply"
risk = "high"
class = "write"
confirm = "required"
action = "builtin.demo"
"#;

#[test]
fn parses_valid_rest_manifest() {
    let c = parse_connector_manifest(VALID_REST).expect("valid rest manifest");
    assert_eq!(c.id, "paper-broker");
    assert_eq!(c.kind, ConnectorKind::Rest);
    assert!(c.enabled);
    assert_eq!(c.base_url.as_deref(), Some("https://quotes.example.com"));
    assert_eq!(c.credentials.len(), 1);
    let cred = c.credentials.get("api_key").expect("api_key slot");
    assert!(!cred.required);
    assert!(matches!(cred.into, CredentialInto::Header(ref h) if h == "X-API-Key"));

    assert_eq!(c.ops.len(), 2);
    let q = &c.ops[0];
    assert_eq!(q.id, "get_quote");
    assert_eq!(q.risk, RiskLevel::Low);
    assert_eq!(q.class, OpClass::Read);
    assert!(!q.confirm_required);
    match &q.binding {
        OpBinding::Http {
            method,
            path,
            placeholders,
        } => {
            assert_eq!(*method, HttpMethod::Get);
            assert_eq!(path, "/quote?symbol={symbol}");
            assert_eq!(placeholders, &vec!["symbol".to_string()]);
        }
        other => panic!("expected http binding, got {other:?}"),
    }

    let p = &c.ops[1];
    assert!(p.confirm_required);
    assert!(p.finance_budget);
    assert_eq!(p.receipt_key.as_deref(), Some("order_id"));
    assert_eq!(p.risk, RiskLevel::Critical);
    assert!(p.args_schema.is_some());
}

#[test]
fn parses_local_kind_with_tool_and_action_bindings() {
    let c = parse_connector_manifest(VALID_LOCAL).expect("valid local manifest");
    assert_eq!(c.kind, ConnectorKind::Local);
    match &c.ops[0].binding {
        OpBinding::Tool { tool } => assert_eq!(tool, "exec"),
        other => panic!("expected tool binding, got {other:?}"),
    }
    match &c.ops[1].binding {
        OpBinding::Action { action } => assert_eq!(action, "builtin.demo"),
        other => panic!("expected action binding, got {other:?}"),
    }
    assert!(c.ops[1].confirm_required);
}

#[test]
fn rejects_unknown_field_loud() {
    let bad = VALID_REST.replace("kind = \"rest\"", "kind = \"rest\"\nkool = \"typo\"");
    let err = parse_connector_manifest(&bad).expect_err("unknown field must be loud");
    assert!(err.contains("TOML 解析失败"), "got: {err}");
}

#[test]
fn rejects_bad_connector_id_charset() {
    let bad = VALID_REST.replace("id = \"paper-broker\"", "id = \"Paper.Broker!\"");
    assert!(parse_connector_manifest(&bad).is_err(), "大写/点号必须拒绝");
}

#[test]
fn rejects_windows_reserved_device_names_as_ids() {
    // id 同时派生注册目录与 sidecar 文件名——保留设备名在 Windows 上连
    // 目录/文件创建都被 OS 拒（报错还难懂），清单层诚实拒绝带原因。
    for reserved in ["con", "nul", "com1", "lpt3"] {
        let bad = VALID_LOCAL.replace("id = \"local-fs\"", &format!("id = \"{reserved}\""));
        let err =
            parse_connector_manifest(&bad).expect_err(&format!("保留名 `{reserved}` 必须拒绝"));
        assert!(err.contains("Windows 保留设备名"), "`{reserved}`: {err}");
    }
    // 精确比对不误伤：连字符/下划线组合与超集名都合法（保留名按「首个
    // 点之前」整段判定，本字符集无点 → 整段即基名）。
    for ok in ["con-x", "con_x", "console", "nullify", "com10"] {
        let good = VALID_LOCAL.replace("id = \"local-fs\"", &format!("id = \"{ok}\""));
        parse_connector_manifest(&good).unwrap_or_else(|e| panic!("`{ok}` 不应误伤: {e}"));
    }
}

#[test]
fn rejects_duplicate_op_ids() {
    let dup = format!(
        "{VALID_LOCAL}\n[[op]]\nid = \"scan\"\nrisk = \"low\"\nclass = \"read\"\ntool = \"exec\"\n"
    );
    let err = parse_connector_manifest(&dup).expect_err("duplicate op");
    assert!(err.contains("重复"), "got: {err}");
}

#[test]
fn mixed_bindings_across_kind_are_legal() {
    // kind 只是描述性分组；混合绑定（http + action）合法——paper-broker 形态
    let toml_text = r#"
[connector]
id = "mixed"
name = "混合"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "https://q.example.com"

[[op]]
id = "get_quote"
risk = "low"
class = "network"
http = { method = "GET", path = "/quote?symbol={symbol}" }

[[op]]
id = "place_order"
risk = "critical"
class = "write"
confirm = "required"
budget = "finance"
action = "builtin.paper_order"
"#;
    let c = parse_connector_manifest(toml_text).expect("mixed binding");
    assert!(matches!(c.ops[0].binding, OpBinding::Http { .. }));
    assert!(matches!(c.ops[1].binding, OpBinding::Action { .. }));
}

#[test]
fn rejects_missing_binding_and_multiple_bindings() {
    let no_binding = VALID_LOCAL.replace("tool = \"exec\"", "");
    let err = parse_connector_manifest(&no_binding).expect_err("no binding");
    assert!(err.contains("缺执行绑定"), "got: {err}");

    let both = VALID_LOCAL.replace("tool = \"exec\"", "tool = \"exec\"\naction = \"x.y\"");
    let err = parse_connector_manifest(&both).expect_err("two bindings");
    assert!(err.contains("只能声明一个"), "got: {err}");
}

#[test]
fn rejects_rest_without_http_base() {
    let bad = VALID_REST.replace(
        "[connector.http]\nbase = \"https://quotes.example.com\"\n",
        "",
    );
    let err = parse_connector_manifest(&bad).expect_err("rest without base");
    assert!(err.contains("[connector.http].base"), "got: {err}");
}

#[test]
fn rejects_invalid_path_placeholders() {
    for bad_path in [
        "/x/{Bad Name}",
        "/x/{unclosed",
        "/x/closed}",
        "/x/{}",
        "/x/{a-b}",
    ] {
        let bad = VALID_REST.replace("/quote?symbol={symbol}", bad_path);
        let err = parse_connector_manifest(&bad).expect_err(bad_path);
        assert!(err.contains("http path 非法"), "path {bad_path}: {err}");
    }
    // 合法占位符照常通过
    let ok = VALID_REST.replace("/quote?symbol={symbol}", "/a/{x1}/{y_2}");
    let c = parse_connector_manifest(&ok).expect("legal placeholders");
    match &c.ops[0].binding {
        OpBinding::Http { placeholders, .. } => {
            assert_eq!(placeholders, &vec!["x1".to_string(), "y_2".to_string()])
        }
        other => panic!("expected http binding, got {other:?}"),
    }
}

#[test]
fn rejects_bad_confirm_and_budget_values() {
    let bad_confirm = VALID_REST.replace("confirm = \"required\"", "confirm = \"maybe\"");
    let err = parse_connector_manifest(&bad_confirm).expect_err("bad confirm");
    assert!(err.contains("confirm"), "got: {err}");

    let bad_budget = VALID_REST.replace("budget = \"finance\"", "budget = \"money\"");
    let err = parse_connector_manifest(&bad_budget).expect_err("bad budget");
    assert!(err.contains("budget"), "got: {err}");
}

#[test]
fn parses_credential_into_query_form() {
    let toml_text = r#"
[connector]
id = "monitor"
name = "监控"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "https://m.example.com"

[connector.credentials]
key = { required = true, into = "query:key" }

[[op]]
id = "fetch"
risk = "low"
class = "network"
http = { method = "GET", path = "/rss" }
"#;
    let c = parse_connector_manifest(toml_text).expect("monitor manifest");
    let cred = c.credentials.get("key").expect("key slot");
    assert!(cred.required);
    assert!(matches!(cred.into, CredentialInto::Query(ref q) if q == "key"));
}

#[test]
fn rejects_credential_into_bad_scheme() {
    let toml_text = r#"
[connector]
id = "monitor"
name = "监控"
version = "0.1.0"
kind = "rest"

[connector.credentials]
key = { into = "body:key" }

[[op]]
id = "fetch"
risk = "low"
class = "network"
http = { method = "GET", path = "/rss" }
"#;
    let err = parse_connector_manifest(toml_text).expect_err("bad into scheme");
    assert!(err.contains("into"), "got: {err}");
}

#[test]
fn rejects_empty_ops() {
    let toml_text = r#"
[connector]
id = "empty"
name = "空"
version = "0.1.0"
kind = "local"
"#;
    let err = parse_connector_manifest(toml_text).expect_err("no ops");
    assert!(err.contains("至少要声明一个"), "got: {err}");
}

#[test]
fn risk_level_rank_is_total_order() {
    assert!(RiskLevel::Low.rank() < RiskLevel::Medium.rank());
    assert!(RiskLevel::Medium.rank() < RiskLevel::High.rank());
    assert!(RiskLevel::High.rank() < RiskLevel::Critical.rank());
    assert_eq!(RiskLevel::Critical.as_str(), "critical");
    assert_eq!(OpClass::Network.as_str(), "network");
}

// ---- 收官批 A 段：HttpMethod::Patch / 静态头 / 凭据 prefix ----

#[test]
fn parses_patch_method_and_static_headers() {
    let toml_text = r#"
[connector]
id = "gitea"
name = "Gitea"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:3000"
[connector.http.headers]
User-Agent = "nemesisbot-connector/0.1"
Accept = "application/json"

[connector.credentials]
pat = { required = true, into = "header:Authorization", prefix = "token " }

[[op]]
id = "issue_close"
risk = "high"
class = "write"
confirm = "required"
http = { method = "PATCH", path = "/repos/{owner}/{repo}/issues/{index}" }
"#;
    let c = parse_connector_manifest(toml_text).expect("gitea-ish manifest");
    match &c.ops[0].binding {
        OpBinding::Http { method, .. } => assert_eq!(*method, HttpMethod::Patch),
        other => panic!("expected http binding, got {other:?}"),
    }
    assert_eq!(c.static_headers.len(), 2);
    assert_eq!(
        c.static_headers.get("User-Agent").map(String::as_str),
        Some("nemesisbot-connector/0.1")
    );
    let pat = c.credentials.get("pat").expect("pat slot");
    assert_eq!(pat.prefix.as_deref(), Some("token "));
    assert!(matches!(pat.into, CredentialInto::Header(ref h) if h == "Authorization"));
}

#[test]
fn rejects_crlf_in_static_headers_and_prefix() {
    let base = |headers_block: &str, prefix_line: &str| {
        format!(
            r#"
[connector]
id = "gitea"
name = "Gitea"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "http://127.0.0.1:3000"
{headers_block}

[connector.credentials]
pat = {{ into = "header:Authorization", prefix = "{prefix_line}" }}

[[op]]
id = "x"
risk = "low"
class = "read"
http = {{ method = "GET", path = "/x" }}
"#
        )
    };
    // 静态头名带空白 = 头注入面，loud 拒（TOML 引号键形态可携带空白）
    let bad = base("[connector.http.headers]\n\"X- Evil\" = \"a\"\n", "token ");
    let err = parse_connector_manifest(&bad).expect_err("bad header name");
    assert!(err.contains("静态头"), "got: {err}");

    // prefix 带 CR = 头注入，loud 拒（value 内 \r 用 TOML 转义形态写入）
    let bad = base("", "token \\r X-Evil: 1");
    let err = parse_connector_manifest(&bad).expect_err("crlf prefix");
    assert!(err.contains("prefix"), "got: {err}");
}

#[test]
fn rejects_prefix_on_query_into() {
    let toml_text = r#"
[connector]
id = "m"
name = "M"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "https://m.example.com"

[connector.credentials]
key = { into = "query:key", prefix = "Bearer " }

[[op]]
id = "fetch"
risk = "low"
class = "network"
http = { method = "GET", path = "/rss" }
"#;
    let err = parse_connector_manifest(toml_text).expect_err("prefix+query");
    assert!(err.contains("prefix 只支持 header"), "got: {err}");
}

#[test]
fn http_base_unknown_field_still_loud() {
    // deny_unknown_fields 纪律不因 headers 扩展而破
    let bad = VALID_REST.replace(
        "[connector.http]\nbase = \"https://quotes.example.com\"",
        "[connector.http]\nbase = \"https://quotes.example.com\"\ntimeout = 30",
    );
    let err = parse_connector_manifest(&bad).expect_err("unknown field in http base");
    assert!(err.contains("TOML 解析失败"), "got: {err}");
}

// —— E3（收官批 2026-10-15）：undo 撤销声明 ——

fn undo_manifest(undo_line: &str, extra_ops: &str) -> String {
    format!(
        r#"
[connector]
id = "paper-broker"
name = "纸面券商"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "https://quotes.example.com"

[[op]]
id = "place_order"
risk = "critical"
class = "write"
receipt = "order_id"
{undo_line}
action = "builtin.paper_order"

{extra_ops}
"#,
        undo_line = undo_line,
        extra_ops = extra_ops
    )
}

const CANCEL_OP_PLAIN: &str = r#"[[op]]
id = "cancel_order"
risk = "medium"
class = "write"
action = "builtin.paper_cancel"
"#;

#[test]
fn undo_declaration_parses_with_forward_reference() {
    // 前向引用合法：cancel_order 声明在 place_order 之后（undo_manifest
    // 模板即此形态）；parsed undo 字段带出。
    let c = parse_connector_manifest(&undo_manifest(
        "undo = { op = \"cancel_order\", arg = \"order_id\" }",
        CANCEL_OP_PLAIN,
    ))
    .expect("valid undo manifest");
    let po = c.ops.iter().find(|o| o.id == "place_order").unwrap();
    let undo = po.undo.as_ref().expect("undo parsed");
    assert_eq!(undo.op, "cancel_order");
    assert_eq!(undo.arg, "order_id");
    assert!(
        c.ops
            .iter()
            .find(|o| o.id == "cancel_order")
            .unwrap()
            .undo
            .is_none()
    );
}

#[test]
fn rejects_undo_op_not_declared() {
    let err = parse_connector_manifest(&undo_manifest(
        "undo = { op = \"nope\", arg = \"order_id\" }",
        CANCEL_OP_PLAIN,
    ))
    .expect_err("undo.op 未声明");
    assert!(err.contains("未在本 connector 声明"), "got: {err}");
}

#[test]
fn rejects_undo_self_reference() {
    let err = parse_connector_manifest(&undo_manifest(
        "undo = { op = \"place_order\", arg = \"order_id\" }",
        CANCEL_OP_PLAIN,
    ))
    .expect_err("undo.op 指向自身");
    assert!(err.contains("不能指向自身"), "got: {err}");
}

#[test]
fn rejects_undo_without_receipt() {
    let err = parse_connector_manifest(
        &undo_manifest(
            "undo = { op = \"cancel_order\", arg = \"order_id\" }",
            CANCEL_OP_PLAIN,
        )
        .replace("receipt = \"order_id\"\n", ""),
    )
    .expect_err("无 receipt 声明 undo");
    assert!(err.contains("必须声明 receipt"), "got: {err}");
}

#[test]
fn rejects_chained_undo() {
    // 目标 op 自身也声明 undo = 撤销链嵌套，禁止。
    let chained =
        format!("{CANCEL_OP_PLAIN}undo = {{ op = \"place_order\", arg = \"order_id\" }}\n");
    let err = parse_connector_manifest(&undo_manifest(
        "undo = { op = \"cancel_order\", arg = \"order_id\" }",
        &chained,
    ))
    .expect_err("撤销链嵌套");
    assert!(err.contains("禁止嵌套"), "got: {err}");
}

#[test]
fn rejects_undo_empty_op_or_arg() {
    let err = parse_connector_manifest(&undo_manifest(
        "undo = { op = \"\", arg = \"order_id\" }",
        CANCEL_OP_PLAIN,
    ))
    .expect_err("undo.op 空串");
    assert!(err.contains("不能为空"), "got: {err}");
}

// ---- M2（2026-10-19）script 声明式绑定 ----

const SCRIPT_VALID: &str = r#"
[connector]
id = "log-rotate"
name = "日志轮转"
version = "0.1.0"
kind = "local"
description = "script 绑定验证"

[[op]]
id = "rotate"
intent = ["轮转日志"]
risk = "medium"
class = "exec"
script = { command = "rotate --dir {dir} --keep {keep}", timeout_secs = 30, workdir = "scripts", capture = 2048 }
args_schema = { type = "object", properties = { dir = { type = "string" }, keep = { type = "integer" } } }

[[op]]
id = "status"
intent = ["查状态"]
risk = "low"
class = "read"
script = { command = "rotate --status" }
"#;

#[test]
fn parses_script_binding_with_placeholders_and_defaults() {
    let c = parse_connector_manifest(SCRIPT_VALID).expect("valid script manifest");
    match &c.ops[0].binding {
        OpBinding::Script {
            command,
            placeholders,
            timeout_secs,
            workdir,
            capture,
        } => {
            assert_eq!(command, "rotate --dir {dir} --keep {keep}");
            assert_eq!(placeholders, &vec!["dir".to_string(), "keep".to_string()]);
            assert_eq!(*timeout_secs, Some(30));
            assert_eq!(workdir.as_deref(), Some("scripts"));
            assert_eq!(*capture, Some(2048));
        }
        other => panic!("expected script binding, got {other:?}"),
    }
    // 缺省臂：timeout/workdir/capture 全 None（运行期走 config 缺省）。
    match &c.ops[1].binding {
        OpBinding::Script {
            timeout_secs,
            workdir,
            capture,
            ..
        } => {
            assert_eq!(*timeout_secs, None);
            assert!(workdir.is_none());
            assert!(capture.is_none());
        }
        other => panic!("expected script binding, got {other:?}"),
    }
}

#[test]
fn rejects_script_undeclared_placeholder_loud() {
    let bad = SCRIPT_VALID.replace(
        "args_schema = { type = \"object\", properties = { dir = { type = \"string\" }, keep = { type = \"integer\" } } }",
        "args_schema = { type = \"object\", properties = { dir = { type = \"string\" } } }",
    );
    let err = parse_connector_manifest(&bad).expect_err("未声明占位符必须 loud");
    assert!(
        err.contains("`{keep}` 未在 args_schema.properties 声明"),
        "got: {err}"
    );
}

#[test]
fn rejects_script_bad_placeholder_syntax() {
    let bad = SCRIPT_VALID.replace("--keep {keep}", "--keep {Keep!}");
    let err = parse_connector_manifest(&bad).expect_err("占位符字符集");
    assert!(err.contains("占位符非法"), "got: {err}");
}

#[test]
fn rejects_script_timeout_zero_and_over_cap() {
    let zero = SCRIPT_VALID.replace("timeout_secs = 30", "timeout_secs = 0");
    let err = parse_connector_manifest(&zero).expect_err("timeout 0");
    assert!(err.contains("timeout_secs 不能为 0"), "got: {err}");

    let over = SCRIPT_VALID.replace("timeout_secs = 30", "timeout_secs = 601");
    let err = parse_connector_manifest(&over).expect_err("timeout 超 600");
    assert!(err.contains("上限 600"), "got: {err}");
}

#[test]
fn rejects_script_workdir_absolute_and_traversal() {
    let abs = SCRIPT_VALID.replace("workdir = \"scripts\"", "workdir = \"C:\\\\tmp\"");
    let err = parse_connector_manifest(&abs).expect_err("绝对路径 workdir");
    assert!(err.contains("必须是相对"), "got: {err}");

    let tilde = SCRIPT_VALID.replace("workdir = \"scripts\"", "workdir = \"~/scripts\"");
    let err = parse_connector_manifest(&tilde).expect_err("~ 开头 workdir");
    assert!(err.contains("必须是相对"), "got: {err}");

    let dotdot = SCRIPT_VALID.replace("workdir = \"scripts\"", "workdir = \"../outside\"");
    let err = parse_connector_manifest(&dotdot).expect_err("`..` 越界 workdir");
    assert!(err.contains(".."), "got: {err}");

    let empty = SCRIPT_VALID.replace("workdir = \"scripts\"", "workdir = \"  \"");
    let err = parse_connector_manifest(&empty).expect_err("空 workdir");
    assert!(err.contains("不能为空"), "got: {err}");
}

#[test]
fn rejects_script_capture_over_cap() {
    let over = SCRIPT_VALID.replace("capture = 2048", "capture = 65537");
    let err = parse_connector_manifest(&over).expect_err("capture 超 65536");
    assert!(err.contains("65536"), "got: {err}");
}

#[test]
fn script_counts_in_binding_uniqueness() {
    // script + tool 同 op 双绑定 = 拒（四选一升级为四选一含 script）。
    let both = SCRIPT_VALID.replace(
        "script = { command = \"rotate --status\" }",
        "script = { command = \"rotate --status\" }\ntool = \"exec\"",
    );
    let err = parse_connector_manifest(&both).expect_err("双绑定");
    assert!(err.contains("只能声明一个"), "got: {err}");

    // 声明 [op.script] 的 op 无其他绑定 = 合法（上面 SCRIPT_VALID 已证）；
    // 这里补「command 全空白」= 拒（缺 command 键是 serde 形态错误，非本校验）。
    let nocommand = SCRIPT_VALID.replace(
        "script = { command = \"rotate --status\" }",
        "script = { command = \"   \" }",
    );
    let err = parse_connector_manifest(&nocommand).expect_err("空白 command");
    assert!(err.contains("command 不能为空"), "got: {err}");
}

// ---------------------------------------------------------------------------
// B2（2026-10-19 独占接管）：exclusive_mcp 清单解析
// ---------------------------------------------------------------------------

#[test]
fn exclusive_mcp_missing_defaults_empty() {
    let c = parse_connector_manifest(VALID_LOCAL).expect("valid local manifest");
    assert!(
        c.exclusive_mcp.is_empty(),
        "缺省不独占（默认态「装了即生效」，拦截只经显式声明出现）"
    );
}

#[test]
fn exclusive_mcp_single_string_form() {
    let text = VALID_LOCAL.replace(
        "kind = \"local\"",
        "kind = \"local\"\nexclusive_mcp = \"playwright-mcp\"",
    );
    let c = parse_connector_manifest(&text).expect("single string form");
    assert_eq!(c.exclusive_mcp, vec!["playwright-mcp".to_string()]);
}

#[test]
fn exclusive_mcp_array_form_normalizes() {
    // 去空白、去重保序。
    let text = VALID_LOCAL.replace(
        "kind = \"local\"",
        "kind = \"local\"\nexclusive_mcp = [\" srv-b \", \"srv-a\", \"srv-b\", \"srv-a\"]",
    );
    let c = parse_connector_manifest(&text).expect("array form");
    assert_eq!(
        c.exclusive_mcp,
        vec!["srv-b".to_string(), "srv-a".to_string()],
        "归一：trim + 去重保序（首现序）"
    );
}

#[test]
fn exclusive_mcp_rejects_blank_entry() {
    let text = VALID_LOCAL.replace(
        "kind = \"local\"",
        "kind = \"local\"\nexclusive_mcp = [\"srv-a\", \"   \"]",
    );
    let err = parse_connector_manifest(&text).expect_err("空白 server 名");
    assert!(err.contains("exclusive_mcp 不能含空项"), "got: {err}");
}
