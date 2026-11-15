//! crate 级集成面单测：re-export 可用性 + serde 形态（动作 wire 契约）。

use crate::cdp;
use crate::driver::{KeyPhase, WebAction, action_op_slug};
use crate::{DriverKind, browser_alias_domain, origin_matches_host};

#[test]
fn web_action_serde_tag_and_snake_case() {
    // wire 形态：{"action": "navigate", ...}——LLM 工具参数契约，改名即破坏。
    let v: WebAction = serde_json::from_value(serde_json::json!({
        "action": "navigate", "url": "https://example.com"
    }))
    .expect("navigate 反序列化");
    match v {
        WebAction::Navigate { url, timeout_ms } => {
            assert_eq!(url, "https://example.com");
            assert_eq!(timeout_ms, None);
        }
        _ => panic!("navigate 变体不匹配"),
    }
}

#[test]
fn web_action_serde_full_fields() {
    let v: WebAction = serde_json::from_value(serde_json::json!({
        "action": "key", "key": "w", "phase": "down", "hold_ms": 120
    }))
    .expect("key 反序列化");
    match v {
        WebAction::Key {
            key,
            phase,
            hold_ms,
            selector,
        } => {
            assert_eq!(key, "w");
            assert_eq!(phase, KeyPhase::Down);
            assert_eq!(hold_ms, Some(120));
            // selector 缺省 = None（wire 兼容：旧调用不带 selector 语义不变）。
            assert_eq!(selector, None);
        }
        _ => panic!("key 变体不匹配"),
    }
    // 带 selector = 焦点承接契约（trusted 键事件需页面有焦点元素，先
    // el.focus() 再派发）。
    let v: WebAction = serde_json::from_value(serde_json::json!({
        "action": "key", "key": "enter", "selector": "#q"
    }))
    .expect("key+selector 反序列化");
    match v {
        WebAction::Key { key, selector, .. } => {
            assert_eq!(key, "enter");
            assert_eq!(selector.as_deref(), Some("#q"));
        }
        _ => panic!("key+selector 变体不匹配"),
    }
}

#[test]
fn web_action_serde_scroll_defaults() {
    // x/y 缺省 = 视口中心；dx/dy 缺省 = 0（工具 schema required 只有
    // action——模型合法省略任一方向：省略 dy = 只横滚、省略 dx = 只纵滚；
    // serde default 与 schema 对齐，缺必填标注会被「missing field」拒绝
    // 白烧一轮——2026-11-15 复审 F8）。
    let v: WebAction = serde_json::from_value(serde_json::json!({
        "action": "scroll", "dx": 0, "dy": -300
    }))
    .expect("scroll 反序列化");
    match v {
        WebAction::Scroll { x, y, dx, dy } => {
            assert_eq!(x, None);
            assert_eq!(y, None);
            assert!((dx - 0.0).abs() < f64::EPSILON);
            assert!((dy + 300.0).abs() < f64::EPSILON);
        }
        _ => panic!("scroll 变体不匹配"),
    }
    // 只纵滚（省略 dx——schema 未标 required，缺字段必须合法）。
    let v: WebAction = serde_json::from_value(serde_json::json!({
        "action": "scroll", "dy": -120
    }))
    .expect("scroll 缺 dx 反序列化");
    match v {
        WebAction::Scroll { dx, dy, .. } => {
            assert!((dx - 0.0).abs() < f64::EPSILON);
            assert!((dy + 120.0).abs() < f64::EPSILON);
        }
        _ => panic!("scroll 缺 dx 变体不匹配"),
    }
    // 只横滚（省略 dy）。
    let v: WebAction = serde_json::from_value(serde_json::json!({
        "action": "scroll", "dx": 200
    }))
    .expect("scroll 缺 dy 反序列化");
    match v {
        WebAction::Scroll { dx, dy, .. } => {
            assert!((dx - 200.0).abs() < f64::EPSILON);
            assert!((dy - 0.0).abs() < f64::EPSILON);
        }
        _ => panic!("scroll 缺 dy 变体不匹配"),
    }
    // 双缺省 = 0（合法 no-op 滚动）。
    let v: WebAction = serde_json::from_value(serde_json::json!({ "action": "scroll" }))
        .expect("scroll 双缺省反序列化");
    match v {
        WebAction::Scroll { dx, dy, .. } => {
            assert!((dx - 0.0).abs() < f64::EPSILON);
            assert!((dy - 0.0).abs() < f64::EPSILON);
        }
        _ => panic!("scroll 双缺省变体不匹配"),
    }
}

#[test]
fn action_op_slug_covers_all_variants() {
    // per-op 安全分档投影表（编排层 operation_override 数据源）。
    let cases: Vec<(WebAction, &str)> = vec![
        (
            WebAction::Navigate {
                url: "u".into(),
                timeout_ms: None,
            },
            "network_request",
        ),
        (
            WebAction::Click {
                selector: None,
                x: None,
                y: None,
            },
            "network_upload",
        ),
        (
            WebAction::Type {
                selector: "s".into(),
                text: "t".into(),
            },
            "network_upload",
        ),
        (
            WebAction::Key {
                key: "enter".into(),
                phase: KeyPhase::Press,
                hold_ms: None,
                selector: None,
            },
            "network_upload",
        ),
        (
            WebAction::Scroll {
                x: None,
                y: None,
                dx: 0.0,
                dy: 0.0,
            },
            "network_upload",
        ),
        (WebAction::ExtractText { selector: None }, "internal"),
        (WebAction::Screenshot, "internal"),
        (
            WebAction::WaitFor {
                selector: "s".into(),
                timeout_ms: None,
            },
            "internal",
        ),
    ];
    for (action, expected) in cases {
        assert_eq!(action_op_slug(&action), expected, "{action:?}");
    }
}

#[test]
fn driver_kind_slug_roundtrip() {
    for k in [
        DriverKind::Edge,
        DriverKind::Chrome,
        DriverKind::Brave,
        DriverKind::Vivaldi,
        DriverKind::Chromium,
        DriverKind::Firefox,
        DriverKind::Custom,
    ] {
        assert_eq!(DriverKind::from_slug(k.slug()), Some(k), "{}", k.slug());
    }
    assert_eq!(DriverKind::from_slug("nosuch"), None);
}

#[test]
fn reexported_pure_fns_work() {
    // re-export 与模块内实现同一函数（防 re-export 偏离）。
    assert_eq!(
        browser_alias_domain("vault:browser/gitee.com/login"),
        Some("gitee.com".to_string())
    );
    assert!(origin_matches_host("Gitee.com", "gitee.com"));
    assert!(!origin_matches_host("evil.com", "gitee.com"));
}

#[test]
fn js_str_escapes_quotes_and_newlines() {
    assert_eq!(cdp::js_str("he\"llo"), "\"he\\\"llo\"");
    assert_eq!(cdp::js_str("a\nb"), "\"a\\nb\"");
    assert_eq!(cdp::js_str(""), "\"\"");
    // 中文原样保留（JSON 编码不转义非 ASCII）。
    assert_eq!(cdp::js_str("登录"), "\"登录\"");
}
