use super::*;

/// 最小合法 tool manifest（其余用例在其上改字段制造错误）。
fn tool_toml() -> String {
    let sha = "a".repeat(64);
    format!(
        "api-version = 1\nslug = \"demo\"\nkind = \"tool\"\nname = \"Demo\"\nversion = \"0.1.0\"\nwasm-sha256 = \"{sha}\"\n"
    )
}

/// 根键尾追加段（TOML 根键必须先于任何 table 头——段插中间会把
/// 后续根键吞进 table）。
fn with_section(extra: &str) -> String {
    format!("{}\n{extra}", tool_toml())
}

fn parse_err(raw: &str) -> String {
    match PluginManifest::parse(raw) {
        Ok(_) => panic!("expected parse error, got ok: {raw}"),
        Err(e) => e.to_string(),
    }
}

fn parse_ok(raw: &str) -> PluginManifest {
    match PluginManifest::parse(raw) {
        Ok((m, raw_back)) => {
            assert_eq!(raw_back, raw, "parse 返回原始文本");
            m
        }
        Err(e) => panic!("expected ok, got: {e}"),
    }
}

// -- kind 词表与缺省 ---------------------------------------------------

#[test]
fn kind_as_str_and_requires_payload() {
    assert_eq!(PluginKind::Tool.as_str(), "tool");
    assert_eq!(PluginKind::Observer.as_str(), "observer");
    assert_eq!(PluginKind::Policy.as_str(), "policy");
    assert_eq!(PluginKind::Panel.as_str(), "panel");
    assert_eq!(PluginKind::FlowStep.as_str(), "flow-step");
    assert!(PluginKind::Tool.requires_payload());
    assert!(PluginKind::Observer.requires_payload());
    assert!(PluginKind::Policy.requires_payload());
    assert!(PluginKind::FlowStep.requires_payload());
    assert!(!PluginKind::Panel.requires_payload());
}

#[test]
fn kind_serde_roundtrip_and_invalid() {
    for s in ["tool", "observer", "policy", "panel", "flow-step"] {
        let v: PluginKindSerde = serde_json::from_str(&format!("\"{s}\"")).expect(s);
        assert_eq!(v.as_str(), s);
    }
    let err = serde_json::from_str::<PluginKindSerde>("\"widget\"")
        .err()
        .unwrap_or_else(|| panic!("invalid kind rejected"));
    assert!(err.to_string().contains("invalid kind 'widget'"), "{err}");
}

// -- 基础谓词 ---------------------------------------------------------

#[test]
fn valid_slug_matrix() {
    for ok in ["a", "demo", "ab-cd-ef", "a1-b2", &"x".repeat(64)] {
        assert!(valid_slug(ok), "{ok}");
    }
    for bad in [
        "",
        "-lead",
        "trail-",
        "ALL",
        "has space",
        "dot.name",
        "under_score",
        "中文",
        &"x".repeat(65),
        "all",
    ] {
        assert!(!valid_slug(bad), "{bad}");
    }
}

#[test]
fn valid_sha256_hex_matrix() {
    assert!(valid_sha256_hex(&"0a9F".repeat(16)));
    assert!(!valid_sha256_hex(&"0".repeat(63)));
    assert!(!valid_sha256_hex(&"0".repeat(65)));
    assert!(!valid_sha256_hex(&"g".repeat(64)));
    assert!(!valid_sha256_hex(""));
}

#[test]
fn valid_step_op_matrix() {
    assert!(valid_step_op("echo"));
    assert!(valid_step_op("jira:fetch"));
    assert!(valid_step_op(&"x".repeat(64)));
    assert!(!valid_step_op(""));
    assert!(!valid_step_op(&"x".repeat(65)));
    assert!(!valid_step_op("has space"));
    assert!(!valid_step_op("ctrl\u{7}"));
}

#[test]
fn parse_plugin_step_ref_matrix() {
    assert_eq!(
        parse_plugin_step_ref("plugin:demo:echo"),
        Some(("demo", "echo"))
    );
    assert_eq!(
        parse_plugin_step_ref("plugin:demo:jira:fetch"),
        Some(("demo", "jira:fetch"))
    );
    assert_eq!(parse_plugin_step_ref("builtin"), None);
    assert_eq!(parse_plugin_step_ref("plugin:nosecond"), None);
    assert_eq!(parse_plugin_step_ref("plugin:BadSlug:op"), None);
    assert_eq!(parse_plugin_step_ref("plugin:demo:has space"), None);
    assert_eq!(parse_plugin_step_ref("plugin:demo:"), None);
}

