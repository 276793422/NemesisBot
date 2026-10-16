use super::*;
use crate::manifest::RiskLevel;

fn tmpws() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn scan_sorted_and_missing_dir_is_empty() {
    let ws = tmpws();
    assert!(scan_connectors(ws.path()).is_empty(), "缺目录 = 空注册表");

    for id in ["c-b", "c-a", "c-c"] {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#
            ),
        )
        .expect("write");
    }
    // 混入散文件与空目录，确认都被处理（散文件跳过、空目录诚实入列）
    std::fs::write(ws.path().join(CONNECTORS_DIR).join("stray.toml"), "x = 1").expect("stray");

    let loaded = scan_connectors(ws.path());
    let ids: Vec<String> = loaded
        .iter()
        .map(|l| {
            l.connector
                .as_ref()
                .map(|c| c.id.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(ids, vec!["c-a", "c-b", "c-c"], "目录名升序，确定性");
}

#[test]
fn scan_corrupt_manifest_honest_entry() {
    let ws = tmpws();
    let dir = ws.path().join(CONNECTORS_DIR).join("broken");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(MANIFEST_FILE), "this is not toml [").expect("write");

    let loaded = scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].connector.is_none());
    let err = loaded[0].error.as_deref().expect("honest error");
    assert!(!err.is_empty());
}

#[test]
fn scan_dir_without_manifest_honest_entry() {
    let ws = tmpws();
    std::fs::create_dir_all(ws.path().join(CONNECTORS_DIR).join("empty-c")).expect("mkdir");
    let loaded = scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].error.as_deref().expect("err").contains("缺少"));
}

#[test]
fn find_connector_three_states() {
    let ws = tmpws();
    // 不存在
    let err = find_connector(ws.path(), "nope");
    assert!(matches!(err, Err(ConnectorLookupError::NotFound { .. })));

    // 损坏（目录存在但缺 manifest）
    std::fs::create_dir_all(ws.path().join(CONNECTORS_DIR).join("half")).expect("mkdir");
    let err = find_connector(ws.path(), "half");
    assert!(matches!(err, Err(ConnectorLookupError::Invalid { .. })));

    // 停用
    let dir = ws.path().join(CONNECTORS_DIR).join("off");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "off"
name = "n"
version = "0.1.0"
kind = "local"
enabled = false

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    )
    .expect("write");
    let err = find_connector(ws.path(), "off");
    assert!(matches!(err, Err(ConnectorLookupError::Disabled { .. })));

    // 正常
    let dir = ws.path().join(CONNECTORS_DIR).join("ok");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "ok"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    )
    .expect("write");
    let c = find_connector(ws.path(), "ok").expect("ok connector");
    assert_eq!(c.id, "ok");
}

#[test]
fn find_op_by_id() {
    let ws = tmpws();
    let dir = ws.path().join(CONNECTORS_DIR).join("c");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "c"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "alpha"
risk = "low"
class = "read"
action = "builtin.a"

[[op]]
id = "beta"
risk = "high"
class = "write"
action = "builtin.b"
"#,
    )
    .expect("write");
    let c = find_connector(ws.path(), "c").expect("c");
    assert_eq!(find_op(&c, "beta").expect("beta op").risk, RiskLevel::High);
    assert!(find_op(&c, "gamma").is_none());
}

#[test]
fn search_intent_hit_and_empty_query_lists_all() {
    let ws = tmpws();
    for (id, intent) in [
        ("monitor", vec!["监控网页", "检查更新"]),
        ("paper-broker", vec!["买入", "卖出"]),
    ] {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let intents = intent
            .iter()
            .map(|i| format!("\"{i}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "{id}"
version = "0.1.0"
kind = "local"

[[op]]
id = "doit"
intent = [{intents}]
risk = "low"
class = "read"
action = "builtin.noop"
"#
            ),
        )
        .expect("write");
    }

    // op 级中文意图命中
    let hits = search_connectors(ws.path(), "监控");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].connector.id, "monitor");
    assert_eq!(hits[0].matched_ops, vec!["doit".to_string()]);

    // connector 级命中（名称包含）→ 全部 ops
    let hits = search_connectors(ws.path(), "broker");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].connector.id, "paper-broker");
    assert_eq!(hits[0].matched_ops, vec!["doit".to_string()]);

    // 空 query = 列全部 enabled
    let hits = search_connectors(ws.path(), "");
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.matched_ops.len() == 1));

    // 无命中
    assert!(search_connectors(ws.path(), "不存在的意图词").is_empty());

    // disabled 不出现在检索面
    let dir = ws.path().join(CONNECTORS_DIR).join("off");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "off"
