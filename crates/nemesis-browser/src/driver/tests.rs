//! driver 纯函数单测：JS 片段形态 / 键表 / 截断。

use super::{js_click, js_exists, js_extract, js_focus, js_type, key_spec, truncate_head_tail};

// --- JS 片段形态（selector 经 js_str 引号安全嵌入是注入面）---

#[test]
fn js_focus_shape_active_element_check() {
    // key 动作焦点承接：el.focus() + activeElement 落点验证（不可聚焦
    // 元素 no-op 时返回 ERROR:not-focusable 供调用方诚实报错）。
    let js = js_focus("#kb");
    assert!(js.contains("document.querySelector(\"#kb\")"));
    assert!(js.contains("el.focus()"));
    assert!(js.contains("document.activeElement === el"));
    assert!(js.contains("ERROR:no-element"));
    assert!(js.contains("ERROR:not-focusable"));
    // 引号注入安全：selector 里的双引号被 JSON 编码。
    let evil = js_focus("a\"; alert(1); \"");
    assert!(evil.contains("\\\""));
}

#[test]
fn js_click_shape() {
    let js = js_click("#btn");
    assert!(js.contains("document.querySelector(\"#btn\")"));
    assert!(js.contains("ERROR:no-element"));
    assert!(js.contains("el.click()"));
    // 引号注入安全：selector 里的双引号被 JSON 编码。
    let evil = js_click("a\"; alert(1); \"");
    assert!(!evil.contains("alert(1);;") || evil.contains("\\\"alert"));
    assert!(evil.contains("\\\""));
}

#[test]
fn js_type_shape_native_setter() {
    let js = js_type("#user", "p@ss");
    assert!(js.contains("HTMLInputElement.prototype"));
    assert!(js.contains("HTMLTextAreaElement.prototype"));
    assert!(js.contains("desc.set.call"));
    assert!(js.contains("Event('input'"));
    assert!(js.contains("\"p@ss\""));
}

#[test]
fn js_extract_never_reads_value() {
    // 硬规则：不回读输入框 .value（凭据打字后 extract 是现实路径）。
    let js = js_extract(None);
    assert!(!js.contains(".value"));
    assert!(js.contains("innerText"));
    let js2 = js_extract(Some("#p"));
    assert!(!js2.contains(".value"));
    assert!(js2.contains("querySelector(\"#p\")"));
}

#[test]
fn js_exists_shape() {
    assert!(js_exists("#x").contains("String(!!document.querySelector(\"#x\"))"));
}

// --- key_spec ---

#[test]
fn key_spec_wasd_and_form_keys() {
    // F41 后小写 "w" 走 named 分支；大写 "W" 的 shift 包裹形态由下方
    // 专项测试锁定。
    let w = key_spec("w").unwrap();
    assert_eq!((w.key.as_str(), w.code.as_str(), w.vk), ("w", "KeyW", 87));
    let s = key_spec("s").unwrap();
    assert_eq!((s.key.as_str(), s.code.as_str(), s.vk), ("s", "KeyS", 83));
    let enter = key_spec("enter").unwrap();
    assert_eq!(enter.vk, 13);
    let esc = key_spec("Escape").unwrap();
    assert_eq!(esc.vk, 27);
    let arrow = key_spec("ArrowLeft").unwrap();
    assert_eq!(arrow.vk, 37);
}

#[test]
fn key_spec_ascii_fallback_and_reject() {
    // 单字符 ASCII 兜底：大写 VK 码。
    let q = key_spec("q").unwrap();
    assert_eq!(q.vk, 'Q' as u32);
    // 不认识的键名诚实拒绝（不猜）。
    assert!(key_spec("f13").is_err());
    assert!(key_spec("blah").is_err());
    assert!(key_spec("").is_err());
}

// --- F41 大写字母 shift 包裹（2026-11-16 八审）---