// -- config-schema 条目 ------------------------------------------------

#[test]
fn config_schema_entry_two_forms() {
    let desc = ConfigSchemaEntry::Desc("目标语言".into());
    assert_eq!(desc.desc(), "目标语言");
    assert!(desc.default_value().is_none());
    let full = ConfigSchemaEntry::Full {
        desc: "计数".into(),
        default: "3".into(),
    };
    assert_eq!(full.desc(), "计数");
    assert_eq!(full.default_value(), Some("3"));
    let empty_default = ConfigSchemaEntry::Full {
        desc: "无缺省".into(),
        default: String::new(),
    };
    assert!(empty_default.default_value().is_none());
}

// -- 根级解析缺省 -------------------------------------------------------

#[test]
fn parse_defaults_wasm_file_min_tier_and_description() {
    let m = parse_ok(&tool_toml());
    assert_eq!(m.wasm, "plugin.wasm");
    assert_eq!(m.min_tier, "big");
    assert!(m.description.is_empty());
    assert!(m.permissions.egress.is_empty());
    assert!(!m.permissions.allow_private);
    assert!(m.takeover.is_empty());
    assert!(m.ui.pages.is_empty());
    assert!(m.flow.templates.is_empty());
}

#[test]
fn parse_rejects_unknown_root_field() {
    let raw = format!("bogus-root = 1\n{}", tool_toml());
    let err = parse_err(&raw);
    assert!(err.contains("parse"), "deny_unknown_fields loud 拒: {err}");
}

#[test]
fn parse_rejects_contract_version_mismatch() {
    let raw = tool_toml().replace("api-version = 1", "api-version = 2");
    let err = parse_err(&raw);
    assert!(err.contains("contract version mismatch"), "{err}");
}

#[test]
fn parse_rejects_bad_slug() {
    let raw = tool_toml().replace("slug = \"demo\"", "slug = \"Bad_Slug\"");
    assert!(parse_err(&raw).contains("invalid slug"));
}

#[test]
fn parse_rejects_bad_sha_hex() {
    let raw = tool_toml().replace(&"a".repeat(64), &"g".repeat(64));
    assert!(parse_err(&raw).contains("64 hex"));
}

#[test]
fn parse_rejects_missing_sha_for_payload_kinds() {
    for kind in ["tool", "observer", "policy", "flow-step"] {
        let raw = tool_toml()
            .replace("kind = \"tool\"", &format!("kind = \"{kind}\""))
            .replace(&format!("wasm-sha256 = \"{}\"\n", "a".repeat(64)), "");
        let err = parse_err(&raw);
        assert!(err.contains("requires wasm-sha256"), "{kind}: {err}");
    }
}

#[test]
fn parse_rejects_bad_min_tier() {
    let raw = with_section("min-tier = \"huge\"");
    assert!(parse_err(&raw).contains("invalid min-tier"));
}

#[test]
fn parse_rejects_oversize_name_and_version() {
    let raw = tool_toml().replace(
        "name = \"Demo\"",
        &format!("name = \"{}\"", "名".repeat(129)),
    );
    assert!(parse_err(&raw).contains("name exceeds 128"));
    let raw = tool_toml().replace(
        "version = \"0.1.0\"",
        &format!("version = \"{}\"", "v".repeat(65)),
    );
    assert!(parse_err(&raw).contains("version exceeds 64"));
}

// -- egress 条目形态三分 + token ---------------------------------------

#[test]
fn egress_literal_forms_accepted() {
    let entries = [
        "api.example.com",
        "API.Example.COM",
        "api.example.com:8443",
        "[::1]",
        "[::1]:8080",
        "::1",
        "2001:db8::9",
        "203.0.113.7:80",
    ];
    for e in entries {
        let raw = with_section(&format!("[permissions]\negress = [\"{e}\"]"));
        parse_ok(&raw);
    }
}