name = "监控隐藏版"
version = "0.1.0"
kind = "local"
enabled = false

[[op]]
id = "doit"
intent = ["监控"]
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    )
    .expect("write");
    let hits = search_connectors(ws.path(), "监控");
    assert_eq!(hits.len(), 1, "disabled 必须从检索面消失");
    assert_eq!(hits[0].connector.id, "monitor");
}

#[test]
fn lookup_error_display_has_id() {
    let e = ConnectorLookupError::NotFound { id: "x".into() };
    assert!(e.to_string().contains("x"));
}

// --- R4：执行前 args 校验 + 加载期 schema 形态扫描 ----------------------------

#[test]
fn validate_op_args_enforces_schema() {
    let schema = serde_json::json!({
        "type": "object",
        "required": ["state"],
        "properties": {
            "state": {"type": "string", "enum": ["closed"]},
            "n": {"type": "integer", "minimum": 1, "maximum": 10},
            "p": {"type": "number", "minimum": 0.5},
            "flag": {"type": "boolean"}
        }
    });
    // 合法
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "n": 5})
        )
        .is_ok()
    );
    // required 缺失 → 错误含字段名
    let e = validate_op_args(Some(&schema), &serde_json::json!({})).unwrap_err();
    assert!(e.contains("state"), "{e}");
    // 类型错 → 错误含实际类型
    let e = validate_op_args(Some(&schema), &serde_json::json!({"state": 1})).unwrap_err();
    assert!(e.contains("类型"), "{e}");
    assert!(e.contains("number"), "{e}");
    // enum 越界 → 错误含允许范围
    let e = validate_op_args(Some(&schema), &serde_json::json!({"state": "open"})).unwrap_err();
    assert!(e.contains("允许范围"), "{e}");
    // integer 接受整值浮点（5.0），拒绝非整值（5.5）
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "n": 5.0})
        )
        .is_ok()
    );
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "n": 5.5})
        )
        .is_err()
    );
    // minimum / maximum 边界
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "n": 0})
        )
        .unwrap_err()
        .contains("下限")
    );
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "n": 11})
        )
        .unwrap_err()
        .contains("上限")
    );
    // bool 不是 number（serde is_number 不含 bool）
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "p": true})
        )
        .unwrap_err()
        .contains("类型")
    );
    // 多余字段忽略（JSON Schema 缺省语义）
    assert!(
        validate_op_args(
            Some(&schema),
            &serde_json::json!({"state": "closed", "extra": {"x": 1}})
        )
        .is_ok()
    );
    // 非必填字段缺席 = 放行
    assert!(validate_op_args(Some(&schema), &serde_json::json!({"state": "closed"})).is_ok());
    // None schema 不设限
    assert!(validate_op_args(None, &serde_json::json!({"anything": [1, 2, 3]})).is_ok());
}

#[test]
fn manifest_args_schema_shape_scan_rejects_unsupported() {
    let ws = tmpws();
    let write_conn = |id: &str, schema_toml: &str| {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "doit"
risk = "low"
class = "read"
action = "builtin.noop"
args_schema = {schema_toml}
"#
            ),
        )
        .expect("write");
    };

    // 白名单外关键词（pattern）→ 加载即拒（诚实拒绝假装支持）
    write_conn(
        "bad-kw",
        r#"{ type = "object", properties = { a = { type = "string", pattern = "^x" } } }"#,
    );
    let err = find_connector(ws.path(), "bad-kw").unwrap_err().to_string();
    assert!(err.contains("不支持的关键词"), "{err}");
    assert!(err.contains("pattern"), "{err}");

    // 顶层 type 不是 object → 拒
    write_conn("bad-top", r#"{ type = "string" }"#);
    let err = find_connector(ws.path(), "bad-top")
        .unwrap_err()
        .to_string();
    assert!(err.contains("object"), "{err}");

    // property type=array（本方校验器不递归）→ 拒
    write_conn(
        "bad-arr",
        r#"{ type = "object", properties = { a = { type = "array" } } }"#,
    );
    let err = find_connector(ws.path(), "bad-arr")
        .unwrap_err()
        .to_string();
    assert!(err.contains("string|number|integer|boolean"), "{err}");

    // 白名单内关键词全通过（含 description——全仓清单大量使用）
    write_conn(
        "good",
        r#"{ type = "object", required = ["state"], properties = { state = { type = "string", enum = ["closed"], description = "目标状态" }, n = { type = "integer", minimum = 1, maximum = 9 } } }"#,
    );
    let c = find_connector(ws.path(), "good").expect("白名单内关键词应通过加载");
    let op = find_op(&c, "doit").expect("op");
    let schema = op.args_schema.as_ref().expect("schema 透传");
    // 加载期合法 + 运行期校验闭环：非法参数被拒、合法参数放行
    assert!(validate_op_args(Some(schema), &serde_json::json!({"state": "open"})).is_err());
    assert!(
        validate_op_args(
            Some(schema),
            &serde_json::json!({"state": "closed", "n": 3})
        )
        .is_ok()
    );
}

