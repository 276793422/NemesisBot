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
use nemesis_browser::{BrowserDriver, BrowserManager, BrowserPrefs, BrowserSession};
use serde_json::json;

// 同进程内两个活体测试串行化（2026-11-18 实测：双 Edge 实例并行重活时
// CDP socket 被 Edge 侧重置——os error 10054，两轮死在不同步骤=时序依赖
// 的资源争用抖动，非协议缺陷）。活体测试本来就重，串行换稳定。
static EDGE_LIVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// 本地站点（axum；每个测试页一个行为面）
// ---------------------------------------------------------------------------

async fn serve_site(
    extra_oopif_src: Option<String>,
    extra_dialog_src: Option<String>,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::Router;
    use axum::routing::get;

    // bind 前移（iframe 宿主页需要 base 拼 iframe src）。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

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

    async fn linkout_page() -> axum::response::Html<&'static str> {
        // 所有权闭环（2026-11-28 批）专用页：target=_blank 锚点 = 新 tab
        // 生成器。**必须走 trusted 坐标点击**：合成 el.click() 无 user
        // activation，Chromium 弹窗拦截器把无激活的 target=_blank 导航当
        // popup 吞掉（活体实证：点后无第二个 tab）；trusted 输入 = 真
        // user activation，开 tab 放行（生产故事同源：游戏反作弊认这个）。
        // 固定绝对坐标 200x40 大块 → 坐标点击 (100,60) 稳定命中。
        axum::response::Html(
            r##"<html><head><title>链接页</title></head><body style="margin:0">
<p id="p1" style="position:absolute;left:40px;top:100px">NEMESIS_LINKOUT_OK</p>
<a id="newtab" href="/text" target="_blank"
   style="position:absolute;left:40px;top:40px;width:200px;height:40px;
          display:block;background:#ccc;line-height:40px;text-align:center">新窗口打开文本页</a>
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

    // DOM 只读三动作验证页（2026-11-18 计划 B3）：details 展开态 / 密码框
    // 页面源码 value 预填（SSO 场景——脱敏目标）/ checkbox（property 实时
    // 性锁）/ disabled+aria+data 按钮（attribute 白名单）/ 初始文本
    // textarea（清空损失面）/ shadow host（穿透）/ 叶子列表（text 面源）。
    async fn dom_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>DOM 验证页</title></head><body id="dom-body">
<details id="details" open><summary>展开区</summary><p>内容可见</p></details>
<input id="pw" type="password" value="sso-prefill-secret" placeholder="密码">
<input id="cb" type="checkbox">
<button id="btndis" disabled aria-expanded="false" data-slug="nb-dom" class="btn primary">禁用按钮</button>
<textarea id="ta-init">初始多行文本
第二行</textarea>
<div id="host"></div>
<ul id="leafs"><li id="leaf1">叶子一 NEMESIS_LEAF_OK</li><li>叶子二</li></ul>
<script>
document.getElementById('host').attachShadow({mode: 'open'});
document.getElementById('host').shadowRoot.innerHTML =
  '<span id="in-shadow">SHADOW_CONTENT_OK</span>';
</script>
<style>.x{color:red}</style>
</body></html>"##,
        )
    }

    // 同源 iframe 内页（同端口 = 同源 = 同进程 frame；目标元素三动作）。
    async fn iframe_inner_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>同源内页</title></head><body>
<h1 id="inner-title">INNER_OK</h1>
<details id="inner-details" open><summary>内层</summary></details>
<input id="inner-pw" type="password" value="inner-prefill-secret">
</body></html>"##,
        )
    }

    // 解析期弹框页（六轮复核活体锁 ⑤）：alert 在解析中途同步弹出，阻塞
    // parser = readyState 停滞 + 加载等待轮询撞框——逼出
    // wait_document_ready 的短轮询 + deadline 顺延路径。
    async fn dialog_parse_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>解析期弹框</title></head><body>
<h1 id="parse-title">PARSE_DIALOG_OK</h1>
<script>alert('NEMESIS_PARSE_DIALOG');</script>
<p id="after-parse">AFTER_PARSE_OK</p>
</body></html>"##,
        )
    }

    // 等待中途弹框页（六轮复核活体锁 ④）：1s alert（wait_for 轮询中途
    // 撞框）+ 2.5s 追加元素（overdue 定时器在 alert 关闭后才补跑）——
    // 逼出 wait_for 的入口探针 miss + 短轮询自愈 + deadline 顺延命中。
    async fn dialog_wait_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>等待中途弹框</title></head><body>
<h1>wait dialog page</h1>
<script>
setTimeout(function(){ alert('NEMESIS_WAIT_DIALOG'); }, 1000);
setTimeout(function(){
  var b = document.createElement('button');
  b.id = 'late-btn'; b.textContent = 'LATE_BTN_OK';
  document.body.appendChild(b);
}, 2500);
</script>
</body></html>"##,
        )
    }

    // iframe 内延迟弹框页（2026-11-18 交付后自查修正 1 的活体锁内页）：
    // alert 刻意延迟 2s——晚于宿主页 load（若在 navigate 期间就弹框会被
    // navigate 自身的 F33 超时臂关掉，走不到 route 路径），早于测试探测。
    async fn iframe_dialog_inner_page() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>内页弹框</title></head><body>