#[test]
fn egress_literal_forms_rejected() {
    for e in [
        "",
        ".wildcard",
        "has/slash",
        "star*",
        "[no-close",
        "[v4]::bad",
        "host::bad",
        "[::1]:abc",
    ] {
        let raw = with_section(&format!("[permissions]\negress = [\"{e}\"]"));
        let err = parse_err(&raw);
        assert!(err.contains("invalid egress entry"), "{e}: {err}");
    }
}

#[test]
fn egress_token_whole_entry_only() {
    // 合法：整条目 {{config.key}}，键已在 config-schema 声明。
    let raw = with_section(
        "[permissions]\negress = [\"{{config.base_host}}\"]\n\n[config-schema]\nbase_host = \"出口域名\"",
    );
    parse_ok(&raw);
    // 非整条目 token（嵌套/前后缀混入）= loud 拒。
    let raw = with_section(
        "[permissions]\negress = [\"prefix-{{config.k}}\"]\n\n[config-schema]\nk = \"x\"",
    );
    assert!(parse_err(&raw).contains("whole-entry"));
    // 引用非 config.* token。
    let raw = with_section("[permissions]\negress = [\"{{other.k}}\"]");
    assert!(parse_err(&raw).contains("config.<key>"));
    // 引用未声明键。
    let raw = with_section("[permissions]\negress = [\"{{config.ghost}}\"]");
    assert!(parse_err(&raw).contains("undeclared key"));
    // 引用 x-secret 键 = 凭据不进 allowlist。
    let raw = with_section(
        "[permissions]\negress = [\"{{config.token}}\"]\nx-secret = [\"token\"]\n\n[config-schema]\ntoken = \"x\"",
    );
    assert!(parse_err(&raw).contains("never enter the egress allowlist"));
}

// -- x-secret / config-schema 键纪律 -----------------------------------

#[test]
fn x_secret_name_shape() {
    parse_ok(&with_section("[permissions]\nx-secret = [\"good_key-1\"]"));
    for bad in ["", &"x".repeat(65), "has space", "dot.name"] {
        let raw = with_section(&format!("[permissions]\nx-secret = [\"{bad}\"]"));
        assert!(parse_err(&raw).contains("invalid x-secret name"), "{bad}");
    }
}

#[test]
fn config_schema_key_discipline() {
    // x- 前缀保留命名空间。
    let raw = with_section("[config-schema]\n\"x-token\" = \"冒用凭据面\"");
    assert!(parse_err(&raw).contains("reserved x- prefix"), "{raw}");
    // 与 x-secret 冲突。
    let raw =
        with_section("[permissions]\nx-secret = [\"dup\"]\n\n[config-schema]\ndup = \"同名分裂\"");
    assert!(parse_err(&raw).contains("collides with x-secret"));
    // 空键与超长键。
    let raw = with_section("[config-schema]\n\"\" = \"空键\"");
    assert!(parse_err(&raw).contains("invalid config-schema key"));
    let raw = with_section(&format!(
        "[config-schema]\n\"{}\" = \"超长\"",
        "k".repeat(65)
    ));
    assert!(parse_err(&raw).contains("invalid config-schema key"));
}

// -- policy kind 收紧 + x-llm 声明位 -----------------------------------

#[test]
fn policy_kind_permission_tightening() {
    let base = tool_toml().replace("kind = \"tool\"", "kind = \"policy\"");
    let raw = format!("{base}\n[permissions]\negress = [\"api.example.com\"]");
    assert!(parse_err(&raw).contains("forbids permissions.egress"));
    let raw = format!("{base}\n[permissions]\nallow-private = true");
    assert!(parse_err(&raw).contains("forbids permissions.allow-private"));
    let raw = format!("{base}\n[permissions]\nx-secret = [\"k\"]");
    assert!(parse_err(&raw).contains("forbids permissions.x-secret"));
}

#[test]
fn x_llm_requires_panel_kind() {
    for kind in ["tool", "observer", "policy", "flow-step"] {
        let base = tool_toml().replace("kind = \"tool\"", &format!("kind = \"{kind}\""));
        let raw = format!("{base}\n[permissions]\nx-llm = true");
        assert!(parse_err(&raw).contains("requires kind=panel"), "{kind}");
    }
    // panel 无载荷 + 至少一页面才合法——这里补页面。
    let base = tool_toml().replace("kind = \"tool\"", "kind = \"panel\"");
    let raw = format!(
        "{base}\n[permissions]\nx-llm = true\n\n[[ui.page]]\nroute = \"p\"\ntitle = \"页\""
    );
    parse_ok(&raw);
}