/// 形态坏的 required / properties loud 拒（2026-10-16 回检补口）：registry
/// 消费侧 `as_array()`/`as_str()`/`as_object()` 全是静默跳过——形态坏 =
/// 必填闸或字段级校验整体失效，加载期必须拒，不能装作支持。
#[test]
fn manifest_args_schema_shape_rejects_malformed_required_and_properties() {
    let ws = tmpws();
    let write_conn = |id: &str, schema_toml: &str| {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "doit"
risk = "low"
class = "read"
action = "builtin.noop"
args_schema = {schema_toml}
"#
            ),
        )
        .expect("write");
    };

    // required 不是数组（registry as_array() None → 必填闸整体失效）→ 拒
    write_conn("bad-req-str", r#"{ type = "object", required = "symbol" }"#);
    let err = find_connector(ws.path(), "bad-req-str")
        .unwrap_err()
        .to_string();
    assert!(err.contains("required 必须是数组"), "{err}");

    // required 成员非字符串（registry as_str() None → 该成员静默跳过）→ 拒
    write_conn(
        "bad-req-mixed",
        r#"{ type = "object", required = ["symbol", 3] }"#,
    );
    let err = find_connector(ws.path(), "bad-req-mixed")
        .unwrap_err()
        .to_string();
    assert!(err.contains("required 成员必须全为字符串"), "{err}");

    // properties 在场但非 object（registry as_object() None → 字段级校验
    // 整体静默失效）→ 拒；不能与「缺席 = 合法」混同
    write_conn(
        "bad-props-arr",
        r#"{ type = "object", properties = ["symbol"] }"#,
    );
    let err = find_connector(ws.path(), "bad-props-arr")
        .unwrap_err()
        .to_string();
    assert!(err.contains("properties 必须是 object"), "{err}");

    // 缺席 required 与缺席 properties 均保持合法（不被新闸误伤）
    write_conn("ok-empty", r#"{ type = "object" }"#);
    assert!(find_connector(ws.path(), "ok-empty").is_ok());
}

/// min/max 声明在非数值型 property 上 loud 拒（2026-10-16 四轮复查）：
/// registry 校验器只对 number/integer 消费边界，string/boolean 上声明 =
/// 运行期静默跳过——同 R4「装作支持」防线。
#[test]
fn manifest_args_schema_shape_rejects_minmax_on_non_numeric() {
    let ws = tmpws();
    let write_conn = |id: &str, schema_toml: &str| {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "doit"
risk = "low"
class = "read"
action = "builtin.noop"
args_schema = {schema_toml}
"#
            ),
        )
        .expect("write");
    };

    // string 型 + minimum → 拒
    write_conn(
        "bad-str-min",
        r#"{ type = "object", properties = { sym = { type = "string", minimum = 1 } } }"#,
    );
    let err = find_connector(ws.path(), "bad-str-min")
        .unwrap_err()
        .to_string();
    assert!(err.contains("minimum 只对 number/integer"), "{err}");

    // boolean 型 + maximum → 拒
    write_conn(
        "bad-bool-max",
        r#"{ type = "object", properties = { flag = { type = "boolean", maximum = 9 } } }"#,
    );
    let err = find_connector(ws.path(), "bad-bool-max")
        .unwrap_err()
        .to_string();
    assert!(err.contains("maximum 只对 number/integer"), "{err}");

    // number/integer 型 min/max 仍合法（不被新闸误伤）
    write_conn(
        "ok-num",
        r#"{ type = "object", properties = { n = { type = "integer", minimum = 1 }, x = { type = "number", maximum = 2.5 } } }"#,
    );
    assert!(find_connector(ws.path(), "ok-num").is_ok());
}