<h1 id="d-title">DIALOG_FRAME_OK</h1>
<script>
setTimeout(function(){ alert('NEMESIS_IFRAME_DIALOG'); }, 2000);
</script>
</body></html>"##,
        )
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/text", get(text_page))
        .route("/linkout", get(linkout_page))
        .route("/form", get(form_page))
        .route("/click", get(click_page))
        .route("/coords", get(coords_page))
        .route("/scroll", get(scroll_page))
        .route("/delay", get(delay_page))
        .route("/dialog", get(dialog_page))
        .route("/tall", get(tall_page))
        .route("/key", get(key_page))
        .route("/coord", get(coord_page))
        .route("/dom", get(dom_page))
        .route("/iframe-inner", get(iframe_inner_page))
        .route("/iframe-dialog-inner", get(iframe_dialog_inner_page))
        .route("/dialog-parse", get(dialog_parse_page))
        .route("/dialog-wait", get(dialog_wait_page));
    // 同源 iframe 宿主页（src 需 base——bind 前移的原因）。
    let iframe_host_page = format!(
        r##"<html><head><title>同源 iframe 宿主页</title></head><body>
<iframe id="frameA" src="{base}/iframe-inner" width="500" height="300"></iframe>
<p id="host-title">HOST_PAGE_OK</p>
</body></html>"##
    );
    let app = app.route(
        "/iframe-host",
        get(move || {
            let p = iframe_host_page.clone();
            async move { axum::response::Html(p) }
        }),
    );
    // iframe 弹框宿主页（src 需 base；同端口 = 同源 = 同进程 frame）。
    let dialog_host_page = format!(
        r##"<html><head><title>弹框宿主页</title></head><body>
<iframe id="frameD" src="{base}/iframe-dialog-inner" width="500" height="300"></iframe>
</body></html>"##
    );
    let app = app.route(
        "/iframe-dialog-host",
        get(move || {
            let p = dialog_host_page.clone();
            async move { axum::response::Html(p) }
        }),
    );
    // 跨域 OOPIF 宿主页（可选——第二 axum 实例的 src 传入；origin 含端口
    // 不同 = 跨源 = site isolation 下 OOPIF）。
    let app = if let Some(src) = extra_oopif_src {
        let x_host_page = format!(
            r##"<html><head><title>跨域宿主页</title></head><body>
<iframe id="frameX" src="{src}" width="500" height="300"></iframe>
<p id="xhost-title">XHOST_PAGE_OK</p>
</body></html>"##
        );
        app.route(
            "/x-host",
            get(move || {
                let p = x_host_page.clone();
                async move { axum::response::Html(p) }
            }),
        )
    } else {
        app
    };
    // OOPIF 弹框宿主页（可选——src 指向第二实例的弹框内页；eval_conn
    // session 路由臂活体面，2026-11-18 四轮复核）。
    let app = if let Some(src) = extra_dialog_src {
        let xd_host_page = format!(
            r##"<html><head><title>OOPIF 弹框宿主页</title></head><body>
<iframe id="frameXD" src="{src}" width="500" height="300"></iframe>
</body></html>"##
        );
        app.route(
            "/x-dialog-host",
            get(move || {
                let p = xd_host_page.clone();
                async move { axum::response::Html(p) }
            }),
        )
    } else {
        app
    };
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, handle)
}

