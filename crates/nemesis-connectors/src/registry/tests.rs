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
    let c = find_connector(ws.path(), "ok").expect("ok connector").0;
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
    let c = find_connector(ws.path(), "c").expect("c").0;
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
    let c = find_connector(ws.path(), "good")
        .expect("白名单内关键词应通过加载")
        .0;
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

// ---------------------------------------------------------------------------
// 管理面（Dashboard connectors.create / set_enabled / remove，2026-10-19）
// ---------------------------------------------------------------------------

const MIN_TOML: &str = r#"[connector]
id = "new-c"
name = "新连接器"
version = "0.1.0"
kind = "local"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#;

#[test]
fn create_connector_happy_path_and_visible_in_scan() {
    let ws = tmpws();
    let c = create_connector(ws.path(), "new-c", MIN_TOML).expect("create");
    assert_eq!(c.id, "new-c");
    // 热生效：下一次 scan 即见（无缓存）。
    let loaded = scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].connector.as_ref().expect("valid").id, "new-c");
    assert!(
        loaded[0].connector.as_ref().expect("valid").enabled,
        "清单缺 enabled 键 = 默认启用"
    );
    // 落盘文件存在。
    assert!(
        ws.path()
            .join(CONNECTORS_DIR)
            .join("new-c")
            .join(MANIFEST_FILE)
            .exists()
    );
}

#[test]
fn create_connector_rejects_bad_dir_name_bad_toml_and_duplicates() {
    let ws = tmpws();
    // 目录名非法（大写 / `..` / 绝对路径形态 / 空）
    for bad in ["Upper", "../escape", "a/b", ""] {
        let err = create_connector(ws.path(), bad, MIN_TOML).unwrap_err();
        assert!(
            err.contains("目录名") || err.contains("非法"),
            "{bad}: {err}"
        );
    }
    // 清单非法不写半截（目录都不应出现）
    let err = create_connector(ws.path(), "bad-toml", "this is not toml [").unwrap_err();
    assert!(err.contains("TOML 解析失败"), "{err}");
    assert!(!ws.path().join(CONNECTORS_DIR).join("bad-toml").exists());
    // 目录重复
    create_connector(ws.path(), "dup", MIN_TOML).expect("first");
    let err = create_connector(ws.path(), "dup", MIN_TOML).unwrap_err();
    assert!(err.contains("已存在"), "{err}");
    // id 撞车（不同目录、同 id）
    let err = create_connector(ws.path(), "other-dir", MIN_TOML).unwrap_err();
    assert!(err.contains("已被其他目录占用"), "{err}");
}

#[test]
fn set_connector_enabled_writes_sidecar_manifest_bytes_untouched() {
    let ws = tmpws();
    let toml_text = format!(
        "# 头部注释必须原样保留\n{}",
        MIN_TOML.replace("id = \"new-c\"", "id = \"tog\"\nenabled = true")
    );
    create_connector(ws.path(), "tog", &toml_text).expect("create");
    let manifest_path = ws
        .path()
        .join(CONNECTORS_DIR)
        .join("tog")
        .join(MANIFEST_FILE);
    let before = std::fs::read_to_string(&manifest_path).expect("read");

    // 停用：清单字节不动（A3-a 启停写 sidecar），effective 反映在 scan/find。
    set_connector_enabled(ws.path(), "tog", false).expect("disable");
    let after = std::fs::read_to_string(&manifest_path).expect("read");
    assert_eq!(before, after, "清单字节必须原样——启停恒写 sidecar");
    assert!(
        after.contains("enabled = true"),
        "清单缺省不受影响: {after}"
    );
    assert!(
        !scan_connectors(ws.path())[0]
            .connector
            .as_ref()
            .expect("valid")
            .enabled,
        "sidecar 覆盖清单缺省 = 停用"
    );
    assert!(
        find_connector(ws.path(), "tog").is_err(),
        "停用后 find 应 Disabled"
    );

    // 再启用：sidecar 翻回 true。
    set_connector_enabled(ws.path(), "tog", true).expect("enable");
    let after = std::fs::read_to_string(&manifest_path).expect("read");
    assert_eq!(before, after, "再启用也不碰清单");
    assert!(find_connector(ws.path(), "tog").is_ok());
}

