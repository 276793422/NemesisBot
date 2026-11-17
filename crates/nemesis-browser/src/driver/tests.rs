//! driver 纯函数单测：JS 片段形态 / 键表 / 截断。

use super::{
    GET_DOM_CHARS_DEFAULT, GET_DOM_CHARS_MAX, SNAP_DEPTH_DEFAULT, SNAP_DEPTH_MAX,
    SNAP_NODES_DEFAULT, SNAP_NODES_MAX, eval_result_value, find_frame_id, flatten_frame_tree,
    frame_url_list, is_stale_frame_error, js_click, js_exists, js_extract, js_focus, js_get_dom,
    js_inspect, js_snapshot, js_type, key_spec, truncate_head_tail,
};

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

// --- DOM 只读三动作 JS 片段（2026-11-18 计划 B1）---

#[test]
fn js_inspect_shape_property_attribute_layering() {
    let js = js_inspect("#details");
    assert!(js.contains("document.querySelector(\"#details\")"));
    assert!(js.contains("ERROR:no-element"));
    // property/attribute 分层（计划 §3.2）：状态字段读 property（动态状态
    // 准——点击后 attribute 不更新）；expanded/role/name/type 读 attribute。
    assert!(js.contains("open: norm(el.open)"));
    assert!(js.contains("checked: norm(el.checked)"));
    assert!(js.contains("disabled: norm(el.disabled)"));
    assert!(js.contains("required: norm(el.required)"));
    assert!(js.contains("expanded: el.getAttribute('aria-expanded')"));
    assert!(js.contains("role: el.getAttribute('role')"));
    // undefined → null 归一（探针实测：非 details 元素 el.open 是 undefined，
    // JSON.stringify 会整键丢掉——norm 保键位稳定）。
    assert!(js.contains("(v === undefined ? null : v)"));
    // 几何 + 可见性。
    assert!(js.contains("getBoundingClientRect()"));
    assert!(js.contains("visibility !== 'hidden'"));
    assert!(js.contains("display !== 'none'"));
    // 白名单 + aria-/data- 前缀；href/src/action 巨型载荷截断。
    assert!(js.contains("startsWith('aria-')"));
    assert!(js.contains("startsWith('data-')"));
    assert!(js.contains("slice(0, 200)"));
    // 凭据硬规则：value 恒不在白名单数组（结构性保证不靠调用方自觉）。
    let wl_start = js.find("const WL = [").unwrap();
    let wl_end = js[wl_start..].find(']').unwrap() + wl_start;
    let wl = &js[wl_start..wl_end];
    assert!(!wl.contains("'value'"));
    // 引号注入安全。
    let evil = js_inspect("a\"; alert(1); \"");
    assert!(evil.contains("\\\""));
}

#[test]
fn js_snapshot_shape_bounded_walk() {
    // 缺省 selector → body（null 形态注入）。
    let js = js_snapshot(None, SNAP_DEPTH_DEFAULT, SNAP_NODES_DEFAULT);
    assert!(js.contains("=== null ? null : document.querySelector"));
    assert!(js.contains("document.body"));
    // 双闸 + 诚实标记 + 竞态可观测。
    assert!(js.contains("truncated = true"));
    assert!(js.contains("nodes_emitted"));
    assert!(js.contains("ts_ms"));
    // text 只给叶子（容器 textContent 是后代拼接会与子行重复）。
    assert!(js.contains("el.children.length === 0"));
    // class 一律 getAttribute（SVGElement.className 是 SVGAnimatedString）。
    assert!(js.contains("el.getAttribute('class')"));
    // shadowRoot 递归穿透 + shadow 标记。
    assert!(js.contains("el.shadowRoot"));
    // id/cls 截断（防构建 hash 类名/超长 id）。
    assert!(js.contains("clip(el.id, 80)"));
    assert!(js.contains("clip(cls, 120)"));
    // 布尔投影 !!（property）。
    assert!(js.contains("!!el.open"));
    assert!(js.contains("!!el.disabled"));
    // 数值参数真实注入（非占位）。
    let js3 = js_snapshot(Some("#x"), 30, 2000);
    assert!(js3.contains("const MD = 30, MN = 2000"));
    // 引号注入安全。
    let evil = js_snapshot(Some("a\"; alert(1); \""), 12, 300);
    assert!(evil.contains("\\\""));
}