// -- 接管声明 ----------------------------------------------------------

#[test]
fn takeover_discipline() {
    // 非 tool kind 声明接管。
    let raw = tool_toml().replace(
        "kind = \"tool\"",
        "kind = \"observer\"\ntakeover = [\"exec\"]",
    );
    assert!(parse_err(&raw).contains("takeover requires kind=tool"));
    // 空/超长目标。
    let raw = tool_toml().replace(
        "version = \"0.1.0\"",
        "version = \"0.1.0\"\ntakeover = [\"\"]",
    );
    assert!(parse_err(&raw).contains("invalid takeover target"));
    let raw = tool_toml().replace(
        "version = \"0.1.0\"",
        &format!("version = \"0.1.0\"\ntakeover = [\"{}\"]", "t".repeat(65)),
    );
    assert!(parse_err(&raw).contains("invalid takeover target"));
    // plugin 命名空间隔离。
    let raw = tool_toml().replace(
        "version = \"0.1.0\"",
        "version = \"0.1.0\"\ntakeover = [\"plugin.demo.x\"]",
    );
    assert!(parse_err(&raw).contains("plugin namespace is isolated"));
    // deny-list 命中。
    let raw = tool_toml().replace(
        "version = \"0.1.0\"",
        "version = \"0.1.0\"\ntakeover = [\"exec\"]",
    );
    assert!(parse_err(&raw).contains("takeover of 'exec' is denied"));
    // 合法声明。
    let raw = tool_toml().replace(
        "version = \"0.1.0\"",
        "version = \"0.1.0\"\ntakeover = [\"weather_lookup\"]",
    );
    parse_ok(&raw);
    assert!(takeover_denied("process_spawn"));
    assert!(!takeover_denied("weather_lookup"));
}

// -- ui / flow 段位 -----------------------------------------------------

#[test]
fn declarative_no_ui_page_rejected_for_panel() {
    // 删掉 sha 行构造无载荷形态。
    let raw = tool_toml()
        .replace("kind = \"tool\"", "kind = \"panel\"")
        .replace(&format!("wasm-sha256 = \"{}\"\n", "a".repeat(64)), "");
    let err = parse_err(&raw);
    assert!(
        err.contains("must declare at least one [[ui.page]]"),
        "{err}"
    );
}

#[test]
fn ui_flow_sections_require_panel() {
    let raw = with_section("[[ui.page]]\nroute = \"p\"\ntitle = \"页\"");
    assert!(parse_err(&raw).contains("ui section requires kind=panel"));
    let raw = with_section(
        "[[flow.template]]\nid = \"t\"\n\n[[flow.template.steps]]\nid = \"s\"\nkind = \"report\"",
    );
    assert!(parse_err(&raw).contains("flow section requires kind=panel"));
}

/// 合法 panel manifest 基座（声明式 + 一页一模板）。
fn panel_toml() -> String {
    r#"api-version = 1
slug = "panel-demo"
kind = "panel"
name = "PanelDemo"
version = "0.1.0"

[[ui.page]]
route = "overview"
title = "总览"

[[ui.page.views]]
kind = "table"
bind = "flow://tpl/list"

[[ui.page.views.columns]]
key = "id"
label = "编号"

[[flow.template]]
id = "tpl"
origin-type = "issue"

[flow.template.on-transition]
op = "add_comment"

[flow.template.on-transition.args]
body = "{{transition.note}}"

[[flow.template.steps]]
id = "fetch"
kind = "connector_fetch"
"#
    .to_string()
}

#[test]
fn valid_panel_manifest_parses() {
    let m = parse_ok(&panel_toml());
    assert_eq!(m.ui.pages.len(), 1);
    assert_eq!(m.flow.templates.len(), 1);
    let tpl = &m.flow.templates[0];
    assert_eq!(tpl.on_transition.op, "add_comment");
    assert_eq!(tpl.on_transition.args["body"], "{{transition.note}}");
    // config_schema 投影三件套（空表）。
    assert!(m.config_schema_keys().is_empty());
    assert!(m.config_schema_defaults().is_empty());
    assert!(m.config_schema_detail().is_empty());
}