#[test]
fn scan_merge_manifest_disabled_without_sidecar_stays_disabled() {
    // 旧工作区兼容：清单 enabled=false 且无 sidecar → 保持停用（缺省语义不漂）。
    let ws = tmpws();
    let toml_text = MIN_TOML.replace("id = \"new-c\"", "id = \"legacy\"\nenabled = false");
    create_connector(ws.path(), "legacy", &toml_text).expect("create");
    let loaded = scan_connectors(ws.path());
    assert!(
        !loaded[0].connector.as_ref().expect("valid").enabled,
        "无 sidecar 时清单缺省必须生效"
    );
    assert!(matches!(
        find_connector(ws.path(), "legacy"),
        Err(ConnectorLookupError::Disabled { .. })
    ));

    // sidecar 显式开启 → 用户意志覆盖清单缺省。
    set_connector_enabled(ws.path(), "legacy", true).expect("enable");
    let loaded = scan_connectors(ws.path());
    assert!(loaded[0].connector.as_ref().expect("valid").enabled);
    assert!(find_connector(ws.path(), "legacy").is_ok());
}

#[test]
fn set_connector_enabled_honest_failures() {
    let ws = tmpws();
    // 不在册
    let err = set_connector_enabled(ws.path(), "ghost", true).unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");
    // 损坏条目（TOML 语法坏）诚实拒绝——sidecar 按 manifest id 键控，损坏
    // 条目解析不出 id（旧实现「语法可解析即可翻位」收窄为不可启停）。
    let dir = ws.path().join(CONNECTORS_DIR).join("broken");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(MANIFEST_FILE), "this is not toml [").expect("write");
    let err = set_connector_enabled(ws.path(), "broken", false).unwrap_err();
    assert!(err.contains("清单损坏"), "{err}");
}

#[test]
fn set_connector_enabled_works_for_signed_entry() {
    // A3-a 核心收益：签名条目启停恒写 sidecar，清单字节不动 → 验签恒过。
    // （旧实现翻 `[connector] enabled` 位破坏验签，只能诚实拒绝。）
    let ws = tmpws();
    create_connector(ws.path(), "signed-c", MIN_TOML).expect("create");
    let dir = ws.path().join(CONNECTORS_DIR).join("signed-c");
    let pair = crate::signing::generate_key_pair().expect("keygen");
    crate::signing::sign_connector_dir(&dir, &pair.private_key).expect("sign");

    set_connector_enabled(ws.path(), "signed-c", false).expect("签名条目可正常停用");
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE)).expect("read");
    assert!(!text.contains("enabled = false"), "清单必须原样: {text}");
    // 清单未动 → 验签仍过（条目保持 connector=Some，trust 徽标在）。
    let loaded = scan_connectors(ws.path());
    let entry = &loaded[0];
    assert!(
        entry.connector.is_some(),
        "验签失败会变 corrupt——清单被改动过"
    );
    assert!(!entry.connector.as_ref().expect("valid").enabled);

    set_connector_enabled(ws.path(), "signed-c", true).expect("再启用");
    assert!(
        scan_connectors(ws.path())[0]
            .connector
            .as_ref()
            .expect("valid")
            .enabled
    );
}

#[test]
fn remove_connector_deletes_dir_but_receipts_survive() {
    let ws = tmpws();
    create_connector(ws.path(), "doomed", MIN_TOML).expect("create");
    // 模拟历史凭条（在 data/ 下，与 connector 目录无关）
    let receipts_dir = ws.path().join("data").join("receipts");
    std::fs::create_dir_all(&receipts_dir).expect("mkdir");
    std::fs::write(
        receipts_dir.join("receipts-20261019.jsonl"),
        "{\"receipt_id\":\"R1\"}",
    )
    .expect("write");

    remove_connector(ws.path(), "doomed").expect("remove");
    assert!(!ws.path().join(CONNECTORS_DIR).join("doomed").exists());
    assert!(
        receipts_dir.join("receipts-20261019.jsonl").exists(),
        "凭条历史保留"
    );
    assert!(scan_connectors(ws.path()).is_empty());

    // 不在册目录名拒绝（无遍历攻击面：不在扫描结果里的名字一律拒）
    let err = remove_connector(ws.path(), "../escape").unwrap_err();
    assert!(err.contains("不在注册表"), "{err}");
}

// ---------------------------------------------------------------------------
// B2（2026-10-19 独占接管）：独占隐藏表 + 镜像 op 枚举
// ---------------------------------------------------------------------------

/// 独占声明 connector 模板（接管 `stub` server；显式 op 绑定一个不在
/// 模拟工具面里的名字，验证「显式档优先、镜像不双路由」不干扰枚举）。
const EXCL_TOML: &str = r#"[connector]
id = "excl"
name = "独占壳"
version = "0.1.0"
kind = "local"
exclusive_mcp = "stub"

[[op]]
id = "page_read"
risk = "medium"
class = "read"
tool = "mcp_stub__explicit_tool"
"#;