/// 第二 axum 实例（2026-11-18 计划 B3）：只服务跨域内页（密码预填 +
/// details + 标题——OOPIF 三动作 + 脱敏正交断言面）。
///
/// **绑 127.0.0.2 而非同 IP 不同端口**（首版实测踩坑）：origin 含端口=
/// 跨源没错，但 Chromium 的 **进程级 site isolation 按 site 分——IP 主机
/// 的 site 是 IP 本身、忽略端口**，127.0.0.1:A 与 127.0.0.1:B 同站点 →
/// iframe 根本不是 OOPIF（frame 树诚实标注 [same-process]，动作走同进程
/// 隔离 world 路径照样全过——假绿OOPIF）。不同回环 IP = 不同 site → 真
/// site isolation → 真 OOPIF target。
async fn serve_oopif_site() -> (String, tokio::task::JoinHandle<()>) {
    use axum::Router;
    use axum::routing::get;

    async fn x_inner() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>跨域内页</title></head><body>
<h1 id="x-title">OOPIF_OK</h1>
<details id="x-details" open><summary>跨域内层</summary></details>
<input id="x-pw" type="password" value="x-prefill-secret">
</body></html>"##,
        )
    }

    // OOPIF 弹框内页（eval_conn session 路由臂活体面）：alert 延迟 2s——
    // 晚于宿主 load（导航期弹框会被 navigate 的 F33 臂先关掉），早于探测。
    async fn x_dialog_inner() -> axum::response::Html<&'static str> {
        axum::response::Html(
            r##"<html><head><title>跨域弹框内页</title></head><body>
<h1 id="xd-title">XDIALOG_OK</h1>
<script>
setTimeout(function(){ alert('NEMESIS_XDIALOG'); }, 2000);
</script>
</body></html>"##,
        )
    }

    let app = Router::new()
        .route("/x-inner", get(x_inner))
        .route("/x-dialog-inner", get(x_dialog_inner));
    // 整个 127/8 都是本机回环（Windows/Linux 同），绑 127.0.0.2 无需特权。
    let listener = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
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
    match manager.spawn(Some("edge"), None, None).await {
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
    // 双活体测试进程内串行（见 EDGE_LIVE_LOCK 注）。
    let _serial = EDGE_LIVE_LOCK.lock().await;
    step("本地站点启动");
    let (base, _server) = serve_site(None, None).await;
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/text"),
                timeout_ms: Some(15_000),
            },
            None,
        )
        .await
        .expect("navigate /text");
    assert_eq!(r["ok"], json!(true));
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#p1".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Click {
                selector: Some("#btn".into()),
                x: None,
                y: None,
            },
            None,
        )
        .await
        .expect("click #btn");
    assert_eq!(r["ok"], json!(true));
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#count".into()),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(r["text"].as_str().unwrap().trim(), "1", "点击后计数应为 1");

    // --- click 坐标（trusted 输入） ---
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/coords"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    // F55 回归见证（2026-11-23）：导航后首次 trusted 坐标点击。修复前
    // press+release 被输入管线静默吞（6/6 挂，coordlog 恒空）；driver 侧
    // mouseMoved 暖场后此断言必须稳定通过。
    session
        .act(
            &WebAction::Click {
                selector: None,
                x: Some(120.0),
                y: Some(80.0),
            },
            None,
        )
        .await
        .expect("click x+y");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#coordlog".into()),
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        r["text"].as_str().unwrap().contains("x=120"),
        "坐标点击落点不符: {}",
        r["text"].as_str().unwrap()
    );

    // --- type（表单输入 + 提交回显；密码框只是形态，明文走守卫前的直通路径） ---
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/form"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    let r = session
        .act(
            &WebAction::Type {
                selector: "#username".into(),
                text: "nemesis".into(),
            },
            None,
        )
        .await
        .expect("type username");
    assert_eq!(r["chars"], json!(7));
    session
        .act(
            &WebAction::Type {
                selector: "#password".into(),
                text: "hunter2secret".into(),
            },
            None,
        )
        .await
        .expect("type password");
    session
        .act(
            &WebAction::Click {
                selector: Some("#submit".into()),
                x: None,
                y: None,
            },
            None,
        )
        .await
        .unwrap();
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#greeting".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/key"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    session
        .act(
            &WebAction::Click {
                selector: None,
                x: Some(110.0),
                y: Some(95.0),
            },
            None,
        )
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
        .act(
            &WebAction::Key {
                key: "a".into(),
                phase: Default::default(),
                hold_ms: None,
                selector: None,
            },
            None,
        )
        .await
        .expect("Key 动作失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Key {
                key: "b".into(),
                phase: Default::default(),
                hold_ms: None,
                selector: Some("#kb".into()),
            },
            None,
        )
        .await
        .expect("Key+selector 失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    assert_eq!(r["focused"], json!(true), "focused 投影应=selector 路径");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/key"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    session
        .act(
            &WebAction::Key {
                key: "A".into(),
                phase: Default::default(),
                hold_ms: None,
                selector: Some("#kb".into()),
            },
            None,
        )
        .await
        .expect("Key 大写失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Key {
                key: "A".into(),
                phase: KeyPhase::Down,
                hold_ms: None,
                selector: None,
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/key"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    for (key, phase) in [
        ("ctrl", KeyPhase::Down),
        ("a", KeyPhase::Down),
        ("a", KeyPhase::Up),
        ("ctrl", KeyPhase::Up),
    ] {
        session
            .act(
                &WebAction::Key {
                    key: key.into(),
                    phase,
                    hold_ms: None,
                    selector: None,
                },
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("Ctrl 组合序列 {key}/{phase:?} 失败: {e}"));
    }
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/key"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    session
        .act(
            &WebAction::Key {
                key: "ctrl+a".into(),
                phase: Default::default(),
                hold_ms: None,
                selector: Some("#kb".into()),
            },
            None,
        )
        .await
        .expect("复合键 ctrl+a 失败");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Key {
                key: "c".into(),
                phase: Default::default(),
                hold_ms: None,
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/key"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    session
        .act(
            &WebAction::Key {
                key: "enter".into(),
                phase: Default::default(),
                hold_ms: None,
                selector: Some("#ta".into()),
            },
            None,
        )
        .await
        .expect("Key Enter 失败——Edge 活体上 Key Err = 真缺陷（F49 禁静默跳过）");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#keylog".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/scroll"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    session
        .act(
            &WebAction::Scroll {
                x: None,
                y: None,
                dx: 0.0,
                dy: 500.0,
            },
            None,
        )
        .await
        .expect("scroll dy=500");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#scroll-indicator".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/delay"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    let r = session
        .act(
            &WebAction::WaitFor {
                selector: "#late-btn".into(),
                timeout_ms: Some(5_000),
            },
            None,
        )
        .await
        .expect("wait_for");
    assert_eq!(r["found"], json!(true));
    let r = session
        .act(
            &WebAction::WaitFor {
                selector: "#nonexistent".into(),
                timeout_ms: Some(600),
            },
            None,
        )
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
        .act(&WebAction::Screenshot, None)
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/dialog"),
                timeout_ms: Some(10_000),
            },
            None,
        )
        .await
        .unwrap();
    // selector 点击触发 50ms 延迟 alert（click 本身先返回，对话框随后开着
    // 暂停页面 JS——trusted 输入从此被渲染器静默吞）。
    session
        .act(
            &WebAction::Click {
                selector: Some("#dlg".into()),
                x: None,
                y: None,
            },
            None,
        )
        .await
        .expect("click #dlg");
    tokio::time::sleep(Duration::from_millis(500)).await;
    // 坐标点击前置探针应自动关闭对话框并在回执注明；页面 JS 随后恢复。
    let r = session
        .act(
            &WebAction::Click {
                selector: None,
                x: Some(150.0),
                y: Some(200.0),
            },
            None,
        )
        .await
        .expect("对话框场景点击应自愈（F33）");
    assert_eq!(
        r["dialog_dismissed"],
        json!(true),
        "应探测并关闭对话框且回执注明: {r}"
    );
    // 恢复验证：evaluate 不再被暂停（extract 正常返回，页面 JS 已复活）。
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#after".into()),
            },
            None,
        )
        .await
        .expect("对话框关闭后页面 JS 应恢复");
    assert_eq!(r["text"].as_str().unwrap().trim(), "BEFORE");

    // --- F34：工作 tab 被手关 → act 诚实报错 + 指针清空 + 重新点名恢复 ---
    step("F34 工作 tab 手关恢复路径");
    // 当前工作 tab = t2（多 tab 段点名）；用户手关（Target.closeTarget 模拟）。
    session.driver().close_tab(&t2.id).await.unwrap();
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#h".into()),
            },
            None,
        )
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
        .act(&WebAction::ExtractText { selector: None }, None)
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/tall"),
                timeout_ms: None,
            },
            None,
        )
        .await
        .unwrap();
    let shot_a = {
        let r = session
            .act(&WebAction::Screenshot, None)
            .await
            .expect("F38 shotA");
        r["png_base64"].as_str().unwrap().to_string()
    };
    session
        .act(
            &WebAction::Scroll {
                x: None,
                y: None,
                dx: 0.0,
                dy: 800.0,
            },
            None,
        )
        .await
        .expect("F38 scroll");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r = session
        .act(&WebAction::Screenshot, None)
        .await
        .expect("F38 shotB");
    assert_eq!(
        r["coordinate_space"],
        json!("css_px"),
        "F38 元数据照常（CSS 像素坐标系）"
    );
    let shot_b = r["png_base64"].as_str().unwrap().to_string();
    let r2 = session
        .act(&WebAction::Screenshot, None)
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
        .act(
            &WebAction::Navigate {
                url: format!("{base}/coord"),
                timeout_ms: None,
            },
            None,
        )
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
        .act(
            &WebAction::ExtractText {
                selector: Some("#rectinfo".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Scroll {
                x: None,
                y: None,
                dx: 0.0,
                dy: 500.0,
            },
            None,
        )
        .await
        .expect("F54 scroll");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#rectinfo".into()),
            },
            None,
        )
        .await
        .unwrap();
    let (_, (vx, vy)) = parse_rect(r["text"].as_str().unwrap());
    assert!(
        (vy - 400.0).abs() < 3.0,
        "滚 500 后按钮视口 y 应约 400（900-500），实测 {vy}——wheel 滚动量未按 dy 落地"
    );
    // 「截图上按钮中心像素」= (vx+60, vy+20)，直发坐标点击（trusted 输入）。
    session
        .act(
            &WebAction::Click {
                selector: None,
                x: Some(vx + 60.0),
                y: Some(vy + 20.0),
            },
            None,
        )
        .await
        .expect("F54 坐标点击");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#hits".into()),
            },
            None,
        )
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
        .act(
            &WebAction::Click {
                selector: None,
                x: Some(vx + 60.0),
                y: Some(dy0 + 20.0),
            },
            None,
        )
        .await
        .expect("F54 反向坐标点击（应落空）");
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#hits".into()),
            },
            None,
        )
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