#[test]
fn panel_page_validation() {
    // route 形态同 slug。
    let raw = panel_toml().replace("route = \"overview\"", "route = \"Bad_Route\"");
    assert!(parse_err(&raw).contains("invalid ui.page.route"));
    // title 空。
    let raw = panel_toml().replace("title = \"总览\"", "title = \"  \"");
    assert!(parse_err(&raw).contains("title must be 1-128"));
    // title 超 128（字符计）。
    let raw = panel_toml().replace(
        "title = \"总览\"",
        &format!("title = \"{}\"", "页".repeat(129)),
    );
    assert!(parse_err(&raw).contains("title must be 1-128"));
    // route 跨页唯一。
    let raw = panel_toml() + &panel_toml().split("[[flow.template]]").next().unwrap_or("").replacen(
            "api-version = 1\nslug = \"panel-demo\"\nkind = \"panel\"\nname = \"PanelDemo\"\nversion = \"0.1.0\"\n",
            "",
            1,
        );
    assert!(parse_err(&raw).contains("duplicate ui.page.route"));
}

#[test]
fn view_kind_and_bind_validation() {
    // 词汇表外 kind。
    let raw = panel_toml().replace("kind = \"table\"", "kind = \"carousel\"");
    assert!(parse_err(&raw).contains("invalid ui view kind"));
    // bind 前缀非法。
    let raw = panel_toml().replace("bind = \"flow://tpl/list\"", "bind = \"http://x\"");
    assert!(parse_err(&raw).contains("must start with flow:// or cmd://"));
    // flow bind 段不足。
    let raw = panel_toml().replace("bind = \"flow://tpl/list\"", "bind = \"flow://onlyone\"");
    assert!(parse_err(&raw).contains("invalid flow bind"));
    let raw = panel_toml().replace("bind = \"flow://tpl/list\"", "bind = \"flow://tpl/\"");
    assert!(parse_err(&raw).contains("invalid flow bind"));
    // cmd bind 空/超长/含空白。
    let raw = panel_toml().replace("bind = \"flow://tpl/list\"", "bind = \"cmd://\"");
    assert!(parse_err(&raw).contains("invalid cmd bind"));
    let raw = panel_toml().replace(
        "bind = \"flow://tpl/list\"",
        &format!("bind = \"cmd://{}\"", "c".repeat(65)),
    );
    assert!(parse_err(&raw).contains("invalid cmd bind"));
    let raw = panel_toml().replace("bind = \"flow://tpl/list\"", "bind = \"cmd://has space\"");
    assert!(parse_err(&raw).contains("invalid cmd bind"));
    // actions 视图允许空 bind。
    let raw = panel_toml().replace(
        "kind = \"table\"\nbind = \"flow://tpl/list\"",
        "kind = \"actions\"\nbind = \"\"",
    );
    // actions 需要按钮——补一个再验（否则先报 needs at least one button）。
    let raw = raw.replace(
        "[[flow.template]]",
        "[[ui.page.views.buttons]]\nlabel = \"执行\"\ncommand = \"go\"\n\n[[flow.template]]",
    );
    // 上面替换后 actions 视图缺列段——列段在原 table 视图上，已被换掉；
    // 重新构造：直接在 views 上追加 buttons 即可（columns 是 table 专用）。
    parse_ok(&raw);
}

#[test]
fn table_view_needs_columns_and_actions_needs_buttons() {
    let raw = panel_toml().replace(
        "[[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"编号\"\n",
        "",
    );
    assert!(parse_err(&raw).contains("needs at least one column"));
    // 列键空。
    let raw = panel_toml().replace("key = \"id\"", "key = \"\"");
    assert!(parse_err(&raw).contains("column key/label must be non-empty"));
    // actions 零按钮。
    let actions = panel_toml().replace(
            "kind = \"table\"\nbind = \"flow://tpl/list\"\n\n[[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"编号\"",
            "kind = \"actions\"\nbind = \"\"",
        );
    assert!(parse_err(&actions).contains("kind=actions needs at least one button"));
}