#[test]
fn key_spec_uppercase_wraps_shift_and_preserves_case() {
    use super::KeySpec;
    // 大写字母（含 named 覆盖的 W/A/S/D——必须在 lowercase 匹配之前截获，
    // 否则游戏 sprint 最常发的 "W" 照样静默小写）：shifted=true、key 保留
    // 大写、VK = 大写 ASCII 码。
    let w = key_spec("W").unwrap();
    let KeySpec {
        key,
        code,
        vk,
        shifted,
        ..
    } = w;
    assert!(
        shifted,
        "W 必须标记 shift 包裹（named 小写 w 分支不得吃掉大写）"
    );
    assert_eq!(key, "W");
    assert_eq!(code, "KeyW");
    assert_eq!(vk, 'W' as u32);
    let a = key_spec("A").unwrap();
    assert!(a.shifted);
    assert_eq!((a.key.as_str(), a.code.as_str(), a.vk), ("A", "KeyA", 65));
    // 小写同字母：不包 shift（WASD 常态路径行为不变）。
    let lower_w = key_spec("w").unwrap();
    assert!(!lower_w.shifted);
    assert_eq!(
        (lower_w.key.as_str(), lower_w.code.as_str(), lower_w.vk),
        ("w", "KeyW", 87)
    );
    // 多字符键名含大写首字母（Escape/ArrowLeft）不误判为单字母大写。
    assert!(!key_spec("Escape").unwrap().shifted);
    assert!(!key_spec("ArrowLeft").unwrap().shifted);
    assert!(!key_spec("Shift").unwrap().shifted);
    // 非字母大写字符（数字/符号无 shift 变体语义差异）不包 shift。
    assert!(!key_spec("5").unwrap().shifted);
}

// --- F46 keyDown 载荷 text 语义（2026-11-16 十审）---

#[test]
fn key_down_payload_enter_carries_carriage_return_f46() {
    use super::key_down_payload;
    // Enter 必须带 text "\r"（Puppeteer 同款）——textarea 换行由 char 事件
    // 插入，裸 keydown 只触发 DOM 事件不落字符（缺失 = 聊天框/多行表单
    // Enter 静默不换行）。
    let p = key_down_payload(&key_spec("enter").unwrap());
    assert_eq!(p.get("key").and_then(|v| v.as_str()), Some("Enter"));
    assert_eq!(p.get("text").and_then(|v| v.as_str()), Some("\r"));
    assert_eq!(p.get("unmodifiedText").and_then(|v| v.as_str()), Some("\r"));
}

