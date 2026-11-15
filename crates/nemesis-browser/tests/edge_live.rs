//! 真 Edge 全矩阵 e2e（2026-11-15 浏览器接管，用户验收面）。
//!
//! 本地 axum 站点（表单/点击/滚动/文本/坐标/延迟/键盘页）+ 真实 Edge
//! spawn（专用 profile + 动态调试端口），九动作全矩阵 + 点名 attach +
//! 多 tab + 截图落字节。**环境不满足诚实 SKIP**（本机无 Edge / spawn 失败
//! ——SAC 拦截等）+ 注明原因，不假红。
//!
//! 运行：`cargo test -p nemesis-browser --test edge_live -- --nocapture`
//! （会打开一个真实 Edge 窗口——人机共享驾驶的活体验收；跑完优雅关闭）。

use std::time::Duration;

use nemesis_browser::driver::{KeyPhase, WebAction};
use nemesis_browser::{BrowserDriver, BrowserManager, BrowserPrefs};
use serde_json::json;

// ---------------------------------------------------------------------------
// 本地站点（axum；每个测试页一个行为面）
// ---------------------------------------------------------------------------

async fn serve_site() -> (String, tokio::task::JoinHandle<()>) {
    use axum::Router;
    use axum::routing::get;

    async fn index() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>测试首页</title></head><body>
<h1 id="h">浏览器接管 e2e 站点</h1>
<a id="to-text" href="/text">文本页</a>
<a id="to-form" href="/form">表单页</a>
</body></html>"##,
        )
    }

    async fn text_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>文本页</title></head><body>
<p id="p1">这是第一段验收文本 NEMESIS_TEXT_OK</p>
<p id="p2">第二段：查找功能验证</p>
</body></html>"##,
        )
    }

    async fn form_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>表单页</title></head><body>
<form id="f" onsubmit="return doSubmit(event)">
  <input id="username" name="username" placeholder="用户名">
  <input id="password" name="password" type="password" placeholder="密码">
  <button id="submit" type="submit">登录</button>
</form>
<div id="greeting"></div>
<script>
function doSubmit(e) {
  e.preventDefault();
  var u = document.getElementById('username').value;
  // 密码明文不回显（守卫纪律同源：页面只回显用户名）。
  document.getElementById('greeting').textContent = '欢迎, ' + u;
  return false;
}
</script>
</body></html>"##,
        )
    }

    async fn click_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>点击页</title></head><body>
<button id="btn" onclick="hit()">点我</button>
<div id="count">0</div>
<script>
var n = 0;
function hit() { n++; document.getElementById('count').textContent = String(n); }
</script>
</body></html>"##,
        )
    }

    async fn coords_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>坐标页</title></head><body style="margin:0">
<div id="board" style="width:400px;height:300px;background:#eee"></div>
<div id="coordlog"></div>
<script>
document.getElementById('board').addEventListener('click', function(e) {
  var r = this.getBoundingClientRect();
  document.getElementById('coordlog').textContent =
    'x=' + Math.round(e.clientX - r.left) + ',y=' + Math.round(e.clientY - r.top);
});
</script>
</body></html>"##,
        )
    }

    async fn scroll_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>滚动页</title></head><body>
<div id="scroll-indicator">0</div>
<div style="height:3000px"></div>
<script>
window.addEventListener('scroll', function() {
  document.getElementById('scroll-indicator').textContent =
    String(Math.round(window.scrollY));
});
</script>
</body></html>"##,
        )
    }

    async fn delay_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>延迟页</title></head><body>
<div id="late-btn">等待出现</div>
<script>
setTimeout(function() {
  document.getElementById('late-btn').textContent = 'NEMESIS_LATE_OK';
}, 600);
</script>
</body></html>"##,
        )
    }

    async fn dialog_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>对话框页</title></head><body style="margin:0">
<button id="dlg" style="position:absolute;left:20px;top:20px"
  onclick="setTimeout(function(){ alert('NEMESIS_DIALOG'); }, 50)">延迟弹框</button>