#[test]
fn form_view_validation() {
    let form = panel_toml().replace(
            "kind = \"table\"\nbind = \"flow://tpl/list\"\n\n[[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"编号\"",
            "kind = \"form\"\nbind = \"cmd://edit\"\n\n[[ui.page.views.fields]]\nkey = \"lang\"\nlabel = \"语言\"\nfield-type = \"int\"\nhint = \"提示\"\nplaceholder = \"占位\"",
        );
    let m = parse_ok(&form);
    let f = &m.ui.pages[0].views[0].fields[0];
    assert_eq!(f.field_type, "int");
    assert_eq!(f.hint, "提示");
    // 零字段。
    let raw = form.replace(
            "[[ui.page.views.fields]]\nkey = \"lang\"\nlabel = \"语言\"\nfield-type = \"int\"\nhint = \"提示\"\nplaceholder = \"占位\"",
            "",
        );
    assert!(parse_err(&raw).contains("kind=form needs at least one field"));
    // 非法控件形态。
    let raw = form.replace("field-type = \"int\"", "field-type = \"color\"");
    assert!(parse_err(&raw).contains("invalid field type"));
    // 字段键空。
    let raw = form.replace("key = \"lang\"", "key = \"\"");
    assert!(parse_err(&raw).contains("field key/label must be non-empty"));
}

#[test]
fn timeline_view_has_no_required_sections() {
    let raw = panel_toml().replace(
            "kind = \"table\"\nbind = \"flow://tpl/list\"\n\n[[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"编号\"",
            "kind = \"timeline\"\nbind = \"flow://tpl/events\"",
        );
    parse_ok(&raw);
}

#[test]
fn chart_view_validation() {
    let chart = panel_toml().replace(
            "kind = \"table\"\nbind = \"flow://tpl/list\"\n\n[[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"编号\"",
            "kind = \"chart\"\nbind = \"flow://tpl/stats\"\nchart = \"line\"\nx-key = \"day\"\n\n[[ui.page.views.series]]\nkey = \"count\"\nlabel = \"计数\"",
        );
    parse_ok(&chart);
    // 非法图表类型。
    let raw = chart.replace("chart = \"line\"", "chart = \"hologram\"");
    assert!(parse_err(&raw).contains("invalid chart type"));
    // 缺 x-key。
    let raw = chart.replace("x-key = \"day\"", "x-key = \"\"");
    assert!(parse_err(&raw).contains("needs x-key"));
    // 缺 series。
    let raw = chart.replace(
        "\n[[ui.page.views.series]]\nkey = \"count\"\nlabel = \"计数\"",
        "",
    );
    assert!(parse_err(&raw).contains("needs at least one series"));
    // series 键空。
    let raw = chart.replace("key = \"count\"", "key = \"\"");
    assert!(parse_err(&raw).contains("series key/label must be non-empty"));
    // chart 声明 columns/fields/buttons = 互斥。
    let raw = chart + "\n[[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"编号\"\n";
    assert!(parse_err(&raw).contains("forbids columns/fields/buttons"));
    // 非 chart 视图声明 chart 专用段。
    let raw = panel_toml().replace(
        "bind = \"flow://tpl/list\"",
        "bind = \"flow://tpl/list\"\nchart = \"line\"",
    );
    assert!(parse_err(&raw).contains("only apply to kind=chart"));
    let raw = panel_toml().replace(
        "bind = \"flow://tpl/list\"",
        "bind = \"flow://tpl/list\"\nx-key = \"day\"",
    );
    assert!(parse_err(&raw).contains("only apply to kind=chart"));
    let raw = panel_toml().replace(
        "bind = \"flow://tpl/list\"",
        "bind = \"flow://tpl/list\"\n\n[[ui.page.views.series]]\nkey = \"c\"\nlabel = \"l\"",
    );
    assert!(parse_err(&raw).contains("only apply to kind=chart"));
}

#[test]
fn flow_template_validation() {
    // 模板 id 非法。
    let raw = panel_toml().replace("id = \"tpl\"", "id = \"Bad_ID\"");
    assert!(parse_err(&raw).contains("invalid flow.template.id"));
    // 模板 id 重复。
    let tpl_section = panel_toml()
        .split("[[flow.template]]")
        .nth(1)
        .unwrap_or("")
        .to_string();
    let raw = panel_toml() + &format!("\n[[flow.template]]{tpl_section}");
    assert!(parse_err(&raw).contains("duplicate flow.template.id"));
    // 零步骤（serde default 让它走到 validate 报错而非 TOML missing field）。
    let raw = panel_toml().replace(
        "\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"",
        "",
    );
    assert!(parse_err(&raw).contains("needs at least one step"));
    // on-transition.op 无 origin-type。
    let raw = panel_toml().replace("origin-type = \"issue\"", "");
    assert!(parse_err(&raw).contains("requires origin-type"));
}

