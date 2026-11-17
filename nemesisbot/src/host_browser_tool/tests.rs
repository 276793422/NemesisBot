//! host_browser 工具纯函数面测试（独立测试文件——生产文件零内联测试纪律）。

use super::*;
use serde_json::json;

/// 1x1 透明 PNG 的最小 base64（Magic + IHDR + IDAT + IEND 完整结构）。
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn tool_for_screenshot(workspace: PathBuf) -> HostBrowserTool {
    let manager = Arc::new(nemesis_browser::BrowserManager::new(
        std::env::temp_dir().join("host_browser_tool_tests_home"),
        Box::new(nemesis_browser::BrowserPrefs::default),
        None,
    ));
    HostBrowserTool::new(
        manager,
        workspace,
        Arc::new(EstopState::new()),
        #[cfg(feature = "security")]
        None,
    )
}

#[test]
fn schema_declares_twelve_actions_and_required_action() {
    // 通道：schema 面是模型唯一可见合同——十二动作枚举缺一即模型盲区
    // （2026-11-18 计划：九动作 + DOM 只读三动作 inspect/snapshot/get_dom）。
    let t = tool_for_screenshot(std::env::temp_dir());
    let schema = t.parameters();
    let actions = schema
        .get("properties")
        .and_then(|p| p.get("action"))
        .and_then(|a| a.get("enum"))
        .and_then(|e| e.as_array())
        .expect("action enum missing");
    let names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        names,
        [
            "navigate",
            "click",
            "type",
            "key",
            "scroll",
            "extract_text",
            "screenshot",
            "wait_for",
            "inspect",
            "snapshot",
            "get_dom",
            "run_script"
        ]
    );
    // DOM 三动作的参数在 schema 面可发现（frame/钳制参数缺一 = 模型盲区）。
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("properties missing");
    for k in ["frame", "max_depth", "max_nodes", "max_chars"] {
        assert!(props.contains_key(k), "schema 缺 {k} 参数");
    }
    let frame_desc = props
        .get("frame")
        .and_then(|v| v.get("description"))
        .and_then(|d| d.as_str())
        .unwrap_or("");
    for needle in ["substring", "cross-process", "same-process"] {
        assert!(
            frame_desc.contains(needle),
            "frame 描述应含「{needle}」（清单发现机制/子串匹配语义）: {frame_desc}"
        );
    }
    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .expect("required missing");
    assert_eq!(required, &vec![json!("action")]);
}

#[test]
fn credential_slot_is_text_only() {
    // 注入层契约：只声明 text 槽——其他槽的 vault: 子串不进注入/闸面。
    let t = tool_for_screenshot(std::env::temp_dir());
    assert_eq!(t.credential_arg_keys(), &["text"]);
}

#[test]
fn min_tier_is_big_trusted_input_is_high_risk() {
    // F20 回归锁：trusted 输入注入是高危面，交付文档承诺「host_browser 恒
    // big 档」（供给秩比较 + dispatch 拒低档双闸按 trait 消费）——override
    // 漏写时本闸静默消失，小模型拿到浏览器接管面。
    let t = tool_for_screenshot(std::env::temp_dir());
    assert_eq!(t.min_tier(), Some("big"));
}

#[test]
fn operation_override_single_actions_project_slugs() {
    let t = tool_for_screenshot(std::env::temp_dir());
    let cases = [
        (
            json!({"action": "navigate", "url": "https://example.com"}),
            "network_request",
        ),
        (
            json!({"action": "click", "selector": "#a"}),
            "network_upload",
        ),
        (
            json!({"action": "type", "selector": "#a", "text": "hi"}),
            "network_upload",
        ),
        (json!({"action": "key", "key": "enter"}), "network_upload"),
        (json!({"action": "scroll", "dy": 300}), "network_upload"),
        (
            json!({"action": "extract_text", "selector": "body"}),
            "internal",
        ),
        (json!({"action": "screenshot"}), "internal"),
        (json!({"action": "wait_for", "selector": "#a"}), "internal"),
        // DOM 只读三动作 = internal（免审批卡读面，2026-11-18 计划 §3.5）。
        (json!({"action": "inspect", "selector": "#a"}), "internal"),
        (json!({"action": "snapshot"}), "internal"),
        (
            json!({"action": "get_dom", "selector": "#a", "frame": "/inner"}),
            "internal",
        ),
    ];
    for (args, expect) in cases {
        let ov = t
            .operation_override(&args.to_string())
            .unwrap_or_else(|| panic!("override missing for {args}"));
        assert_eq!(ov.op_type, expect, "slug mismatch for {args}");
        assert!(
            ov.force_approval.is_none(),
            "single action must not force approval"
        );
    }
}