<div id="after" style="position:absolute;left:20px;top:80px">BEFORE</div>
</body></html>"##,
        )
    }

    async fn tall_page() -> axum::response::Html<&'static str> {
        // F38（2026-11-16 七审）截图滚动词域测试页：纯静态两色块（2000px），
        // **刻意无**滚动指示器/固定定位元素/任何动态内容——滚动指示器会在
        // 滚动后改变顶部区文本，把「滚动前后截图必须不同」的断言架空
        // （旧缺陷下两截图都取文档顶部，但指示器文本已变 = 字节差假阳）。
        axum::response::Html(
            r##"<html><head><title>长页</title></head><body style="margin:0">
<div style="height:1000px;background:#3366cc"></div>
<div style="height:1000px;background:#cc6633"></div>
</body></html>"##,
        )
    }

    async fn key_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>键盘页</title></head><body>
<!-- 固定坐标放置：trusted 坐标点击 (110,110) 应聚焦本输入框
     （真实 mousedown 聚焦——合成 dispatchEvent 不聚焦）。 -->
<input id="kb" style="position:absolute;left:60px;top:80px;width:200px;"
  placeholder="点击聚焦后按键">
<div id="keylog" style="position:absolute;left:60px;top:130px;">ARMED</div>
<!-- F46（2026-11-16 十审）：Enter 换行验证面——textarea input 事件只在
     char 事件（text 字段）落字符时触发；裸 keydown（无 text）只记 keylog
     不记 ta-input。 -->
<textarea id="ta" style="position:absolute;left:300px;top:80px;width:200px;height:60px;"></textarea>
<script>
// document 级监听（捕获阶段；落点元素由焦点决定）。
var el = document.getElementById('keylog');
['keydown','keypress','keyup'].forEach(function(t) {
  document.addEventListener(t, function(e) {
    el.textContent += ' [' + t + ':' + e.key + ':trusted=' + e.isTrusted + ':shift=' + e.shiftKey + ':ctrl=' + e.ctrlKey + ']';
  }, true);
});
var ta = document.getElementById('ta');
ta.addEventListener('input', function() {
  el.textContent += ' [ta-input:len=' + ta.value.length + ']';
});
</script>
</body></html>"##,
        )
    }

    async fn coord_page() -> axum::response::Html<&'static str> {
        // F54 坐标闭环验证页（2026-11-16 十四轮）：用户锚定场景（截图→量
        // 坐标→点击→命中）端到端实证面——已知绝对位置按钮 + rect 自报。
        // rectinfo 随 scroll 实时刷新 DOC（文档坐标）/VIEW（视口坐标）双段
        // ——测试从 VIEW 段取「截图上应看到的像素位置」，闭环不依赖滚动手
        // 动换算；3000px 垫高保证任何窗高下 dy=500 都能落满。
        axum::response::Html(
            r##"<html><head><title>坐标页</title></head><body style="margin:0">
<button id="target" style="position:absolute;left:300px;top:900px;width:120px;height:40px"
  onclick="hit()">TARGET</button>
<div id="hits" style="position:absolute;left:300px;top:1000px;">0</div>
<div id="rectinfo" style="position:absolute;left:300px;top:1040px;">PENDING</div>
<div style="position:absolute;left:0;top:3000px;width:1px;height:1px"></div>
<script>
function upd() {
  var b = document.getElementById('target').getBoundingClientRect();
  document.getElementById('rectinfo').textContent =
    'DOC:' + Math.round(b.left + window.scrollX) + ',' + Math.round(b.top + window.scrollY) +
    ' VIEW:' + Math.round(b.left) + ',' + Math.round(b.top);
}
var n = 0;
function hit() { n++; document.getElementById('hits').textContent = String(n); }
window.addEventListener('load', upd);
window.addEventListener('scroll', upd, true);
</script>
</body></html>"##,
        )
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/text", get(text_page))
        .route("/form", get(form_page))
        .route("/click", get(click_page))
        .route("/coords", get(coords_page))
        .route("/scroll", get(scroll_page))
        .route("/delay", get(delay_page))
        .route("/dialog", get(dialog_page))
        .route("/tall", get(tall_page))
        .route("/key", get(key_page))
        .route("/coord", get(coord_page));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), handle)
}

// ---------------------------------------------------------------------------
// Edge 发现 + spawn（缺席诚实 SKIP）
// ---------------------------------------------------------------------------

/// 找本机 Edge 可执行文件；None = 无 Edge（诚实 SKIP）。
fn find_edge() -> Option<std::path::PathBuf> {
    nemesis_browser::detect_installed()
        .into_iter()
        .find(|b| b.kind.slug() == "edge" && !b.phase2)
        .and_then(|b| b.path)
}

