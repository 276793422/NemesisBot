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