// frame 内动作带重试（2026-11-18 计划 B3）：iframe/OOPIF 的 target 注册
// 滞后于 navigate 返回（site isolation 进程孵化需要时间），首次 resolve
// 可能「frame 未找到」——退避重试；其余错误（含 stale 自愈失败）立即返回，
// 诚实报错不被重试掩盖。
async fn act_in_frame_with_retry(
    session: &BrowserSession,
    action: &WebAction,
    tries: usize,
) -> Result<serde_json::Value, String> {
    let stale = |e: &str| {
        e.contains("frame 未找到")
            || e.contains("Cannot find context")
            || e.contains("No frame with given id")
            || e.contains("Target closed")
            || e.contains("Session closed")
    };
    let mut last = String::new();
    for _ in 0..tries {
        match session.act(action, None).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !stale(&e) {
                    return Err(e);
                }
                last = e;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    Err(format!("frame 定位重试 {tries} 次仍失败: {last}"))
}

/// DOM 只读三动作活体矩阵（2026-11-18 计划 §6 验收 4-7）：真 spawn Edge，
/// 覆盖 inspect 属性-attribute 分层与白名单、snapshot 有界遍历（shadow/
/// 叶子 text/双闸钳制）、get_dom 脱敏与截断、同进程 iframe 与跨域 OOPIF
/// 双路径、frame 清单诚实报错、导航后自愈。与 edge_live_full_matrix 并行
/// 跑（各自独立 Edge 实例 + profile，互不干扰）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edge_live_dom_read_actions() {
    fn step(name: &str) {
        eprintln!("[dom_live] {name}");
    }
    // 双活体测试进程内串行（见 EDGE_LIVE_LOCK 注）。
    let _serial = EDGE_LIVE_LOCK.lock().await;
    step("本地站点 + OOPIF 站点启动");
    let (base2, _oopif_server) = serve_oopif_site().await;
    let port2 = base2.rsplit(':').next().unwrap_or("").to_string();
    let (base, _server) = serve_site(
        Some(format!("{base2}/x-inner")),
        Some(format!("{base2}/x-dialog-inner")),
    )
    .await;
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
    // panic hook 树杀（同 edge_live_full_matrix 纪律）。
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

    // ---- 主 frame inspect：property/attribute 分层 + 白名单 ----
    step("navigate /dom + inspect details");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/dom"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate /dom");
    let r = session
        .act(
            &WebAction::Inspect {
                selector: "#details".into(),
                frame: None,
            },
            None,
        )
        .await
        .expect("inspect #details");
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["tag"], json!("details"));
    assert_eq!(r["open"], json!(true), "details 展开态 property");
    assert_eq!(r["frame"], json!("main"), "缺省 frame 回显 main");
    assert_eq!(r["visible"], json!(true));
    assert!(
        r["rect"]["w"].as_f64().unwrap_or(0.0) > 0.0,
        "rect 几何面: {r}"
    );

    step("inspect disabled/aria/data 白名单分层");
    let r = session
        .act(
            &WebAction::Inspect {
                selector: "#btndis".into(),
                frame: None,
            },
            None,
        )
        .await
        .expect("inspect #btndis");
    assert_eq!(r["disabled"], json!(true), "disabled property（动态态面）");
    assert_eq!(
        r["expanded"],
        json!("false"),
        "aria-expanded attribute（字符串非布尔）"
    );
    assert_eq!(
        r["attributes"]["data-slug"],
        json!("nb-dom"),
        "data-* 白名单"
    );
    assert_eq!(
        r["attributes"]["class"],
        json!("btn primary"),
        "class 白名单"
    );

    step("inspect 密码框 value 恒缺");
    let r = session
        .act(
            &WebAction::Inspect {
                selector: "#pw".into(),
                frame: None,
            },
            None,
        )
        .await
        .expect("inspect #pw");
    assert!(
        !r.to_string().contains("sso-prefill-secret"),
        "inspect 输出不得含 value 明文: {r}"
    );
    assert_eq!(r["type"], json!("password"));
    assert_eq!(
        r["attributes"].get("value"),
        None,
        "白名单不含 value 键: {r}"
    );

    step("inspect 未命中诚实报错 + extract 缺口对照");
    let r = session
        .act(
            &WebAction::Inspect {
                selector: "#nonexistent".into(),
                frame: None,
            },
            None,
        )
        .await;
    assert!(
        matches!(&r, Err(e) if e.contains("未找到元素")),
        "inspect 未命中应诚实报错: {r:?}"
    );
    let r = session
        .act(
            &WebAction::ExtractText {
                selector: Some("#btndis".into()),
            },
            None,
        )
        .await
        .expect("extract #btndis");
    assert_eq!(
        r["text"].as_str().unwrap_or("").trim(),
        "禁用按钮",
        "extract 只拿文本——无 disabled/aria 状态面（缺口对照）: {r}"
    );

    step("property 实时性锁：click checkbox → checked=true");
    session
        .act(
            &WebAction::Click {
                selector: Some("#cb".into()),
                x: None,
                y: None,
            },
            None,
        )
        .await
        .expect("click #cb");
    let r = session
        .act(
            &WebAction::Inspect {
                selector: "#cb".into(),
                frame: None,
            },
            None,
        )
        .await
        .expect("inspect #cb");
    assert_eq!(
        r["checked"],
        json!(true),
        "点击后 checked property 应实时为 true: {r}"
    );

    // ---- snapshot：shadow 穿透 / 叶子 text / 双闸钳制 ----
    step("snapshot 全页");
    let r = session
        .act(
            &WebAction::SnapshotDom {
                selector: None,
                frame: None,
                max_depth: None,
                max_nodes: None,
            },
            None,
        )
        .await
        .expect("snapshot body");
    assert_eq!(r["ok"], json!(true));
    assert!(r["ts_ms"].as_u64().is_some(), "ts_ms 在场");
    assert_eq!(r["truncated"], json!(false), "页面小于缺省闸不截断");
    let nodes = r["nodes"].as_array().expect("nodes array");
    assert!(
        nodes
            .iter()
            .any(|n| n["id"] == json!("details") && n["open"] == json!(true)),
        "details 入树（open 布尔投影）: {r}"
    );
    assert!(
        nodes.iter().any(|n| n["id"] == json!("in-shadow")
            && n["shadow"] == json!(true)
            && n["text"] == json!("SHADOW_CONTENT_OK")),
        "shadowRoot 穿透: {r}"
    );
    assert!(
        nodes.iter().any(|n| n["id"] == json!("leaf1")
            && n["text"].as_str().unwrap_or("").contains("NEMESIS_LEAF_OK")),
        "叶子 text 面: {r}"
    );

    step("snapshot 钳制：depth 下限 / nodes 上限");
    let r = session
        .act(
            &WebAction::SnapshotDom {
                selector: None,
                frame: None,
                max_depth: Some(0),
                max_nodes: None,
            },
            None,
        )
        .await
        .expect("snapshot depth=0");
    assert!(
        r["nodes_emitted"].as_u64().unwrap_or(0) > 1,
        "max_depth=0 钳到 1（根+子层入树）: {r}"
    );
    assert_eq!(r["truncated"], json!(true), "深层被剪=截断语义");
    let r = session
        .act(
            &WebAction::SnapshotDom {
                selector: None,
                frame: None,
                max_depth: None,
                max_nodes: Some(3),
            },
            None,
        )
        .await
        .expect("snapshot nodes=3");
    assert_eq!(r["nodes_emitted"], json!(3), "节点闸生效: {r}");
    assert_eq!(r["truncated"], json!(true), "触顶诚实标记");

    // ---- get_dom：脱敏 + 截断 + 钳制 ----
    step("get_dom body 脱敏");
    let r = session
        .act(
            &WebAction::GetDom {
                selector: "body".into(),
                frame: None,
                max_chars: None,
            },
            None,
        )
        .await
        .expect("get_dom body");
    let html = r["html"].as_str().expect("html str");
    assert!(
        !html.contains("sso-prefill-secret"),
        "页面源码 value 属性应被剥除: {html}"
    );
    assert!(
        !html.contains("初始多行文本"),
        "textarea textContent 应被清空（宁过度脱敏）: {html}"
    );
    assert!(!html.contains("<script"), "script 应被删: {html}");
    assert!(!html.contains("<style"), "style 应被删: {html}");
    assert!(html.contains("<details"), "结构保留: {html}");
    assert_eq!(r["truncated"], json!(false), "小于缺省 cap 不截断");
    let chars0 = r["chars"].as_u64().expect("chars");

    step("get_dom 钳制 + 截断注记");
    let r = session
        .act(
            &WebAction::GetDom {
                selector: "body".into(),
                frame: None,
                max_chars: Some(200),
            },
            None,
        )
        .await
        .expect("get_dom cap=200");
    assert_eq!(r["truncated"], json!(true), "超 cap 截断");
    assert_eq!(
        r["chars"].as_u64(),
        Some(chars0),
        "chars = 原始总字符（非截后）"
    );
    assert!(
        r["html"].as_str().unwrap_or("").contains("已截断"),
        "截断注记在场: {r}"
    );
    let r_low = session
        .act(
            &WebAction::GetDom {
                selector: "body".into(),
                frame: None,
                max_chars: Some(100),
            },
            None,
        )
        .await
        .expect("get_dom cap=100");
    assert!(
        r_low["html"].as_str().unwrap_or("").chars().count() > 150,
        "max_chars=100 应钳到 200（截后 >150 字符）: {}",
        r_low["html"].as_str().unwrap_or("").chars().count()
    );
    let r_hi = session
        .act(
            &WebAction::GetDom {
                selector: "body".into(),
                frame: None,
                max_chars: Some(999_999),
            },
            None,
        )
        .await
        .expect("get_dom cap=999999");
    assert_eq!(r_hi["truncated"], json!(false), "上限钳 50000 内页面不截断");

    // ---- 同进程 iframe（§3.7 路径 A）----
    step("同源 iframe 三动作");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/iframe-host"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate /iframe-host");
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::Inspect {
            selector: "#inner-title".into(),
            frame: Some("/iframe-inner".into()),
        },
        20,
    )
    .await
    .expect("iframe inspect");
    assert_eq!(r["id"], json!("inner-title"), "同源 iframe inspect: {r}");
    assert_eq!(r["frame"], json!("/iframe-inner"), "frame 回显 want 子串");
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::SnapshotDom {
            selector: None,
            frame: Some("/iframe-inner".into()),
            max_depth: None,
            max_nodes: None,
        },
        20,
    )
    .await
    .expect("iframe snapshot");
    assert!(
        r.to_string().contains("INNER_OK"),
        "同源 iframe snapshot: {r}"
    );
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::GetDom {
            selector: "#inner-pw".into(),
            frame: Some("/iframe-inner".into()),
            max_chars: None,
        },
        20,
    )
    .await
    .expect("iframe get_dom");
    let html = r["html"].as_str().expect("iframe html");
    assert!(
        !html.contains("inner-prefill-secret"),
        "同源 iframe 脱敏照常: {html}"
    );
    assert!(html.contains("input"), "同源 iframe get_dom 内容: {html}");

    // ---- 跨域 OOPIF（§3.7 路径 B；不同回环 IP = 不同 site = 真 OOPIF，
    //      见 serve_oopif_site 注——首版端口隔离是假 OOPIF）----
    step("跨域 OOPIF 三动作");
    let frame_x = format!(":{port2}/x-inner");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/x-host"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate /x-host");
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::Inspect {
            selector: "#x-title".into(),
            frame: Some(frame_x.clone()),
        },
        30,
    )
    .await
    .expect("OOPIF inspect");
    assert_eq!(r["id"], json!("x-title"), "OOPIF inspect: {r}");
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::SnapshotDom {
            selector: Some("#x-details".into()),
            frame: Some(frame_x.clone()),
            max_depth: None,
            max_nodes: None,
        },
        20,
    )
    .await
    .expect("OOPIF snapshot");
    assert!(
        r["nodes_emitted"].as_u64().unwrap_or(0) >= 1,
        "OOPIF snapshot: {r}"
    );
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::GetDom {
            selector: "body".into(),
            frame: Some(frame_x.clone()),
            max_chars: None,
        },
        20,
    )
    .await
    .expect("OOPIF get_dom");
    let html = r["html"].as_str().expect("OOPIF html");
    assert!(
        !html.contains("x-prefill-secret"),
        "OOPIF 脱敏照常（路径正交）: {html}"
    );
    assert!(html.contains("x-pw"), "OOPIF get_dom 内容: {html}");

    // ---- frame 清单诚实报错 ----
    step("虚构 frame 诚实报错带清单");
    let r = session
        .act(
            &WebAction::Inspect {
                selector: "#x-title".into(),
                frame: Some("no-such-frame-xyz".into()),
            },
            None,
        )
        .await;
    match r {
        Err(e) => {
            assert!(e.contains("frame 未找到"), "错误面: {e}");
            assert!(e.contains("no-such-frame-xyz"), "回显 want: {e}");
            assert!(e.contains("127.0.0.1"), "清单带 URL: {e}");
            assert!(e.contains("[cross-process]"), "OOPIF 标注: {e}");
            assert!(e.contains("[same-process]"), "同进程标注: {e}");
        }
        Ok(v) => panic!("虚构 frame 应诚实报错，却返回: {v}"),
    }

    // ---- 导航走开再走回：frame 缓存自愈 ----
    step("导航往返后 frame 自愈");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/text"),
                timeout_ms: Some(10000),
            },
            None,
        )
        .await
        .expect("navigate /text");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/x-host"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate /x-host again");
    let r = act_in_frame_with_retry(
        &session,
        &WebAction::Inspect {
            selector: "#x-title".into(),
            frame: Some(frame_x.clone()),
        },
        30,
    )
    .await
    .expect("OOPIF inspect after nav");
    assert_eq!(
        r["id"],
        json!("x-title"),
        "导航往返后 OOPIF 仍可用（缓存逐出/重解析自愈）"
    );

    // ---- iframe 内 JS 对话框 → route 路径 30s 超时 → 自愈关闭 → 恢复 ----
    // 2026-11-18 交付后自查修正 1 的活体锁：同进程 iframe 的 alert 阻塞渲染
    // 器主线程——resolve_frame（getFrameTree/createIsolatedWorld 不跑 JS）
    // 快速成功，evaluate 撞满 CDP_CMD_TIMEOUT → eval_conn 超时臂
    // dismiss_dialog_if_open 兜底（修正前该臂只覆盖 main 路径，route 内联
    // 调用绕过了它）。iframe 的 URL 子串匹配 /iframe-dialog-inner。
    step("iframe 内对话框 route 路径自愈");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/iframe-dialog-host"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate /iframe-dialog-host");
    // 2s 定时器已触发：弹框开着、页面 JS 暂停。
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let dialog_act = WebAction::Inspect {
        selector: "#d-title".into(),
        frame: Some("/iframe-dialog-inner".into()),
    };
    let r = session.act(&dialog_act, None).await;
    let Err(e) = &r else {
        panic!("对话框阻塞期间 inspect 应报错，却返回: {r:?}");
    };
    assert!(
        e.contains("对话框") && e.contains("已自动关闭"),
        "route 路径超时臂应自愈关框并在错误面注明: {e}"
    );
    // 自愈证明：同一动作再来一次——弹框已被按拒绝关闭，frame 恢复可用。
    let r = session
        .act(&dialog_act, None)
        .await
        .expect("自愈后同 frame inspect 应恢复");
    assert_eq!(r["id"], json!("d-title"), "自愈后 inspect 应命中目标元素");

    // ---- OOPIF 内 JS 对话框 → eval_conn 超时臂（session 路由）→ 自愈 ----
    // 修复 A 原始声称的最后活体面（2026-11-18 四轮复核）：OOPIF 是唯一
    // 「resolve 存活（getTargets/attachToTarget 走 browser 进程）而
    // evaluate 阻塞（对话框在该 iframe 自己的渲染器里）」的路径——
    // eval_conn 超时臂 + dismiss_dialog_if_open 的 session 路由
    // （handleJavaScriptDialog 经 flatten 子会话打给该 target）只有这里能
    // 被真对话框打到；同进程场景 resolve 先死（三轮实测），走不到本臂。
    step("OOPIF 内对话框 route 路径自愈");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/x-dialog-host"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate /x-dialog-host");
    // 2s 定时器已触发：弹框开着、OOPIF 渲染器暂停。
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let x_dialog_act = WebAction::Inspect {
        selector: "#xd-title".into(),
        frame: Some(format!(":{port2}/x-dialog-inner")),
    };
    let r = session.act(&x_dialog_act, None).await;
    let Err(e) = &r else {
        panic!("OOPIF 对话框阻塞期间 inspect 应报错，却返回: {r:?}");
    };
    assert!(
        e.contains("对话框") && e.contains("已自动关闭"),
        "eval_conn session 路由臂应自愈关框并在错误面注明: {e}"
    );
    // 自愈证明：同一动作再来一次——弹框已按拒绝关闭，OOPIF 恢复可用。
    let r = session
        .act(&x_dialog_act, None)
        .await
        .expect("自愈后 OOPIF inspect 应恢复");
    assert_eq!(r["id"], json!("xd-title"), "自愈后 OOPIF inspect 应命中");

    step("close");
    let r = session.close().await.expect("close");
    assert_eq!(r["ok"], json!(true));
    // panic hook 不还原（与 edge_live_full_matrix 同款——测试进程随用例结束
    // 收场；残留 hook 只会对已关闭会话的 pid 空杀，无害）。
    step("全部通过 ✓");
}