#[test]
fn operation_override_run_script_forces_approval_with_script_text() {
    // D12③：编排步 = process_exec 档 + 强制人工审批（卡片带完整脚本）。
    let t = tool_for_screenshot(std::env::temp_dir());
    let args = json!({
        "action": "run_script",
        "steps": [
            {"action": "navigate", "url": "https://example.com"},
            {"action": "extract_text", "selector": "body"}
        ]
    });
    let ov = t
        .operation_override(&args.to_string())
        .expect("run_script override missing");
    assert_eq!(ov.op_type, "process_exec");
    let card = ov.force_approval.expect("run_script must force approval");
    assert!(card.contains("2 步"), "审批卡应带步数: {card}");
    assert!(card.contains("run_script"), "审批卡应带完整脚本: {card}");
}

#[test]
fn operation_override_malformed_args_is_none() {
    // 畸形 args 不投影（dispatch 回落静态表/fail-closed 旧行为）。
    let t = tool_for_screenshot(std::env::temp_dir());
    assert!(t.operation_override("not json").is_none());
    assert!(t.operation_override("{}").is_none());
    assert!(
        t.operation_override(r#"{"action":"unknown_action"}"#)
            .is_none()
    );
}

#[test]
fn screenshot_result_is_saved_to_disk_with_path_metadata() {
    // 截图契约：b64 落盘 uploads/browser/，结果换路径不回字节。
    let workspace = std::env::temp_dir().join(format!(
        "host_browser_tool_tests_shot_{}",
        std::process::id()
    ));
    let t = tool_for_screenshot(workspace.clone());
    let result = json!({"ok": true, "png_base64": TINY_PNG_B64});
    let meta = t
        .save_screenshot("b7", &result)
        .expect("screenshot save failed");
    assert_eq!(meta.get("ok"), Some(&json!(true)));
    let path = meta
        .get("path")
        .and_then(|v| v.as_str())
        .expect("path missing");
    let path = PathBuf::from(path);
    assert!(path.is_file(), "截图文件未落盘: {}", path.display());
    assert!(path.starts_with(workspace.join("uploads/browser")));
    let bytes = std::fs::read(&path).expect("read back failed");
    assert_eq!(meta.get("bytes"), Some(&json!(bytes.len())));
    assert_eq!(&bytes[0..4], b"\x89PNG");
    assert_eq!(meta.get("session_id"), Some(&json!("b7")));
    // F12 回归锁：连拍两次文件名必不同（毫秒时间戳会撞名——run_script 连续
    // 两步 screenshot 可在同一毫秒内完成，后者覆盖前者；序号后缀保证唯一）。
    let meta2 = t
        .save_screenshot("b7", &result)
        .expect("second screenshot save failed");
    let path2 = PathBuf::from(
        meta2
            .get("path")
            .and_then(|v| v.as_str())
            .expect("path missing"),
    );
    assert_ne!(
        path,
        path2,
        "连拍两帧文件名相同（撞名覆盖）: {}",
        path.display()
    );
    assert!(path2.is_file(), "第二帧截图未落盘: {}", path2.display());
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn screenshot_save_rejects_missing_or_bad_payload() {
    let t = tool_for_screenshot(std::env::temp_dir());
    let missing = json!({"ok": true});
    assert!(t.save_screenshot("b1", &missing).is_err());
    let bad = json!({"ok": true, "png_base64": "!!!not-base64!!!"});
    assert!(t.save_screenshot("b1", &bad).is_err());
    let empty = json!({"ok": true, "png_base64": ""});
    assert!(t.save_screenshot("b1", &empty).is_err());
}

#[test]
fn script_steps_cap_is_bounded() {
    const {
        assert!(MAX_SCRIPT_STEPS >= 1 && MAX_SCRIPT_STEPS <= 128);
    }
}

// --- F48 estop 停车契约对称（2026-11-16 十一审）---

#[test]
fn script_summary_estop_arm_carries_stop_fields_f48() {
    // estop 停车：stopped_at + stop_reason="estop" 在场（与其余三停车臂
    // security_denied/credential_rejected/step_failed 契约对称——此前 estop
    // 臂漏标，模型只能靠 receipts 里 action:"estop" 猜停车原因）；ok 按
    // receipts 实况（estop 伪回执 ok:false → 汇总 false）。
    let s = super::script_summary(
        3,
        1,
        vec![
            json!({"index": 0, "action": "click", "ok": true}),
            json!({"index": 1, "action": "estop", "ok": false}),
        ],
        Some(1),
    );
    assert_eq!(s.get("stop_reason"), Some(&json!("estop")));
    assert_eq!(s.get("stopped_at"), Some(&json!(1)));
    assert_eq!(s.get("executed"), Some(&json!(1)));
    assert_eq!(s.get("ok"), Some(&json!(false)));
    // 正常完成路径：两字段缺席（无停车语义），ok=true——契约不漂移。
    let s2 = super::script_summary(1, 1, vec![json!({"ok": true})], None);
    assert!(s2.get("stop_reason").is_none());
    assert!(s2.get("stopped_at").is_none());
    assert_eq!(s2.get("ok"), Some(&json!(true)));
}

// --- DOM 只读三动作编排回执摘要投影（2026-11-18 计划 §3.6）---

#[test]
fn dom_action_summary_projects_compact_fields() {
    // inspect 摘 tag/open；snapshot 摘 nodes_emitted/truncated；get_dom 摘
    // chars/truncated——编排回执不携带全量（snapshot 2000 节点 / get_dom
    // 50K 字符会把回执撑爆）。非 DOM 动作 = None（回执回全量）。
    let r = json!({
        "ok": true, "selector": "#d", "frame": "main", "tag": "details",
        "open": true, "checked": false, "visible": true,
        "rect": {"x": 0, "y": 0, "w": 100, "h": 20}, "attributes": {},
        "nodes": [], "nodes_emitted": 42, "html": "<p>…</p>", "chars": 9000
    });
    let s = super::dom_action_summary(
        &WebAction::Inspect {
            selector: "#d".into(),
            frame: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(s, json!({"tag": "details", "open": true}));
    let s = super::dom_action_summary(
        &WebAction::SnapshotDom {
            selector: None,
            frame: None,
            max_depth: None,
            max_nodes: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(s, json!({"nodes_emitted": 42, "truncated": null}));
    let s = super::dom_action_summary(
        &WebAction::GetDom {
            selector: "#d".into(),
            frame: None,
            max_chars: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(s, json!({"chars": 9000, "truncated": null}));
    assert!(
        super::dom_action_summary(&WebAction::Screenshot, &r).is_none(),
        "非 DOM 动作不投影"
    );
}

// --- F42 脚本级前置校验（2026-11-16 八审）---

#[test]
fn script_plan_validation_rejects_malformed_without_execution() {
    // 畸形步（第 2 步未知动作值 → WebAction 解析失败）：校验期拒绝且
    // 错误注明「脚本未执行任何步骤」——此前这类错误在编排中途裸 Err，
    // 已执行步回执全丢，模型只能整脚本重发 = 前序副作用二次执行。
    let bad_action = json!({
        "steps": [
            {"action": "navigate", "url": "https://x"},
            {"action": "frobnicate"}
        ]
    });
    let err = HostBrowserTool::validate_script_plan(&bad_action).unwrap_err();
    assert!(err.contains("参数不合法"), "错误面: {err}");
    assert!(err.contains("脚本未执行任何步骤"), "错误面: {err}");
    // 缺 action 字段同语义。
    let missing = json!({"steps": [{"selector": "#x"}, {"action": "click", "selector": "#x"}]});
    let err = HostBrowserTool::validate_script_plan(&missing).unwrap_err();
    assert!(err.contains("缺 action 字段"), "错误面: {err}");
    assert!(err.contains("脚本未执行任何步骤"), "错误面: {err}");
    // 嵌套 run_script 拒绝。
    let nested = json!({"steps": [
        {"action": "navigate", "url": "https://x"},
        {"action": "run_script", "steps": []}
    ]});
    let err = HostBrowserTool::validate_script_plan(&nested).unwrap_err();
    assert!(err.contains("嵌套"), "错误面: {err}");
    // 空 steps / 缺 steps / 超限同在闸内。
    assert!(HostBrowserTool::validate_script_plan(&json!({"steps": []})).is_err());
    assert!(HostBrowserTool::validate_script_plan(&json!({})).is_err());
    let too_many = json!({"steps": (0..MAX_SCRIPT_STEPS + 1)
        .map(|i| json!({"action": "navigate", "url": format!("https://x/{i}")}))
        .collect::<Vec<_>>()});
    let err = HostBrowserTool::validate_script_plan(&too_many).unwrap_err();
    assert!(err.contains("步数超限"), "错误面: {err}");
}

#[test]
fn script_plan_validation_accepts_all_basic_actions() {
    // 基础动作全形通过（click 双形态 / key / scroll / 读类三件 / DOM 只读
    // 三动作）+ continue_on_error 收编进计划。
    let args = json!({
        "continue_on_error": true,
        "steps": [
            {"action": "navigate", "url": "https://x"},
            {"action": "click", "selector": "#a"},
            {"action": "click", "x": 10, "y": 20},
            {"action": "type", "selector": "#u", "text": "hi"},
            {"action": "key", "key": "enter"},
            {"action": "scroll", "dy": 300},
            {"action": "extract_text"},
            {"action": "screenshot"},
            {"action": "wait_for", "selector": "#done"},
            {"action": "inspect", "selector": "#details"},
            {"action": "snapshot", "max_depth": 5},
            {"action": "get_dom", "selector": "#f", "frame": "/inner", "max_chars": 4000}
        ]
    });
    let plan = HostBrowserTool::validate_script_plan(&args).expect("valid plan rejected");
    assert_eq!(plan.steps.len(), 12);
    assert!(plan.continue_on_error);
    // 回执面动作名与原始 JSON 都在计划里（安全闸看别名原文）。
    assert_eq!(plan.steps[1].action_name, "click");
    assert_eq!(
        plan.steps[1].raw,
        json!({"action": "click", "selector": "#a"})
    );
    // DOM 只读三动作进编排（serde 反序列化天然支持——2026-11-18 计划 §3.6）。
    assert_eq!(plan.steps[9].action_name, "inspect");
    assert_eq!(plan.steps[10].action_name, "snapshot");
    assert_eq!(plan.steps[11].action_name, "get_dom");
}

#[tokio::test]
async fn run_script_live_orchestrates_type_click_extract() {
    // D12 核心需求活体验证：真 spawn Edge（无 Edge 诚实 SKIP——同
    // nemesis-browser edge_live 纪律），run_script 编排 navigate→type→
    // click→extract→inspect→snapshot→get_dom 七步全回执，extract 拿到
    // type+click 的效果文本；DOM 三步回执 = 摘要投影（§3.6：tag/open /
    // nodes_emitted/truncated / chars/truncated，非全量）——钉「编排器在
    // 真浏览器上串起多步 + DOM 只读步骤成编排合法步骤」。
    // fail-fast / estop 边界 / 审批语义归纯函数面测试（run_script_forces_approval
    // 等），此处不重复。
    let Some(_edge) =
        nemesis_browser::detect::find_executable(nemesis_browser::detect::DriverKind::Edge)
    else {
        eprintln!("SKIP：本机无 Edge（run_script 活体编排测试需真浏览器）");
        return;
    };
    let home = std::env::temp_dir().join(format!("host_browser_live_{}", std::process::id()));
    let manager = Arc::new(nemesis_browser::BrowserManager::new(
        home.clone(),
        Box::new(nemesis_browser::BrowserPrefs::default),
        None,
    ));
    let t = tool_for_screenshot(home.clone());
    let session = manager
        .spawn(Some("edge"), None)
        .await
        .expect("spawn Edge failed");

    // data: URL 页面（单引号避转义地狱）：type 进输入框 → click 触发
    // onclick 回显 → extract 读回显；details（open 态）+ 密码预填框 +
    // 足量子树 = DOM 三动作编排断言面。
    let page = "data:text/html,<input id='u' value='hello'>\
         <input id='pw' type='password' value='prefill-secret'>\
         <details id='d' open><summary>s</summary><p>body-text</p></details>\
         <button id='b' \
         onclick=\"document.getElementById('o').textContent='clicked '+document.getElementById('u').value\">go</button><div id='o'></div>";
    let args = json!({
        "steps": [
            {"action": "navigate", "url": page},
            {"action": "type", "selector": "#u", "text": "hello"},
            {"action": "click", "selector": "#b"},
            {"action": "extract_text", "selector": "#o"},
            {"action": "inspect", "selector": "#d"},
            {"action": "snapshot", "selector": "#d", "max_depth": 3},
            {"action": "get_dom", "selector": "#d", "max_chars": 500}
        ]
    });
    // F42：编排签名 = 前置校验产物（ScriptPlan）——生产 execute 链路
    // （validate → pick_alive → run_script）在此镜像。
    let plan = HostBrowserTool::validate_script_plan(&args).expect("plan validation failed");
    let out = t
        .run_script(&session, plan)
        .await
        .expect("run_script failed");
    let summary: serde_json::Value = serde_json::from_str(&out).expect("receipts not json");
    let receipts = summary
        .get("receipts")
        .and_then(|v| v.as_array())
        .expect("receipts array missing")
        .clone();
    assert_eq!(receipts.len(), 7, "七步全回执: {out}");
    for (i, r) in receipts.iter().enumerate() {
        assert_eq!(r.get("ok"), Some(&json!(true)), "第 {i} 步失败: {out}");
    }
    // extract 步 = index 3（七步版 navigate/type/click/extract/inspect/
    // snapshot/get_dom）——旧五步版用 receipts.last() 恰好是 extract，扩容
    // 后 last 是 get_dom（摘要投影无 text 字段）→ 读空串假红（首轮实跑抓出）。
    let text = receipts[3]
        .get("result")
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        text.contains("clicked hello"),
        "extract 未拿到 type+click 效果文本: {out}"
    );
    // inspect 步回执 = 摘要（tag/open），非全量（visible/rect 等不在场）。
    let insp = &receipts[4];
    let insp_res = insp.get("result").expect("inspect result missing");
    assert_eq!(insp_res.get("tag"), Some(&json!("details")), "{out}");
    assert_eq!(insp_res.get("open"), Some(&json!(true)), "{out}");
    assert!(
        insp_res.get("rect").is_none() && insp_res.get("attributes").is_none(),
        "inspect 编排回执应投影摘要（rect/attributes 不在场）: {out}"
    );
    // snapshot 步回执 = 摘要（nodes_emitted/truncated），nodes 数组不在场。
    let snap_res = receipts[5].get("result").expect("snapshot result missing");
    assert!(snap_res.get("nodes_emitted").is_some(), "{out}");
    assert!(snap_res.get("truncated").is_some(), "{out}");
    assert!(
        snap_res.get("nodes").is_none(),
        "snapshot 编排回执不应携带 nodes 数组: {out}"
    );
    // get_dom 步回执 = 摘要（chars/truncated），html 不在场。
    let dom_res = receipts[6].get("result").expect("get_dom result missing");
    assert!(dom_res.get("chars").is_some(), "{out}");
    assert!(dom_res.get("truncated").is_some(), "{out}");
    assert!(
        dom_res.get("html").is_none(),
        "get_dom 编排回执不应携带 html: {out}"
    );

    let _ = manager.close_session(&session.id).await;
    let _ = std::fs::remove_dir_all(&home);
}