#[test]
fn flow_step_validation() {
    // 步骤 id 非法 / 重复。
    let raw = panel_toml().replace("id = \"fetch\"", "id = \"Bad_Step\"");
    assert!(parse_err(&raw).contains("step id invalid"));
    let raw = panel_toml() + "\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"report\"\n";
    assert!(parse_err(&raw).contains("duplicate step id"));
    // kind 词表外且非 plugin 引用。
    let raw = panel_toml().replace("kind = \"connector_fetch\"", "kind = \"teleport\"");
    assert!(parse_err(&raw).contains("invalid kind"));
    // plugin 引用合法。
    let raw = panel_toml().replace(
        "kind = \"connector_fetch\"",
        "kind = \"plugin:stepdemo:echo\"",
    );
    parse_ok(&raw);
    // gate 词表。
    let raw = panel_toml().replace(
        "kind = \"connector_fetch\"",
        "kind = \"connector_fetch\"\ngate = \"manual\"",
    );
    parse_ok(&raw);
    let raw = panel_toml().replace(
        "kind = \"connector_fetch\"",
        "kind = \"connector_fetch\"\ngate = \"auto\"",
    );
    assert!(parse_err(&raw).contains("invalid gate"));
    // on_fail 分支。
    for ok in ["", "park_human", "continue", "retry:1", "retry:10"] {
        let raw = panel_toml().replace(
            "kind = \"connector_fetch\"",
            &format!("kind = \"connector_fetch\"\non-fail = \"{ok}\""),
        );
        parse_ok(&raw);
    }
    for bad in ["retry:0", "retry:11", "retry:abc", "abort"] {
        let raw = panel_toml().replace(
            "kind = \"connector_fetch\"",
            &format!("kind = \"connector_fetch\"\non-fail = \"{bad}\""),
        );
        assert!(parse_err(&raw).contains("invalid on_fail"), "{bad}");
    }
}

#[test]
fn when_validation() {
    // 六算子全过（二字优先）。
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        let raw = panel_toml().replace(
            "kind = \"connector_fetch\"",
            &format!("kind = \"connector_fetch\"\nwhen = \"{{{{steps.fetch.count}}}} {op} 0\""),
        );
        parse_ok(&raw);
    }
    // 缺算子。
    let raw = panel_toml().replace(
        "kind = \"connector_fetch\"",
        "kind = \"connector_fetch\"\nwhen = \"just a sentence\"",
    );
    assert!(parse_err(&raw).contains("invalid when"));
    // 左/右侧空。
    let raw = panel_toml().replace(
        "kind = \"connector_fetch\"",
        "kind = \"connector_fetch\"\nwhen = \"== 0\"",
    );
    assert!(parse_err(&raw).contains("both sides"));
    let raw = panel_toml().replace(
        "kind = \"connector_fetch\"",
        "kind = \"connector_fetch\"\nwhen = \"x == \"",
    );
    assert!(parse_err(&raw).contains("both sides"));
}

// -- load_from_dir -----------------------------------------------------

#[test]
fn load_from_dir_missing_and_oversize_and_ok() {
    let dir = tempfile::tempdir().expect("tmp");
    let err = PluginManifest::load_from_dir(dir.path())
        .err()
        .unwrap_or_else(|| panic!("missing plugin.toml"));
    assert!(err.to_string().contains("read"));

    let big = format!("{}# pad\n", tool_toml());
    let pad = " ".repeat(MANIFEST_MAX_BYTES);
    std::fs::write(dir.path().join("plugin.toml"), format!("{big}{pad}")).expect("write");
    let err = PluginManifest::load_from_dir(dir.path())
        .err()
        .unwrap_or_else(|| panic!("oversize"));
    assert!(err.to_string().contains("exceeds"));

    std::fs::write(dir.path().join("plugin.toml"), tool_toml()).expect("write");
    let (m, _) = PluginManifest::load_from_dir(dir.path()).expect("ok");
    assert_eq!(m.slug, "demo");
}