/// 六轮复核活体锁（2026-11-18）：基线 4 处容忍型 evaluate 消费点的前置
/// 探针修复——截图视口 metrics / navigate 文档身份锚 / wait_document_ready
/// 轮询 / wait_for 轮询。修复前这四处撞对话框 = 静默假伤（DPR 连坐降级
/// device_px 截图 / same_doc 保守判定 / navigate 假超时 / wait_for 假未
/// 命中）；修复后入口探针毫秒级关框 + 结果面 `dialog_dismissed` 注记，
/// 短轮询把加载期弹框的自愈代价压到 ~3s。墙钟判据 <15s（修复前 30s+
/// 起步）。与既有弹框活体（route 路径）互补：那两个测「错误面注解 +
/// 重试自愈」，本测试测「探针快路径 + 结果面注记 + 不假伤」。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edge_live_dialog_probe_first_read_actions() {
    fn step(name: &str) {
        eprintln!("[probe_live] {name}");
    }
    let _serial = EDGE_LIVE_LOCK.lock().await;
    let (base, _server) = serve_site(None, None).await;
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

    // ---- ① 截图撞框：入口探针关框 → css_px 归一保持 + 注记 ----
    step("截图撞框（/iframe-dialog-inner 直航主 frame）");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/iframe-dialog-inner"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate 弹框页");
    // 2s 定时器已触发：弹框开着。
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let t0 = std::time::Instant::now();
    let r = session
        .act(&WebAction::Screenshot, None)
        .await
        .expect("对话框场景截图应探针自愈成功");
    let elapsed = t0.elapsed();
    assert!(
        elapsed < Duration::from_secs(15),
        "截图撞框应探针快路径而非 30s 阻塞: {elapsed:?}"
    );
    assert_eq!(
        r["dialog_dismissed"],
        json!(true),
        "截图结果应注记关框: {r}"
    );
    assert_eq!(
        r["coordinate_space"],
        json!("css_px"),
        "F32 归一不应连坐降级 device_px: {r}"
    );
    assert!(
        r["viewport"]["width"].as_f64().unwrap_or(0.0) > 0.0,
        "viewport metrics 应读到位: {r}"
    );

    // ---- ② navigate 撞框：锚点探针关框 → F14 闸完整 + 注记 ----
    step("navigate 撞框（文档身份锚前探测）");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/iframe-dialog-inner"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("re-navigate 弹框页");
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let t0 = std::time::Instant::now();
    let r = session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/text"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("对话框场景 navigate 应探针自愈成功");
    assert!(
        t0.elapsed() < Duration::from_secs(15),
        "navigate 撞框应探针快路径: {:?}",
        t0.elapsed()
    );
    assert_eq!(
        r["dialog_dismissed"],
        json!(true),
        "navigate 结果应注记关框: {r}"
    );
    assert_eq!(
        r["load_state"],
        json!("complete"),
        "F14 加载闸不应因锚点探针退化: {r}"
    );

    // ---- ③ wait_for 撞框（弹框先于等待）：入口探针 → 立即命中 + 注记 ----
    step("wait_for 撞框（弹框先于等待）");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/iframe-dialog-inner"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("re-navigate 弹框页");
    tokio::time::sleep(Duration::from_millis(3000)).await;
    let t0 = std::time::Instant::now();
    let r = session
        .act(
            &WebAction::WaitFor {
                selector: "#d-title".into(),
                timeout_ms: Some(10000),
            },
            None,
        )
        .await
        .expect("对话框场景 wait_for 应探针自愈成功");
    assert!(
        t0.elapsed() < Duration::from_secs(15),
        "wait_for 撞框应探针快路径: {:?}",
        t0.elapsed()
    );
    assert_eq!(r["found"], json!(true), "{r}");
    assert_eq!(
        r["dialog_dismissed"],
        json!(true),
        "wait_for 结果应注记关框: {r}"
    );

    // ---- ④ wait_for 等待中途弹框：短轮询自愈 + deadline 顺延 → 命中 ----
    step("wait_for 等待中途弹框（元素后到）");
    session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/dialog-wait"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("navigate dialog-wait");
    let t0 = std::time::Instant::now();
    let r = session
        .act(
            &WebAction::WaitFor {
                selector: "#late-btn".into(),
                timeout_ms: Some(10000),
            },
            None,
        )
        .await
        .expect("中途弹框 wait_for 应自愈命中");
    let elapsed = t0.elapsed();
    assert_eq!(r["found"], json!(true), "中途弹框不应假未命中: {r}");
    assert_eq!(
        r["dialog_dismissed"],
        json!(true),
        "中途弹框应注记关框: {r}"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "短轮询自愈应 ~3-4s 而非 30s+: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(2500),
        "元素 2.5s 后才出现（overdue 定时器在关框后补跑），命中不该早于此: {elapsed:?}"
    );

    // ---- ⑤ navigate 加载期弹框（解析阻塞）：轮询自愈 → complete ----
    step("navigate 加载期弹框（解析期 alert 阻塞 parser）");
    let t0 = std::time::Instant::now();
    let r = session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/dialog-parse"),
                timeout_ms: Some(15000),
            },
            None,
        )
        .await
        .expect("解析期弹框 navigate 应轮询自愈完成");
    let elapsed = t0.elapsed();
    assert_eq!(
        r["load_state"],
        json!("complete"),
        "解析期弹框不应假超时: {r}"
    );
    assert_eq!(r["dialog_dismissed"], json!(true), "加载期关框应注记: {r}");
    assert!(
        elapsed < Duration::from_secs(15),
        "短轮询自愈应 ~3s 而非 30s+: {elapsed:?}"
    );

    step("close");
    let r = session.close().await.expect("close");
    assert_eq!(r["ok"], json!(true));
    // panic hook 不还原（同款纪律——残留 hook 只会对已关闭会话的 pid 空杀）。
    step("全部通过 ✓");
}