#[test]
fn key_down_payload_text_semantics_f46() {
    use super::key_down_payload;
    let txt = |name: &str, field: &str| {
        key_down_payload(&key_spec(name).unwrap())
            .get(field)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    // 小写字符：text 自身。
    assert_eq!(txt("a", "text").as_deref(), Some("a"));
    // 数字字符：text 自身。
    assert_eq!(txt("5", "text").as_deref(), Some("5"));
    // 空格：text 空格（vk 32 边界内）。
    assert_eq!(txt("space", "text").as_deref(), Some(" "));
    // 修饰键：无 text 键（F50：字段缺席而非显式 null——CDP optional 非
    // nullable，"text": null = Invalid parameters）。
    assert!(txt("shift", "text").is_none());
    // 功能键：无 text 键（同上）。
    assert!(txt("escape", "text").is_none());
    // F41 shift 包裹的大写：text 大写、unmodifiedText 去 shift 小写
    // （Chromium 无修饰键语义推断的输入）。
    assert_eq!(txt("A", "text").as_deref(), Some("A"));
    assert_eq!(txt("A", "unmodifiedText").as_deref(), Some("a"));
    // 非包裹路径 unmodifiedText 与 text 同形。
    assert_eq!(txt("a", "unmodifiedText").as_deref(), Some("a"));
}

// --- F50 keyDown 载荷 text 判据（2026-11-16 十二审）---

#[test]
fn key_down_payload_function_keys_omit_text_field_f50() {
    use super::key_down_payload;
    // CDP text 是 optional **非 nullable**——显式 "text": null = Invalid
    // parameters（Edge 活体 F49 fail-fast 实证：shift 包裹大写键整族瘫痪，
    // 旧假绿通道把这行失败当「能力闸拒绝」吞掉）。功能键 text/unmodifiedText
    // 字段整个缺席（Puppeteer 无字符键同款形态）；方向键/Delete 旧 bug 落
    // 32..127 分支被给成键名字符串（keypress 携带假字符），一并钉死缺席。
    for name in [
        "shift",
        "ctrl",
        "alt",
        "escape",
        "esc",
        "tab",
        "backspace",
        "arrowleft",
        "arrowup",
        "arrowright",
        "arrowdown",
        "delete",
    ] {
        let p = key_down_payload(&key_spec(name).unwrap());
        let obj = p.as_object().expect("payload object");
        assert!(
            !obj.contains_key("text"),
            "{name} 的 keyDown 帧不得带 text 键（显式 null = Invalid parameters）: {p}"
        );
        assert!(
            !obj.contains_key("unmodifiedText"),
            "{name} 不得带 unmodifiedText 键: {p}"
        );
        // 键身份照常在场（渲染器按 key/code/vk 派发，不依赖 text）。
        assert!(obj.contains_key("windowsVirtualKeyCode"), "{name} 缺 vk");
        assert!(obj.contains_key("code"), "{name} 缺 code");
    }
}

// --- F50 第二段：modifiers 位掩码（2026-11-16 十二审活体实证）---

#[test]
fn key_payload_carries_modifier_bits_f50() {
    use super::{key_down_payload, key_up_payload};
    // 帧不带 modifiers 时 Chromium 不推断修饰状态——活体实测 shift 包裹
    // 五帧 shift 全 false（字符面 keypress:A 已由 text 传达，修饰状态面
    // 丢失）。物理键盘语义：keyup 也携带释放前按下状态，down/up 同公式。
    // 大写 A（shift 包裹）：down/up 帧 modifiers=8。
    assert_eq!(
        key_down_payload(&key_spec("A").unwrap())["modifiers"],
        serde_json::json!(8)
    );
    assert_eq!(
        key_up_payload(&key_spec("A").unwrap())["modifiers"],
        serde_json::json!(8)
    );
    // Shift 键自己：down/up 帧 modifiers=8（释放前在位，keyup:shift=true）。
    assert_eq!(
        key_down_payload(&key_spec("shift").unwrap())["modifiers"],
        serde_json::json!(8)
    );
    assert_eq!(
        key_up_payload(&key_spec("shift").unwrap())["modifiers"],
        serde_json::json!(8)
    );
    // 小写 a / 功能键：modifiers=0（无修饰状态，显式 0 与 Puppeteer 同款恒带）。
    assert_eq!(
        key_down_payload(&key_spec("a").unwrap())["modifiers"],
        serde_json::json!(0)
    );
    assert_eq!(
        key_up_payload(&key_spec("a").unwrap())["modifiers"],
        serde_json::json!(0)
    );
    assert_eq!(
        key_down_payload(&key_spec("enter").unwrap())["modifiers"],
        serde_json::json!(0)
    );
    // Ctrl 独立位 = 2（CDP 位定义：Alt=1 Ctrl=2 Shift=8）。
    assert_eq!(
        key_down_payload(&key_spec("ctrl").unwrap())["modifiers"],
        serde_json::json!(2)
    );
}

// --- F52 复合键名（2026-11-16 十三轮）---

#[test]
fn parse_combo_modifiers_and_upgrade_f52() {
    use super::parse_combo;
    // ctrl+a：修饰序列 1 帧（Control），a 帧 modifiers=2、text="a"。
    let (mods, base) = parse_combo("ctrl+a").unwrap();
    assert_eq!(mods.len(), 1);
    assert_eq!(mods[0].key, "Control");
    assert_eq!(base.key, "a");
    assert_eq!(
        super::key_down_payload(&base)["modifiers"],
        serde_json::json!(2),
        "组合 base 帧必须带组合位（页面 ctrlKey 逐位真）"
    );
    // ctrl+shift+a：小写升级大写（物理字符面一致——shift+a 产出 'A'），
    // modifiers=10（Ctrl|Shift）、text="A"、修饰序列两帧。
    let (mods, base) = parse_combo("ctrl+shift+a").unwrap();
    assert_eq!(mods.len(), 2);
    assert_eq!(base.key, "A");
    assert!(base.shifted);
    assert_eq!(
        super::key_down_payload(&base)["modifiers"],
        serde_json::json!(10)
    );
    assert_eq!(
        super::key_down_payload(&base)["text"],
        serde_json::json!("A")
    );
    // ctrl+A（大写 base + 无显式 shift）：base 保留 shifted，act 臂自动补挂
    // Shift 物理帧（此处只锁 spec 形态：shifted=true + extra=2）。
    let (mods, base) = parse_combo("ctrl+A").unwrap();
    assert_eq!(mods.len(), 1);
    assert!(base.shifted);
    assert_eq!(base.extra_mods, 2);
    // shift+ArrowLeft（游戏修饰键语义）：base 非字母不升级，extra=8。
    let (mods, base) = parse_combo("shift+ArrowLeft").unwrap();
    assert_eq!(base.key, "ArrowLeft");
    assert_eq!(
        super::key_down_payload(&base)["modifiers"],
        serde_json::json!(8)
    );
    assert_eq!(mods.len(), 1);
    // 单键名无 '+'：空修饰序列，走原路径。
    let (mods, base) = parse_combo("a").unwrap();
    assert!(mods.is_empty());
    assert_eq!(base.extra_mods, 0);
    // 非法形态诚实拒绝：前缀非修饰键 / 空段 / 尾段修饰键也拒（无意义组合）。
    assert!(parse_combo("a+ctrl").is_err(), "前缀必须是修饰键");
    assert!(parse_combo("ctrl+").is_err(), "空段拒绝");
    assert!(parse_combo("+a").is_err(), "空段拒绝");
    assert!(parse_combo("ctrl+a+b").is_err(), "多基键拒绝");
}

#[test]
fn truncate_short_text_passthrough() {
    let (s, truncated) = truncate_head_tail("hello", 100, 100);
    assert_eq!(s, "hello");
    assert!(!truncated);
}

#[test]
fn truncate_long_text_head_tail_with_marker() {
    let long: String = "x".repeat(500);
    let (s, truncated) = truncate_head_tail(&long, 10, 5);
    assert!(truncated);
    assert!(s.contains("…[已截断]…"));
    assert!(s.starts_with("xxxxxxxxxx"));
    assert!(s.ends_with("xxxxx"));
    // 总长有界（head + marker + tail）。
    assert!(s.chars().count() < 40);
}

#[test]
fn truncate_char_boundary_safe() {
    // 中文按 char 截断（不炸 UTF-8 边界）。
    let long = "中".repeat(100);
    let (s, truncated) = truncate_head_tail(&long, 3, 2);
    assert!(truncated);
    assert!(s.contains("中中中"));
}

// --- retain_conns 死连接 GC（F31，2026-11-15 五审）---

/// 假 CDP WS 端点（握手即收——CdpConn 只需 connect 成功，测试不发帧；
/// 连接持有防内核过早回收）。
async fn spawn_ws_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                held.push(ws);
            }
        }
    });
    port
}