#[test]
fn js_get_dom_shape_redaction() {
    let js = js_get_dom("#form");
    assert!(js.contains("document.querySelector(\"#form\")"));
    assert!(js.contains("ERROR:no-element"));
    // 脱敏三件套（计划 §3.3）：剥 input value 属性（页面源码预填）/
    // 清 textarea textContent（当前值在 textContent）/ 删 script+style。
    assert!(js.contains("removeAttribute('value')"));
    assert!(js.contains("querySelectorAll('textarea')"));
    assert!(js.contains("textContent = ''"));
    assert!(js.contains("querySelectorAll('script, style')"));
    assert!(js.contains("cloneNode(true)"));
    assert!(js.contains("outerHTML"));
    // 根元素自脱敏（2026-11-18 edge_live 实测缺口）：querySelectorAll 只
    // 匹配后代——selector 直指密码框本身时根也要剥，缺 matches 三连 = 缺口回归。
    assert!(js.contains("copy.matches('input')"));
    assert!(js.contains("copy.matches('textarea')"));
    assert!(js.contains("copy.matches('script, style')"));
    // 引号注入安全。
    let evil = js_get_dom("a\"; alert(1); \"");
    assert!(evil.contains("\\\""));
}

#[test]
fn dom_action_clamp_constants_ordered() {
    // 缺省值永不越界（缺省 + clamp 组合的语义前提）；上下限有序。
    // 常量断言进 const 块（clippy assertions_on_constants 同款纪律）。
    const {
        assert!(SNAP_DEPTH_DEFAULT <= SNAP_DEPTH_MAX);
        assert!(SNAP_NODES_DEFAULT <= SNAP_NODES_MAX);
        assert!(GET_DOM_CHARS_DEFAULT <= GET_DOM_CHARS_MAX);
    }
}

// --- frame 路由纯函数（§3.7 双路径）---

#[test]
fn is_stale_frame_error_four_needles() {
    // 四针命中（页面导航后隔离 world 换 context / OOPIF 换 target / 会话关闭）。
    assert!(is_stale_frame_error(
        "Runtime.evaluate: Cannot find context with given id"
    ));
    assert!(is_stale_frame_error("No frame with given id found"));
    assert!(is_stale_frame_error("Target closed."));
    assert!(is_stale_frame_error(
        "Session closed: attached session not found"
    ));
    // 非 stale 错误不触发自愈重试（普通失败重试是浪费 + 可能双执行）。
    assert!(!is_stale_frame_error("ERROR:no-element"));
    assert!(!is_stale_frame_error("cdp evaluate timeout"));
    assert!(!is_stale_frame_error("element not visible"));
}

#[test]
fn eval_result_value_exception_vs_ok() {
    // 正常路径取 result RemoteObject（{type, value}——调用方再取 .value）。
    let ok = serde_json::json!({"result": {"type": "string", "value": "OK"}});
    assert_eq!(
        eval_result_value(ok, "expr").unwrap().pointer("/value"),
        Some(&serde_json::json!("OK"))
    );
    // exceptionDetails → Err（description 优先，details fallback）。
    let exc = serde_json::json!({
        "exceptionDetails": {"exception": {"description": "TypeError: boom"}}
    });
    assert!(
        eval_result_value(exc, "expr")
            .unwrap_err()
            .contains("TypeError: boom")
    );
    let exc2 = serde_json::json!({
        "exceptionDetails": {"exception": {"details": [{"text": "SyntaxError"}]}}
    });
    assert!(
        eval_result_value(exc2, "expr")
            .unwrap_err()
            .contains("SyntaxError")
    );
    // result 缺席 → Null（不炸）。
    let bare = serde_json::json!({});
    assert_eq!(
        eval_result_value(bare, "expr").unwrap(),
        serde_json::Value::Null
    );
}