// ---------------------------------------------------------------------------
// 会话级四动作 · 所有权闭环（2026-11-28 批）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn edge_live_ownership_closed_loop() {
    fn step(name: &str) {
        eprintln!("[own_live] {name}");
    }
    let _serial = EDGE_LIVE_LOCK.lock().await;
    step("本地站点启动");
    let (base, _server) = serve_site(None, None).await;
    let tmp = tempfile::tempdir().unwrap();
    let manager = BrowserManager::new(
        tmp.path().to_path_buf(),
        Box::new(BrowserPrefs::default),
        None,
    );

    step("sess-A spawn（bot 面）");
    // 先验 Edge 在场（诚实 SKIP 判据与全矩阵同源）。
    let edge = match find_edge() {
        Some(_) => true,
        None => {
            eprintln!("SKIP：本机未检测到 Edge——所有权闭环无法跑");
            return;
        }
    };
    let _ = edge;
    let session = match manager.spawn(Some("edge"), None, Some("sess-A")).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP：Edge spawn 失败（{e}）——SAC 拦截/端口受限等环境原因");
            return;
        }
    };
    let id = session.id.clone();
    // panic 兜底树杀（同全矩阵纪律）。prev_hook 用 Arc 共享——本测试收尾
    // 要还原默认 hook（后续活体测试不受本 pid 的树杀 hook 污染）。
    let spawned_pid = session.pid;
    let prev_hook: std::sync::Arc<dyn Fn(&std::panic::PanicHookInfo) + Send + Sync> =
        std::panic::take_hook().into();
    let hook_for_panic = prev_hook.clone();
    std::panic::set_hook(Box::new(move |info| {
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &spawned_pid.to_string()])
            .output();
        #[cfg(not(windows))]
        let _ = std::process::Command::new("kill")
            .args(["-9", &spawned_pid.to_string()])
            .output();
        hook_for_panic(info);
    }));

    step("实例台账 + owner 投影");
    assert_eq!(manager.owner_of(&id).as_deref(), Some("sess-A"));
    let snap = manager.sessions_snapshot();
    assert_eq!(snap[0]["id"], json!(id));
    assert_eq!(snap[0]["owner"], json!("sess-A"));

    step("初始 tab 全部绑 sess-A（spawn 时绑定）");
    let tabs = session.tabs_snapshot().await.expect("tabs_snapshot");
    assert!(!tabs.is_empty(), "spawn 后至少一个 tab");
    let first = tabs[0]["target_id"].as_str().unwrap().to_string();
    for t in &tabs {
        assert_eq!(t["owner"], json!("sess-A"), "初始 tab 未绑定: {t}");
    }

    step("last-tab 守卫：唯一 tab 拒 close_tab");
    let err = session.close_tab(&first, Some("sess-A")).await.unwrap_err();
    assert!(err.contains("唯一标签页"), "{err}");
    assert!(err.contains("close_browser"), "{err}");

    step("act 归因（堵洞②）：A 点 target=_blank → 新 tab 记 sess-A");
    session.set_working_tab(&first).await.expect("set_working");
    let r = session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/linkout"),
                timeout_ms: Some(15_000),
            },
            Some("sess-A"),
        )
        .await
        .expect("navigate /linkout");
    assert_eq!(r["ok"], json!(true));
    let r = session
        .act(
            // trusted 坐标点击（合成 el.click() 无 user activation，target=
            // _blank 被弹窗拦截器吞——活体实证）。锚点固定绝对位 (40,40)
            // 200x40 块，(100,60) 稳定命中。mouseMoved 暖场吸收首事件丢弃。
            &WebAction::Click {
                selector: None,
                x: Some(100.0),
                y: Some(60.0),
            },
            Some("sess-A"),
        )
        .await
        .expect("trusted click 链接块");
    assert_eq!(r["ok"], json!(true));
    // 新 tab 异步出现 → 轮询（归因在 act 的收敛窗口内完成）。
    let mut second = String::new();
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let tabs = session.tabs_snapshot().await.unwrap();
        if tabs.len() >= 2 {
            second = tabs
                .iter()
                .find(|t| t["target_id"].as_str() != Some(first.as_str()))
                .and_then(|t| t["target_id"].as_str().map(str::to_string))
                .unwrap_or_default();
            if !second.is_empty() {
                break;
            }
        }
    }
    assert!(!second.is_empty(), "点 target=_blank 后未出现第二个 tab");
    let tabs = session.tabs_snapshot().await.unwrap();
    for t in &tabs {
        assert_eq!(t["owner"], json!("sess-A"), "act 归因失败: {t}");
    }

    step("close_tab 匹配三态：未找到 / 多命中");
    let err = session
        .close_tab("no-such-page-xyz", Some("sess-A"))
        .await
        .unwrap_err();
    assert!(err.contains("未找到匹配"), "{err}");
    assert!(err.contains("先用 tabs 动作查看"), "{err}");
    let err = session
        .close_tab("127.0.0.1", Some("sess-A"))
        .await
        .unwrap_err();
    assert!(err.contains("命中 2 个"), "{err}");

    step("跨属主拒绝：B 关 A 的 tab");
    let err = session
        .close_tab("/linkout", Some("sess-B"))
        .await
        .unwrap_err();
    assert!(err.contains("sess-A") && err.contains("无权关闭"), "{err}");

    step("close_browser 双闸①：B 关 A 的实例");
    let err = manager
        .close_session(&id, Some("sess-B"))
        .await
        .unwrap_err();
    assert!(err.contains("只关自己开的"), "{err}");
    assert!(manager.session(&id).is_some(), "拒关后实例必须在注册表");

    step("close_browser 双闸②：B 在 A 实例里留 tab → A 拒关（连坐闸）");
    session
        .set_working_tab(&second)
        .await
        .expect("set_working 2");
    let r = session
        .act(
            &WebAction::Navigate {
                url: format!("{base}/linkout"),
                timeout_ms: Some(15_000),
            },
            Some("sess-B"),
        )
        .await
        .expect("navigate 2 /linkout");
    assert_eq!(r["ok"], json!(true));
    let r = session
        .act(
            &WebAction::Click {
                selector: None,
                x: Some(100.0),
                y: Some(60.0),
            },
            Some("sess-B"),
        )
        .await
        .expect("trusted click 链接块 as B");
    assert_eq!(r["ok"], json!(true));
    // B 归因的新 tab（owner=sess-B）出现 → 轮询。
    let mut third = String::new();
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let tabs = session.tabs_snapshot().await.unwrap();
        if let Some(t) = tabs.iter().find(|t| t["owner"].as_str() == Some("sess-B")) {
            third = t["target_id"].as_str().unwrap().to_string();
            break;
        }
    }
    assert!(!third.is_empty(), "B 点开后未出现 sess-B 名下 tab");
    let err = manager
        .close_session(&id, Some("sess-A"))
        .await
        .unwrap_err();
    assert!(err.contains("非本会话的标签页"), "{err}");
    assert!(err.contains("sess-B"), "异己清单必须带 owner: {err}");
    assert!(manager.session(&id).is_some(), "连坐闸拒后实例必须在注册表");

    step("B 关自己的 tab（唯一命中 + working_cleared）");
    session
        .set_working_tab(&third)
        .await
        .expect("set_working 3");
    let r = session
        .close_tab(&third, Some("sess-B"))
        .await
        .expect("B close own tab");
    assert_eq!(r["closed"]["target_id"], json!(third));
    assert_eq!(r["working_cleared"], json!(true));
    assert_eq!(session.working_tab_id(), None, "工作指针必须解除");

    step("A 收尾 close_browser（全 tab 归 A，双闸全过）");
    let r = manager
        .close_session(&id, Some("sess-A"))
        .await
        .expect("A close_browser");
    assert_eq!(r["ok"], json!(true));
    assert!(manager.session(&id).is_none(), "关闭后摘出注册表");
    assert_eq!(manager.owner_of(&id), None, "关闭后摘出台账");

    step("人面哨兵：human spawn / B 拒 / None 免检");
    let s2 = match manager.spawn(Some("edge"), None, None).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP：第二实例 spawn 失败（{e}）——人面臂无法跑");
            return;
        }
    };
    let id2 = s2.id.clone();
    assert_eq!(manager.owner_of(&id2).as_deref(), Some("human"));
    let tabs2 = s2.tabs_snapshot().await.expect("tabs 2");
    let first2 = tabs2[0]["target_id"].as_str().unwrap().to_string();
    for t in &tabs2 {
        assert_eq!(t["owner"], json!("human"), "human 初始 tab: {t}");
    }
    // B 关 human 的 tab → 拒（human 哨兵 ≠ 任何会话键）。
    let err = s2.close_tab(&first2, Some("sess-B")).await.unwrap_err();
    assert!(err.contains("human") && err.contains("无权关闭"), "{err}");
    // B 关 human 的实例 → 双闸① 拒。
    let err = manager
        .close_session(&id2, Some("sess-B"))
        .await
        .unwrap_err();
    assert!(err.contains("只关自己开的"), "{err}");
    // 人面 None 免检（Dashboard 关闭语义不变）→ 收尾清理。
    let r = manager
        .close_session(&id2, None)
        .await
        .expect("human close");
    assert_eq!(r["ok"], json!(true));
    assert!(manager.session(&id2).is_none());

    // 还原：set_hook 只收 Box——用新 Box 包一层 Arc 克隆（原 hook 行为不变）。
    let restore = prev_hook.clone();
    std::panic::set_hook(Box::new(move |info| restore(info)));
    step("全部通过 ✓");
}