#[tokio::test]
async fn retain_conns_drops_entries_not_in_live_list() {
    // F31 回归锁：手关 tab 后缓存 conn 永不触发 act 失败摘除（tab 没了就
    // 不会再 act）——retain GC 只留存活清单内的条目，摘后可重连语义不变。
    use super::ChromiumCdpDriver;
    use crate::cdp::CdpConn;

    let port = spawn_ws_server().await;
    let d = ChromiumCdpDriver::new(port, crate::detect::DriverKind::Edge, String::new());
    let c1 = CdpConn::connect(port, "/devtools/page/t1").await.unwrap();
    let c2 = CdpConn::connect(port, "/devtools/page/t2").await.unwrap();
    d.conns.lock().insert("t1".to_string(), c1);
    d.conns.lock().insert("t2".to_string(), c2);

    d.retain_conns(&["t1".to_string()]);
    assert!(d.conns.lock().contains_key("t1"));
    assert!(!d.conns.lock().contains_key("t2"));

    // 幂等：再 GC 一次不炸、存活条目不丢。
    d.retain_conns(&["t1".to_string()]);
    assert!(d.conns.lock().contains_key("t1"));
    // 空清单 = 全清（会话死透路径）。
    d.retain_conns(&[]);
    assert!(d.conns.lock().is_empty());
}

// --- screenshot_params 坐标系归一（F32，2026-11-15 六审）---

#[test]
fn screenshot_params_css_px_clip_or_raw_fallback() {
    use super::screenshot_params;
    // 视口可读：clip scale:1 = 输出 CSS 像素图（与 Input 坐标 1:1——
    // 截图→视觉→坐标点击闭环的前提；DPR≠1 的缩放屏上裸截图恒脱靶）。
    // F38：clip 锚滚动位置（文档坐标）而非 (0,0)——滚动页面上 (0,0)
    // 截的是文档顶部，与用户所见画面脱节。
    let with_v = screenshot_params(Some((1920.0, 1080.0)), (120.0, 340.0));
    assert_eq!(with_v["clip"]["scale"], serde_json::json!(1));
    assert_eq!(with_v["clip"]["width"], serde_json::json!(1920.0));
    assert_eq!(with_v["clip"]["height"], serde_json::json!(1080.0));
    assert_eq!(with_v["clip"]["x"], serde_json::json!(120.0));
    assert_eq!(with_v["clip"]["y"], serde_json::json!(340.0));
    assert_eq!(with_v["captureBeyondViewport"], serde_json::json!(true));
    assert_eq!(with_v["format"], serde_json::json!("png"));
    // 视口不可读 = 裸截图（无 clip），诚实降级不硬编。
    let none = screenshot_params(None, (0.0, 0.0));
    assert!(none.get("clip").is_none());
    // 非法尺寸（0/负）同样回落裸截图。
    let zero = screenshot_params(Some((0.0, 500.0)), (0.0, 0.0));
    assert!(zero.get("clip").is_none());
    let neg = screenshot_params(Some((100.0, -1.0)), (0.0, 0.0));
    assert!(neg.get("clip").is_none());
}