async fn spawn_edge_session(
    manager: &BrowserManager,
) -> Option<std::sync::Arc<nemesis_browser::BrowserSession>> {
    // 先验 Edge 在场（诚实 SKIP 判据）；spawn 只传 kind——executable_override
    // 会把会话归类成 Custom（resolve_browser ①号分支优先级），profile 目录
    // 与 kind 名都会偏。生产路径 = 设置页 kind=edge → resolve_kind 探测。
    let edge = match find_edge() {
        Some(p) => p,
        None => {
            eprintln!("SKIP：本机未检测到 Edge（detect_installed 空）——真浏览器矩阵无法跑");
            return None;
        }
    };
    let _ = edge;
    match manager.spawn(Some("edge"), None).await {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("SKIP：Edge spawn 失败（{e}）——SAC 拦截/端口受限等环境原因");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// 全矩阵
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edge_live_full_matrix() {
    fn step(name: &str) {
        eprintln!("[edge_live] {name}");
    }
    step("本地站点启动");
    let (base, _server) = serve_site().await;
    let tmp = tempfile::tempdir().unwrap();
    let manager = BrowserManager::new(
        tmp.path().to_path_buf(),
        Box::new(BrowserPrefs::default),
        None,
    );

    step("spawn Edge");
    let session = match spawn_edge_session(&manager).await {
        Some(s) => s,
        None => return, // 诚实 SKIP（原因已在 spawn_edge_session 打印）
    };

    // F51b（2026-11-16 十二审收尾，两次 panic 跑各漏一棵 Edge 树实证）：
    // panic/断言失败路径兜底树杀——close 步骤之前 panic = 没人收 Edge。
    // panic hook 在 unwinding 前同步树杀，不依赖 Drop 语义与 async。
    // 正常路径 close 已优雅关树，hook 不触发。
    // F53（2026-11-16 十三轮）：树杀命令按平台分支——taskkill 是 Windows
    // only，Linux 上静默失败仍漏树（kill -9 主进程，Chromium 子进程随主
    // 进程退出机制兜底）。
    let spawned_pid = session.pid;
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &spawned_pid.to_string()])
            .output();
        #[cfg(not(windows))]
        let _ = std::process::Command::new("kill")
            .args(["-9", &spawned_pid.to_string()])
            .output();
        prev_hook(info);
    }));

    // --- tabs 枚举 + 点名（attach） ---
    step("tabs_snapshot + set_working_tab");
    let tabs = session.tabs_snapshot().await.expect("tabs_snapshot");
    assert!(!tabs.is_empty(), "spawn 后至少一个 tab");
    let first = tabs[0]["target_id"].as_str().unwrap().to_string();
    let tab = session
        .set_working_tab(&first)
        .await
        .expect("set_working_tab");
    assert_eq!(tab.id, first);
    assert_eq!(session.working_tab_id().as_deref(), Some(first.as_str()));
    // F11 回归锁：attach（点名）必须真建页面级连接——点名后 working 行的
    // load_state 应为真实 readyState（about:blank 页 = "complete"），不是
    // "unknown"（unknown = 连接没建上，徽标永远失明）。
    let tabs = session.tabs_snapshot().await.expect("attach 后快照");
    let working_row = tabs
        .iter()
        .find(|t| t["working"] == json!(true))
        .expect("attach 后应有 working 行");
    assert_eq!(
        working_row["load_state"],
        json!("complete"),
        "点名后 load_state 应为真实 readyState（F11：attach 真建连接）"
    );

    // --- navigate + extract_text（查找） ---
    step("navigate /text + extract");
    let r = session
        .act(&WebAction::Navigate {
            url: format!("{base}/text"),
            timeout_ms: Some(15_000),
        })
        .await
        .expect("navigate /text");
    assert_eq!(r["ok"], json!(true));
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#p1".into()),
        })
        .await
        .expect("extract #p1");
    let text = r["text"].as_str().unwrap();
    assert!(text.contains("NEMESIS_TEXT_OK"), "extract 实文不符: {text}");

    // --- 多 tab：开新页 + 快照 + 切换工作指针 ---
    step("open_tab 多 tab");
    let t2 = session
        .driver()
        .open_tab(Some(&format!("{base}/click")))
        .await
        .expect("open_tab");
    let tabs = session.tabs_snapshot().await.unwrap();
    assert!(tabs.len() >= 2, "open_tab 后 >= 2 个 tab");
    session.set_working_tab(&t2.id).await.unwrap();

    // --- click selector（点击页计数） ---
    step("click selector + 坐标 + type + key");
    let r = session
        .act(&WebAction::Click {
            selector: Some("#btn".into()),
            x: None,
            y: None,
        })
        .await
        .expect("click #btn");
    assert_eq!(r["ok"], json!(true));
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#count".into()),
        })
        .await
        .unwrap();
    assert_eq!(r["text"].as_str().unwrap().trim(), "1", "点击后计数应为 1");

    // --- click 坐标（trusted 输入） ---
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/coords"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    session
        .act(&WebAction::Click {
            selector: None,
            x: Some(120.0),
            y: Some(80.0),
        })
        .await
        .expect("click x+y");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#coordlog".into()),
        })
        .await
        .unwrap();
    assert!(
        r["text"].as_str().unwrap().contains("x=120"),
        "坐标点击落点不符: {}",
        r["text"].as_str().unwrap()
    );

    // --- type（表单输入 + 提交回显；密码框只是形态，明文走守卫前的直通路径） ---
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/form"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    let r = session
        .act(&WebAction::Type {
            selector: "#username".into(),
            text: "nemesis".into(),
        })
        .await
        .expect("type username");
    assert_eq!(r["chars"], json!(7));
    session
        .act(&WebAction::Type {
            selector: "#password".into(),
            text: "hunter2secret".into(),
        })
        .await
        .expect("type password");
    session
        .act(&WebAction::Click {
            selector: Some("#submit".into()),
            x: None,
            y: None,
        })
        .await
        .unwrap();
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#greeting".into()),
        })
        .await
        .unwrap();
    assert!(
        r["text"].as_str().unwrap().contains("欢迎, nemesis"),
        "表单提交回显不符: {}",
        r["text"].as_str().unwrap()
    );

    // --- key（键盘事件）：trusted 坐标点击聚焦输入框（真实 mousedown 才
    // 聚焦；合成 click 不聚焦）→ Key 派发到焦点元素 → 冒泡被 document 捕获。
    // 无 selector 路径 = 焦点依赖前置 trusted 点击（模型先点后按的编排）。 ---
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/key"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    session
        .act(&WebAction::Click {
            selector: None,
            x: Some(110.0),
            y: Some(95.0),
        })
        .await
        .expect("trusted 点击聚焦 #kb");
    // F49（2026-11-16 十二审）：能力闸显式前置断言——v1 ChromiumCdpDriver
    // caps.trusted_input 恒 true，Edge 活体上 Key Err = 真缺陷而非环境缺席；
    // 旧写法「if let Err = eprintln 跳过」把任意 Key 失败（含回归）当环境
    // 原因静默吞掉 = 假绿通道（违反诚实 SKIP 纪律：跳过必须钉死在能力真
    // 缺席的条件上）。二期 Firefox BiDi caps 缺席时此处 fail-fast 提示分叉。
    assert!(
        session.driver().capabilities().trusted_input,
        "Edge 活体要求 trusted_input 能力（v1 恒真；若为 false 说明能力位被改，先修 capabilities）"
    );
    session
        .act(&WebAction::Key {
            key: "a".into(),
            phase: Default::default(),
            hold_ms: None,
            selector: None,
        })
        .await
        .expect("Key 动作失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#keylog".into()),
        })
        .await
        .unwrap();
    assert!(
        r["text"]
            .as_str()
            .unwrap()
            .contains("[keydown:a:trusted=true:shift=false:ctrl=false]"),
        "键盘事件应到达页面且为 trusted（CDP 派发），实测: {:?}",
        r["text"].as_str().unwrap()
    );

    // --- key + selector（焦点承接，2026-11-15 复审补）：**无前置点击**直接
    // 带 selector 按键——act 先 el.focus() 承接焦点再派发 trusted 事件。
    // 修复前该路径整包被渲染器静默丢弃（文档警告的陷阱），修复后应到达。 ---
    step("key+selector 焦点承接（无前置点击）");
    let r = session
        .act(&WebAction::Key {
            key: "b".into(),
            phase: Default::default(),
            hold_ms: None,
            selector: Some("#kb".into()),
        })
        .await
        .expect("Key+selector 失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    assert_eq!(r["focused"], json!(true), "focused 投影应=selector 路径");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#keylog".into()),
        })
        .await
        .unwrap();
    assert!(
        r["text"]
            .as_str()
            .unwrap()
            .contains("[keydown:b:trusted=true:shift=false:ctrl=false]"),
        "selector 焦点承接后键事件应到达（无前置点击），实测: {:?}",
        r["text"].as_str().unwrap()
    );

    // --- F41（2026-11-16 八审）：大写字母 = shift 包裹（Puppeteer 同款）。
    // 旧缺陷：key="A" 被 to_ascii_lowercase() 静默小写——页面只收到小写
    // "a"、无 Shift 帧，游戏 sprint（shift+W）语义丢失。修复后应收到
    // Shift down → A（大写、shiftKey=true）→ Shift up；Down 相位遇大写
    // 诚实拒绝（不猜按住语义）。 ---
    step("key 大写字母 shift 包裹（F41）");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/key"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    session
        .act(&WebAction::Key {
            key: "A".into(),
            phase: Default::default(),
            hold_ms: None,
            selector: Some("#kb".into()),
        })
        .await
        .expect("Key 大写失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#keylog".into()),
        })
        .await
        .unwrap();
    let log = r["text"].as_str().unwrap();
    assert!(
        log.contains("[keydown:Shift:trusted=true:shift=true:ctrl=false]")
            && log.contains("[keydown:A:trusted=true:shift=true:ctrl=false]")
            && log.contains("[keyup:Shift:trusted=true:shift=true:ctrl=false]"),
        "大写键应包 Shift 三帧（down→A→up），实测: {:?}",
        log
    );
    assert!(
        !log.contains("[keydown:a:"),
        "大写键不得静默小写（旧缺陷形态），实测: {:?}",
        log
    );
    // Down 相位遇大写 = 诚实拒绝。
    let r = session
        .act(&WebAction::Key {
            key: "A".into(),
            phase: KeyPhase::Down,
            hold_ms: None,
            selector: None,
        })
        .await;
    match r {
        Err(e) => assert!(
            e.contains("phase=press"),
            "大写 Down 拒绝错误应指路 press: {e}"
        ),
        Ok(_) => panic!("大写键 Down 相位应诚实拒绝"),
    }

    // --- Ctrl 组合键（F52 候选坐实段，2026-11-16 十三轮）：显式按住路径
    // （ctrl down → a down → a up → ctrl up）的修饰状态面逐帧验证——
    // modifier_bits 是 per-call 无状态：own 位（Ctrl 帧 ctrl=true）与
    // 跨帧缺失（a 帧 ctrl=false）在此坐实。 ---
    step("key Ctrl 组合键修饰状态（F52）");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/key"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    for (key, phase) in [
        ("ctrl", KeyPhase::Down),
        ("a", KeyPhase::Down),
        ("a", KeyPhase::Up),
        ("ctrl", KeyPhase::Up),
    ] {
        session
            .act(&WebAction::Key {
                key: key.into(),
                phase,
                hold_ms: None,
                selector: None,
            })
            .await
            .unwrap_or_else(|e| panic!("Ctrl 组合序列 {key}/{phase:?} 失败: {e}"));
    }
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#keylog".into()),
        })
        .await
        .unwrap();
    let log = r["text"].as_str().unwrap();
    assert!(
        log.contains("[keydown:Control:trusted=true:shift=false:ctrl=true]")
            && log.contains("[keyup:Control:trusted=true:shift=false:ctrl=true]"),
        "Ctrl 自身帧应带 ctrl=true（modifier_bits own 位），实测: {:?}",
        log
    );
    // F52 修复面：复合键名单调用（ctrl+a press）——a 帧必须带 ctrl=true
    // （跨帧缺失由组合位补齐；显式按住路径的 a 帧 ctrl=false 是无状态边界，
    // 由上方现状锁钉死）。
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/key"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    session
        .act(&WebAction::Key {
            key: "ctrl+a".into(),
            phase: Default::default(),
            hold_ms: None,
            selector: Some("#kb".into()),
        })
        .await
        .expect("复合键 ctrl+a 失败");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#keylog".into()),
        })
        .await
        .unwrap();
    let log = r["text"].as_str().unwrap();
    assert!(
        log.contains("[keydown:a:trusted=true:shift=false:ctrl=true]")
            && log.contains("[keyup:a:trusted=true:shift=false:ctrl=true]")
            && log.contains("[keydown:Control:trusted=true:shift=false:ctrl=true]")
            && log.contains("[keyup:Control:trusted=true:shift=false:ctrl=true]"),
        "复合键 ctrl+a：a 帧 ctrl=true + Control 物理帧成对，实测: {:?}",
        log
    );

    // --- key + 不可聚焦元素（负路径）：#keylog 是普通 div（无 tabindex）
    // ——focus() 是 no-op，诚实报错而非静默丢键。 ---
    step("key+selector 不可聚焦元素拒绝");
    let r = session
        .act(&WebAction::Key {
            key: "c".into(),
            phase: Default::default(),
            hold_ms: None,
            selector: Some("#keylog".into()),
        })
        .await;
    match r {
        Err(e) => assert!(e.contains("不可聚焦"), "负路径错误应说明不可聚焦: {e}"),
        Ok(_) => panic!("不可聚焦元素应诚实拒绝（静默丢键陷阱回归）"),
    }

    // --- F46（2026-11-16 十审）：Enter 带 text "\r"——textarea 换行由
    // char 事件（text 字段）插入；旧缺陷（text=null）= keydown 到达
    // keylog 有帧、但 textarea 静默不换行（无 ta-input 事件）。 ---
    step("key Enter textarea 换行（F46）");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/key"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    session
        .act(&WebAction::Key {
            key: "enter".into(),
            phase: Default::default(),
            hold_ms: None,
            selector: Some("#ta".into()),
        })
        .await
        .expect("Key Enter 失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#keylog".into()),
        })
        .await
        .unwrap();
    let log = r["text"].as_str().unwrap();
    assert!(
        log.contains("[ta-input:len=1]"),
        "Enter 应在 textarea 插入换行（text \\r → char 事件 → input），实测: {:?}",
        log
    );

    // --- scroll（滚动指示器） ---
    step("scroll");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/scroll"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    session
        .act(&WebAction::Scroll {
            x: None,
            y: None,
            dx: 0.0,
            dy: 500.0,
        })
        .await
        .expect("scroll dy=500");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#scroll-indicator".into()),
        })
        .await
        .unwrap();
    let scrolled = r["text"]
        .as_str()
        .unwrap()
        .trim()
        .parse::<i64>()
        .unwrap_or(0);
    assert!(scrolled > 0, "滚动后 scrollY 应 > 0，实测 {scrolled}");

    // --- wait_for（延迟出现元素）+ 未命中超时形态 ---
    step("wait_for");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/delay"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    let r = session
        .act(&WebAction::WaitFor {
            selector: "#late-btn".into(),
            timeout_ms: Some(5_000),
        })
        .await
        .expect("wait_for");
    assert_eq!(r["found"], json!(true));
    let r = session
        .act(&WebAction::WaitFor {
            selector: "#nonexistent".into(),
            timeout_ms: Some(600),
        })
        .await
        .expect("wait_for 未命中也 Ok");
    assert_eq!(
        r["found"],
        json!(false),
        "超时未命中应 found=false（步级失败信号）"
    );

    // --- screenshot（PNG 字节面 + F32 坐标系元数据） ---
    step("screenshot");
    let r = session
        .act(&WebAction::Screenshot)
        .await
        .expect("screenshot");
    let b64 = r["png_base64"].as_str().expect("png_base64");
    let bytes = decode_b64(b64);
    assert!(bytes.len() > 8, "PNG 字节过短");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG magic 不符");
    // F32（2026-11-15 六审）：截图归一 CSS 像素坐标系（clip scale=1），
    // 与 Input.dispatchMouseEvent 坐标 1:1——元数据面可自查。
    assert_eq!(
        r["coordinate_space"],
        json!("css_px"),
        "截图应归一 CSS 像素坐标系（F32）: {r}"
    );
    assert!(
        r["viewport"]["width"].as_f64().unwrap_or(0.0) > 0.0
            && r["viewport"]["height"].as_f64().unwrap_or(0.0) > 0.0,
        "viewport 元数据缺失: {r}"
    );
    assert!(r["dpr"].as_f64().is_some(), "dpr 元数据缺失: {r}");

    // --- F33：JS 对话框探测-自动关闭（trusted 输入前置探针） ---
    step("F33 对话框自动关闭");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/dialog"),
            timeout_ms: Some(10_000),
        })
        .await
        .unwrap();
    // selector 点击触发 50ms 延迟 alert（click 本身先返回，对话框随后开着
    // 暂停页面 JS——trusted 输入从此被渲染器静默吞）。
    session
        .act(&WebAction::Click {
            selector: Some("#dlg".into()),
            x: None,
            y: None,
        })
        .await
        .expect("click #dlg");
    tokio::time::sleep(Duration::from_millis(500)).await;
    // 坐标点击前置探针应自动关闭对话框并在回执注明；页面 JS 随后恢复。
    let r = session
        .act(&WebAction::Click {
            selector: None,
            x: Some(150.0),
            y: Some(200.0),
        })
        .await
        .expect("对话框场景点击应自愈（F33）");
    assert_eq!(
        r["dialog_dismissed"],
        json!(true),
        "应探测并关闭对话框且回执注明: {r}"
    );
    // 恢复验证：evaluate 不再被暂停（extract 正常返回，页面 JS 已复活）。
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#after".into()),
        })
        .await
        .expect("对话框关闭后页面 JS 应恢复");
    assert_eq!(r["text"].as_str().unwrap().trim(), "BEFORE");

    // --- F34：工作 tab 被手关 → act 诚实报错 + 指针清空 + 重新点名恢复 ---
    step("F34 工作 tab 手关恢复路径");
    // 当前工作 tab = t2（多 tab 段点名）；用户手关（Target.closeTarget 模拟）。
    session.driver().close_tab(&t2.id).await.unwrap();
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#h".into()),
        })
        .await;
    match r {
        Err(e) => {
            assert!(
                e.contains("已被关闭") && e.contains("重新点选"),
                "死指针 act 应给恢复指引（F34）: {e}"
            );
        }
        Ok(_) => panic!("死指针 act 应诚实报错而非撞传输层错误死循环（F34）"),
    }
    assert_eq!(
        session.working_tab_id(),
        None,
        "死指针应被清空（不留幻影工作 tab）"
    );
    // 重新点名存活 tab 后恢复正常（恢复路径可用性）。
    session.set_working_tab(&first).await.unwrap();
    let r = session
        .act(&WebAction::ExtractText { selector: None })
        .await
        .expect("重新点名后 act 应恢复");
    assert_eq!(r["ok"], json!(true));

    // --- F38（2026-11-16 七审）：截图 clip 滚动词域——滚动后截图必须跟随
    // 视口（旧 F32 写法 clip 锚文档 (0,0)：滚动页截图=文档顶部，与用户
    // 所见画面脱节）。字节差断言（无图像解码依赖）：滚动前后截图必须
    // 不同；同位置连拍必须相同（确定性对照——证明 A != B 不是编码噪声
    // 而是内容真的变了）。 ---
    step("F38 截图滚动词域");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/tall"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    let shot_a = {
        let r = session
            .act(&WebAction::Screenshot)
            .await
            .expect("F38 shotA");
        r["png_base64"].as_str().unwrap().to_string()
    };
    session
        .act(&WebAction::Scroll {
            x: None,
            y: None,
            dx: 0.0,
            dy: 800.0,
        })
        .await
        .expect("F38 scroll");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r = session
        .act(&WebAction::Screenshot)
        .await
        .expect("F38 shotB");
    assert_eq!(
        r["coordinate_space"],
        json!("css_px"),
        "F38 元数据照常（CSS 像素坐标系）"
    );
    let shot_b = r["png_base64"].as_str().unwrap().to_string();
    let r2 = session
        .act(&WebAction::Screenshot)
        .await
        .expect("F38 shotB2");
    let shot_b2 = r2["png_base64"].as_str().unwrap().to_string();
    assert_eq!(
        shot_b, shot_b2,
        "同位置连拍应字节相同（确定性对照；若不同=编码不确定，A!=B 断言被架空）"
    );
    assert_ne!(
        shot_a, shot_b,
        "滚动前后截图必须不同（clip 锚文档 (0,0) 的旧缺陷下两者都是文档顶部=字节相同）"
    );

    // --- F54（2026-11-16 十四轮）：坐标闭环——用户锚定场景（截图→量坐标
    // →点击→命中）端到端实证。F32/F38 修过 clip scale 与滚动锚，但「滚动
    // 页面上按换算后的视口坐标点击命中预期元素」从未真跑过。页面自报按钮
    // 文档坐标（getBoundingClientRect + scrollY），滚动后换算视口坐标点击
    // ，命中断言 = 坐标系一致性闭环成立。 ---
    step("F54 坐标闭环（截图→量坐标→点击→命中）");
    session
        .act(&WebAction::Navigate {
            url: format!("{base}/coord"),
            timeout_ms: None,
        })
        .await
        .unwrap();
    // 页面 rect 自报「DOC:x,y VIEW:x,y」（scroll 时实时刷新）。
    let parse_pair = |s: &str| -> (f64, f64) {
        let (x, y) = s.split_once(',').expect("rect pair x,y");
        (
            x.trim().parse().expect("rect x 数值"),
            y.trim().parse().expect("rect y 数值"),
        )
    };
    let parse_rect = |s: &str| -> ((f64, f64), (f64, f64)) {
        let (d, v) = s
            .split_once(" VIEW:")
            .expect("rectinfo 缺 VIEW 段（页面脚本未跑）");
        (
            parse_pair(d.strip_prefix("DOC:").expect("DOC 前缀")),
            parse_pair(v),
        )
    };
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#rectinfo".into()),
        })
        .await
        .unwrap();
    let rect_text = r["text"].as_str().unwrap();
    let ((dx0, dy0), (vx0, vy0)) = parse_rect(rect_text);
    assert!(
        (dx0 - 300.0).abs() < 2.0 && (dy0 - 900.0).abs() < 2.0,
        "按钮文档坐标应约 (300,900)，实测 ({dx0},{dy0})"
    );
    assert!(
        (vx0 - 300.0).abs() < 2.0 && (vy0 - 900.0).abs() < 2.0,
        "载入时视口坐标应=文档坐标（scrollY=0），实测 ({vx0},{vy0})"
    );
    // 滚 500px：wheel 是 trusted 输入，页面随动——按钮视口位置由页面自报
    // （模拟「截图上量出来的像素」，不硬算 scrollY）。
    session
        .act(&WebAction::Scroll {
            x: None,
            y: None,
            dx: 0.0,
            dy: 500.0,
        })
        .await
        .expect("F54 scroll");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#rectinfo".into()),
        })
        .await
        .unwrap();
    let (_, (vx, vy)) = parse_rect(r["text"].as_str().unwrap());
    assert!(
        (vy - 400.0).abs() < 3.0,
        "滚 500 后按钮视口 y 应约 400（900-500），实测 {vy}——wheel 滚动量未按 dy 落地"
    );
    // 「截图上按钮中心像素」= (vx+60, vy+20)，直发坐标点击（trusted 输入）。
    session
        .act(&WebAction::Click {
            selector: None,
            x: Some(vx + 60.0),
            y: Some(vy + 20.0),
        })
        .await
        .expect("F54 坐标点击");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#hits".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        r["text"].as_str().unwrap(),
        "1",
        "滚动页面按测得视口坐标点击必须命中按钮（截图像素=视口坐标闭环），实测: {:?}",
        r["text"]
    );
    // 反向对照：坐标空间混淆形态——未换算的文档 y（dy0=900 → 中心 920）直发
    // 视口 = 文档 1420（空白垫高区），必不命中；命中即说明坐标系断言被架空。
    session
        .act(&WebAction::Click {
            selector: None,
            x: Some(vx + 60.0),
            y: Some(dy0 + 20.0),
        })
        .await
        .expect("F54 反向坐标点击（应落空）");
    let r = session
        .act(&WebAction::ExtractText {
            selector: Some("#hits".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        r["text"].as_str().unwrap(),
        "1",
        "未换算的文档坐标直发视口必须不命中（命中=坐标系断言架空），实测: {:?}",
        r["text"]
    );

    // --- 收尾：优雅关闭（落盘 cookie/session） ---
    step("close");
    let r = session.close().await.expect("close");
    assert_eq!(r["ok"], json!(true));
    step("全部通过 ✓");
}

// base64 解码（避免引依赖：nemesis-browser 生产依赖无 base64；此处内联小解码器
// 只服务本测试的 PNG magic 断言）。
fn decode_b64(s: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = match TABLE.iter().position(|&t| t == c) {
            Some(i) => i as u32,
            None => continue,
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    out
}