fn write_connector(ws: &tempfile::TempDir, id: &str, toml_text: &str) {
    let dir = ws.path().join(CONNECTORS_DIR).join(id);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(MANIFEST_FILE), toml_text).expect("write");
}

fn mcp_tools() -> Vec<String> {
    vec![
        "mcp_stub__read_page".to_string(),
        "mcp_stub__click".to_string(),
        "mcp_other__list".to_string(),
    ]
}

#[test]
fn exclusive_prefix_uses_sanitize_truth_source() {
    // 前缀形态 `mcp_<sanitize(server)>_`——与 MCP 挂载面
    // （adapter.rs `format!("mcp_{server_name}_{tool_name}")`）同源同构。
    assert_eq!(exclusive_prefix("stub"), "mcp_stub_");
    // 大写/非法字符被 sanitize 归一（与 MCP 挂载名一致）。
    assert_eq!(exclusive_prefix("My-Server"), exclusive_prefix("my-server"));
}

#[test]
fn exclusive_hidden_tools_hides_prefix_matches() {
    let ws = tmpws();
    write_connector(&ws, "excl", EXCL_TOML);

    let hidden = exclusive_hidden_tools(ws.path(), &mcp_tools());
    assert_eq!(
        hidden,
        vec![
            "mcp_stub__read_page".to_string(),
            "mcp_stub__click".to_string()
        ],
        "独占前缀命中全隐（default 与 blocked 同隐），表外不连坐"
    );

    // exempt 豁免：恢复原生直调（供给闸放行）。
    crate::state::set_tool_policy(
        ws.path(),
        "excl",
        "mcp_stub__click",
        crate::state::ToolPolicy::Exempt,
    )
    .expect("exempt");
    let hidden = exclusive_hidden_tools(ws.path(), &mcp_tools());
    assert_eq!(
        hidden,
        vec!["mcp_stub__read_page".to_string()],
        "豁免项不隐"
    );

    // 停用 connector = 原生直通恢复（整表为空）。
    set_connector_enabled(ws.path(), "excl", false).expect("disable");
    assert!(exclusive_hidden_tools(ws.path(), &mcp_tools()).is_empty());
}

#[test]
fn exclusive_hidden_tools_disabled_or_absent_contributes_nothing() {
    let ws = tmpws();
    // 无任何 connector = 空表。
    assert!(exclusive_hidden_tools(ws.path(), &mcp_tools()).is_empty());

    // enabled=false 的独占 connector 不贡献隐藏。
    let disabled = EXCL_TOML.replace(
        "exclusive_mcp = \"stub\"",
        "exclusive_mcp = \"stub\"\nenabled = false",
    );
    write_connector(&ws, "excl", &disabled);
    assert!(exclusive_hidden_tools(ws.path(), &mcp_tools()).is_empty());

    // 无独占声明的启用 connector 也不贡献。
    write_connector(&ws, "plain", MIN_TOML);
    assert!(exclusive_hidden_tools(ws.path(), &mcp_tools()).is_empty());
}

#[test]
fn exclusive_mirror_ops_enumeration_and_exclusions() {
    let ws = tmpws();
    write_connector(&ws, "excl", EXCL_TOML);
    let c = find_connector(ws.path(), "excl").expect("valid").0;

    let mirrors = exclusive_mirror_ops(ws.path(), &c, &mcp_tools());
    let ids: Vec<&str> = mirrors.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        ids,
        vec!["mcp_stub__click", "mcp_stub__read_page"],
        "镜像枚举按工具名字典序，前缀外不连坐"
    );

    // blocked：镜像不提供（能力对 agent 消失）。
    crate::state::set_tool_policy(
        ws.path(),
        "excl",
        "mcp_stub__click",
        crate::state::ToolPolicy::Blocked,
    )
    .expect("blocked");
    let mirrors = exclusive_mirror_ops(ws.path(), &c, &mcp_tools());
    let ids: Vec<&str> = mirrors.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(ids, vec!["mcp_stub__read_page"], "封禁项镜像不提供");

    // exempt：回原生直调，同样不经镜像。
    crate::state::set_tool_policy(
        ws.path(),
        "excl",
        "mcp_stub__read_page",
        crate::state::ToolPolicy::Exempt,
    )
    .expect("exempt");
    let mirrors = exclusive_mirror_ops(ws.path(), &c, &mcp_tools());
    assert!(mirrors.is_empty(), "全部例外化后镜像面为空");
}