#[test]
fn find_frame_id_nested_tree_first_match() {
    let tree = serde_json::json!({
        "frame": {"id": "root", "url": "http://a/main.html"},
        "childFrames": [
            {"frame": {"id": "f1", "url": "http://a/inner.html"}},
            {"frame": {"id": "f2", "url": "http://a/inner.html#second"},
             "childFrames": [{"frame": {"id": "f3", "url": "http://a/deep.html"}}]},
        ]
    });
    // 嵌套 childFrames 递归命中。
    assert_eq!(find_frame_id(&tree, "deep.html").as_deref(), Some("f3"));
    // 多匹配首个命中（f1 在 f2 之前——与 querySelector 首个语义一致）。
    assert_eq!(find_frame_id(&tree, "inner.html").as_deref(), Some("f1"));
    // 无匹配 None。
    assert_eq!(find_frame_id(&tree, "missing.html"), None);
    // 空树。
    let empty = serde_json::json!({});
    let mut none = Vec::new();
    flatten_frame_tree(&empty, &mut none);
    assert!(none.is_empty());
    assert_eq!(find_frame_id(&empty, "x"), None);
}

#[test]
fn get_frame_tree_envelope_must_be_unwrapped_before_find() {
    // 回归锁（2026-11-18 edge_live 实测）：`Page.getFrameTree` 的 result
    // 载荷顶层是 `frameTree` 键（包络），纯函数消费的是 frameTree **节点**
    // `{frame, childFrames}`。把包络直接喂 find_frame_id 会让 flatten 首层
    // 即 return——查找恒未命中、清单恒「（无）」（同源 iframe 路径整体
    // 瘫痪）。lib 只能钉形态契约，包络解包在 resolve_frame；形态漂移时
    // 此测试先红。
    let envelope = serde_json::json!({
        "frameTree": {
            "frame": {"id": "root", "url": "http://a/main.html"},
            "childFrames": [{"frame": {"id": "f1", "url": "http://a/inner.html"}}]
        }
    });
    // 包络直接喂 = 未命中（陷阱形态钉死）。
    assert_eq!(find_frame_id(&envelope, "inner.html"), None);
    // 解包后命中（resolve_frame 的正确姿势）。
    let node = envelope.get("frameTree").cloned().unwrap_or_default();
    assert_eq!(find_frame_id(&node, "inner.html").as_deref(), Some("f1"));
}

#[test]
fn frame_url_list_annotations_dedup_and_empty() {
    let tree = serde_json::json!({
        "frame": {"id": "root", "url": "http://a/main.html"},
        "childFrames": [
            {"frame": {"id": "f1", "url": "http://a/inner.html"}},
            {"frame": {"id": "f2", "url": "http://a:9/inner.html"}},
        ]
    });
    let targets = serde_json::json!({
        "targetInfos": [
            {"type": "page", "url": "http://a/main.html"},
            {"type": "iframe", "url": "http://a:9/inner.html"},
            {"type": "iframe", "url": "http://b/cross.html"},
            {"type": "iframe", "url": "http://a:9/inner.html"},
        ]
    });
    let list = frame_url_list(&tree, &targets);
    // OOPIF targets 先列 + cross-process 标注；同 URL 第三次出现被去重。
    assert!(list.contains("http://b/cross.html [cross-process]"));
    assert!(list.contains("http://a:9/inner.html [cross-process]"));
    assert_eq!(list.matches("http://a:9/inner.html").count(), 1);
    // 只在 frame 树出现的 URL = same-process 标注。
    assert!(list.contains("http://a/inner.html [same-process]"));
    // page 型 target 不进清单（只列 iframe target）——主文档 URL 只能从
    // frame 树以 same-process 出现。
    assert!(!list.contains("http://a/main.html [cross-process]"));
    // 空清单 = 「（无）」（空 URL frame 跳过、无 target）。
    let empty = frame_url_list(
        &serde_json::json!({"frame": {"id": "r", "url": ""}}),
        &serde_json::json!({"targetInfos": []}),
    );
    assert_eq!(empty, "（无）");
}