#[test]
fn exclusive_mirror_ops_excludes_explicit_op_coverage() {
    // 显式 [[op]] 的 tool 绑定命中独占前缀内的真实工具 = 该工具走显式档，
    // 镜像不提供（避免双路由）。
    let ws = tmpws();
    let toml = EXCL_TOML.replace("mcp_stub__explicit_tool", "mcp_stub__click");
    write_connector(&ws, "excl", &toml);
    let c = find_connector(ws.path(), "excl").expect("valid").0;
    let mirrors = exclusive_mirror_ops(ws.path(), &c, &mcp_tools());
    let ids: Vec<&str> = mirrors.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        ids,
        vec!["mcp_stub__read_page"],
        "显式已覆盖的 click 不进镜像面"
    );
}

#[test]
fn exclusive_mirror_ops_empty_without_declaration() {
    let ws = tmpws();
    write_connector(&ws, "plain", MIN_TOML);
    let c = find_connector(ws.path(), "plain").expect("valid").0;
    assert!(
        exclusive_mirror_ops(ws.path(), &c, &mcp_tools()).is_empty(),
        "无独占声明 = 无镜像面（默认态不拦截）"
    );
}

/// 治理补全 P1（2026-10-19）：纯独占接管清单合法——ops 为空 + exclusive_mcp
/// 声明 = 镜像 op 就是操作面（此前「至少一个 [[op]]」把纯接管形态逼成
/// 写假 op）。同时钉 exclusive_hidden_tools 对该形态照常命中。
#[test]
fn exclusive_only_manifest_is_legal_and_hidden_tools_hit() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    let conn_dir = ws.join("connectors").join("gov-smoke-excl");
    std::fs::create_dir_all(&conn_dir).unwrap();
    std::fs::write(
        conn_dir.join("connector.toml"),
        "[connector]\nid = \"gov-smoke-excl\"\nname = \"gov\"\nversion = \"0.1.0\"\nkind = \"local\"\nenabled = true\nexclusive_mcp = \"stub\"\n",
    )
    .unwrap();
    let scanned = scan_connectors(&ws);
    assert_eq!(scanned.len(), 1, "纯独占清单必须合法入册");
    assert!(
        scanned[0].error.is_none(),
        "纯独占清单不得报错，实际 {:?}",
        scanned[0].error
    );
    let names = vec!["mcp_stub__read".to_string()];
    let hidden = exclusive_hidden_tools(&ws, &names);
    assert_eq!(hidden, vec!["mcp_stub__read".to_string()]);
}

/// 对照：既无 op 又无 exclusive_mcp = 没有任何可调用面，仍拒绝。
#[test]
fn empty_manifest_without_exclusive_still_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    let conn_dir = ws.join("connectors").join("bare");
    std::fs::create_dir_all(&conn_dir).unwrap();
    std::fs::write(
        conn_dir.join("connector.toml"),
        "[connector]
id = \"bare\"
name = \"bare\"
version = \"0.1.0\"
kind = \"local\"
enabled = true
",
    )
    .unwrap();
    let scanned = scan_connectors(&ws);
    assert_eq!(scanned.len(), 1);
    assert!(
        scanned[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("[[op]]")),
        "空连接器仍必须拒绝，实际 {:?}",
        scanned[0].error
    );
}

/// B3：mirror_baseline 四档 + 矛盾裁决纯函数表。
#[test]
fn mirror_baseline_tier_table() {
    use crate::manifest::OpClass;
    // 只读自述 → low/read 免卡。
    let (r, c, confirm) = mirror_baseline(Some(true), None);
    assert_eq!((r, c, confirm), (RiskLevel::Low, OpClass::Read, false));
    // 破坏性自述 → high/write 强制确认。
    let (r, c, confirm) = mirror_baseline(None, Some(true));
    assert_eq!((r, c, confirm), (RiskLevel::High, OpClass::Write, true));
    // 显式非只读（readOnly=false）同档破坏性。
    let (r, c, confirm) = mirror_baseline(Some(false), None);
    assert_eq!((r, c, confirm), (RiskLevel::High, OpClass::Write, true));
    // 未声明 → medium/read（v1 保守基线不变）。
    let (r, c, confirm) = mirror_baseline(None, None);
    assert_eq!((r, c, confirm), (RiskLevel::Medium, OpClass::Read, false));
    let (r, c, confirm) = mirror_baseline(None, Some(false));
    assert_eq!((r, c, confirm), (RiskLevel::Medium, OpClass::Read, false));
    // 矛盾自述（ro=true + destr=true）按 destructive 裁（就高不就低）。
    let (r, c, confirm) = mirror_baseline(Some(true), Some(true));
    assert_eq!((r, c, confirm), (RiskLevel::High, OpClass::Write, true));
}
