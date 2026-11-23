//! Driver 抽象与 v1 Chromium CDP 实现（计划 D4/D5/Q3）。
//!
//! `BrowserDriver` trait 动作语义按「网页操作」描述（navigate/click/type/
//! key/scroll/extract_text/screenshot/wait_for），不做 CDP 特有假设——未来
//! FirefoxBiDiDriver（二期 P4，WebDriver BiDi）只加实现不返工接口。
//!
//! `ChromiumCdpDriver`（v1，Edge/Chrome/Brave/Vivaldi/Chromium 同构）：
//! - 页面级常驻 WS 连接按 target 缓存（多路复用，见 [`crate::cdp::CdpConn`]）；
//! - 选择器动作走 Runtime.evaluate JS（native setter + input/change——
//!   React/Vue 受控组件兼容，guest 版实证形态移植）；
//! - 坐标点击 / 滚轮 / 键盘走 `Input.dispatchMouseEvent|dispatchKeyEvent`
//!   （**trusted 事件**——游戏反作弊认这个，纯 JS 合成事件会被判不可信）；
//! - `run_script` 不进 trait（会话编排层逐步展开，计划 D12）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cdp::{self, CdpConn};
pub use crate::detect::DriverKind;

/// 等待类型复用（guest 同名常量语义）。
pub const NAV_WAIT_DEFAULT_MS: u64 = 30_000;
/// navigate 加载等待墙钟上限（F18：模型传超大 timeout_ms 时 act 会挂到
/// 页面加载完或墙钟耗尽——与 wait_for 同语义钉上限）。
pub const NAV_WAIT_MAX_MS: u64 = 120_000;
pub const WAIT_FOR_DEFAULT_MS: u64 = 10_000;
pub const WAIT_FOR_MAX_MS: u64 = 60_000;
/// 就绪轮询类 evaluate 的短超时（六轮复核 2026-11-18）：JS 对话框开着时
/// 轮询 evaluate 撞满超时——30s 的 [`cdp::CDP_CMD_TIMEOUT`] 把每次弹框
/// 阻塞代价放大到 30s+（wait_document_ready 轮询被它拖死 = navigate
/// 假超时；wait_for 假未命中）。轮询语义下空闲主线程的 readyState
/// evaluate 毫秒级，2s 已足够；配合 eval_conn 超时臂自愈，弹框阻塞代价
/// ≈ 2s 超时 + 毫秒级探测。一次性读取仍用 30s。
pub const POLL_CDP_TIMEOUT: Duration = Duration::from_secs(2);
/// extract 文本截断（head+tail，guest 同款 4000+4000）。
const EXTRACT_HEAD: usize = 4000;
const EXTRACT_TAIL: usize = 4000;

pub use crate::cdp::TargetInfo as TabInfo;

// ---------------------------------------------------------------------------
// 能力位与动作
// ---------------------------------------------------------------------------

/// Driver 能力位（prompt 层按能力诚实暴露工具面）。
#[derive(Debug, Clone, Copy)]
pub struct DriverCaps {
    /// trusted 输入事件（坐标点击/键盘）。
    pub trusted_input: bool,
    /// 页面截图。
    pub screenshot: bool,
    /// 事件订阅（Page.loadEventFired 等）。
    pub events: bool,
}

/// v1 单动作（`run_script` 是编排层概念，不在此处）。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WebAction {
    /// 导航到 URL；timeout_ms = 加载等待墙钟（超时诚实报 load_state）。
    Navigate {
        url: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// 点击。selector（DOM 路径）与坐标（trusted 输入，canvas 游戏用）二选一。
    Click {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
    },
    /// 输入文本到选择器元素（native setter；凭据别名在编排层守卫后传入
    /// 明文，trait 不感知凭据）。
    Type { selector: String, text: String },
    /// 键盘事件（WASD down/hold/up、Enter/Esc 等，trusted 输入）。可选
    /// selector = 先 `el.focus()` 承接焦点再派发（trusted 键事件需要页面
    /// 有焦点元素；无焦点整包被渲染器**静默丢弃**——Edge 实证 keydown+
    /// keypress 全无，只剩 keyup）。省略 selector 时焦点依赖前置 trusted
    /// 点击（合成 click 不聚焦）。
    Key {
        key: String,
        #[serde(default)]
        phase: KeyPhase,
        #[serde(default)]
        hold_ms: Option<u64>,
        #[serde(default)]
        selector: Option<String>,
    },
    /// 滚动（trusted wheel；x/y 缺省 = 视口中心）。dx/dy 缺省 = 0（工具
    /// schema required 只有 action——模型合法省略任一方向，不可拒读；
    /// 省略 dy = 只横滚、省略 dx = 只纵滚。serde default 与 schema 对齐：
    /// 必填无标注会被「missing field」拒绝白烧一轮——2026-11-15 复审 F8）。
    Scroll {
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
    },
    /// 提取页面文本（选择器缺省 = 全页 body）。**硬规则：不回读输入框
    /// .value**（凭据打字后 extract 是现实路径，DOM value 有明文）。
    ExtractText {
        #[serde(default)]
        selector: Option<String>,
    },
    /// 页面截图（PNG）。
    Screenshot,
    /// 等待选择器元素出现。
    WaitFor {
        selector: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// DOM 元素结构化检视（只读，2026-11-18 DOM 只读动作计划）。状态字段
    /// 读 property（open/checked/disabled/required——用户点击后 attribute
    /// 不更新，读 attribute 会「勾了但显示没勾」）；expanded/role/name/type
    /// 读 attribute；属性白名单输出（`value` 恒不在名单——凭据面硬规则）。
    Inspect {
        selector: String,
        /// frame 定位（URL 子串匹配；缺省/"main" = 主 frame；同进程 iframe
        /// 走 createIsolatedWorld、跨域 OOPIF 走 flatten 子会话——§3.7）。
        #[serde(default)]
        frame: Option<String>,
    },
    /// 有界 DOM 树快照（只读）。text 只给叶子节点（容器 textContent 是
    /// 后代拼接会与子行重复）；shadowRoot 递归穿透；不进 iframe
    /// contentDocument（iframe 元素本身出现在树里，src 可见——下一步用
    /// frame 参数定位内部）。
    #[serde(rename = "snapshot")]
    SnapshotDom {
        /// 遍历根（缺省 body；只主 frame 或 frame 指定的 document）。
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        frame: Option<String>,
        /// 深度闸（缺省 12，钳 [1,30]）。
        #[serde(default)]
        max_depth: Option<u32>,
        /// 节点数闸（缺省 300，钳 [1,2000]）。
        #[serde(default)]
        max_nodes: Option<u32>,
    },
    /// 子树 outerHTML（只读，脱敏序列化）：clone 后剥 input value 属性、
    /// 清 textarea textContent、后代 script/style 整体删除。打字值
    /// 走 native setter 设 property 天然不进 outerHTML——脱敏剥的是页面
    /// 源码 value 属性（SSO 预填场景），两层各自有效。
    GetDom {
        selector: String,
        #[serde(default)]
        frame: Option<String>,
        /// 输出字符上限（缺省 12000，钳 [200,50000]；head+tail 截断）。
        #[serde(default)]
        max_chars: Option<u32>,
    },
}

/// 键盘事件相位。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPhase {
    /// 按下+抬起（hold_ms 可拉长按住时长）。
    #[default]
    Press,
    /// 仅按下（游戏按住 W 场景；up 之前持续生效）。
    Down,
    /// 仅抬起。
    Up,
}

/// 动作风险投影 slug（编排层 per-step 安全管线的 `override_op_type` 数据源；
/// run_script 本体 force 卡在 nemesisbot 工具侧组装）。
pub fn action_op_slug(action: &WebAction) -> &'static str {
    match action {
        WebAction::Navigate { .. } => "network_request",
        WebAction::Click { .. } => "network_upload",
        WebAction::Type { .. } => "network_upload",
        WebAction::Key { .. } => "network_upload",
        WebAction::Scroll { .. } => "network_upload",
        WebAction::ExtractText { .. } => "internal",
        WebAction::Screenshot => "internal",
        WebAction::WaitFor { .. } => "internal",
        // DOM 只读三动作与 extract_text 同级（免审批卡；审计照常——
        // 2026-11-18 DOM 只读动作计划 §3.5）。
        WebAction::Inspect { .. } => "internal",
        WebAction::SnapshotDom { .. } => "internal",
        WebAction::GetDom { .. } => "internal",
    }
}

// ---------------------------------------------------------------------------
// Driver trait
// ---------------------------------------------------------------------------

/// 浏览器驱动接口（D4：接口一致、底层不同）。
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    fn kind(&self) -> DriverKind;
    fn capabilities(&self) -> DriverCaps;

    /// tab 列表全量快照（便宜，是地图——D8）。
    async fn list_tabs(&self) -> Result<Vec<TabInfo>, String>;

    /// 开新 tab（url 缺省 = about:blank），返回其信息。
    async fn open_tab(&self, url: Option<&str>) -> Result<TabInfo, String>;

    /// 关 tab。
    async fn close_tab(&self, target_id: &str) -> Result<(), String>;

    /// 激活 tab（浏览器前置显示）。
    async fn activate_tab(&self, target_id: &str) -> Result<(), String>;

    /// 在指定 tab 上执行单个网页动作，返回结果 JSON（恒含 ok=true）。
    async fn act(&self, target_id: &str, action: &WebAction) -> Result<Value, String>;

    /// 当前 tab 的 location.origin（凭据守卫①数据源）。
    async fn tab_origin(&self, target_id: &str) -> Result<String, String>;

    /// 目标元素是否密码型输入框（凭据守卫②；元素缺失/非密码框 = false）。
    async fn is_password_input(&self, target_id: &str, selector: &str) -> Result<bool, String>;

    /// tab 加载态（readyState；工作 tab 徽标数据源——未附着 tab 返回
    /// Err，调用方诚实降级为「未附着」）。
    async fn tab_ready_state(&self, target_id: &str) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// ChromiumCdpDriver（v1）
// ---------------------------------------------------------------------------

/// Chromium 家族 CDP 驱动。
pub struct ChromiumCdpDriver {
    port: u16,
    kind: DriverKind,
    /// 浏览器级 WS path（DevToolsActivePort 次行；tab 管理/Browser.close 用）。
    browser_ws_path: String,
    /// 页面级连接缓存（target_id → conn）。
    conns: Mutex<HashMap<String, Arc<CdpConn>>>,
    browser_conn: tokio::sync::OnceCell<Arc<CdpConn>>,
    /// 同进程 iframe 隔离 world 缓存（frame_id → executionContextId；
    /// per-call 创建会积累垃圾 world，复用 + 页面导航自动清）。
    frame_worlds: Mutex<HashMap<String, u64>>,
    /// OOPIF attach 会话缓存（target_id → sessionId；重复 attachToTarget
    /// 同一 target 会积累会话）。失效靠执行期报错逐出自愈（§3.7）。
    frame_sessions: Mutex<HashMap<String, String>>,
}

impl ChromiumCdpDriver {
    pub fn new(port: u16, kind: DriverKind, browser_ws_path: String) -> Self {
        Self {
            port,
            kind,
            browser_ws_path,
            conns: Mutex::new(HashMap::new()),
            browser_conn: tokio::sync::OnceCell::const_new(),
            frame_worlds: Mutex::new(HashMap::new()),
            frame_sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// 页面级常驻连接（缺席才连；连上即 Page.enable + Runtime.enable）。
    async fn page_conn(&self, target_id: &str) -> Result<Arc<CdpConn>, String> {
        let existing = self.conns.lock().get(target_id).cloned();
        if let Some(conn) = existing {
            return Ok(conn);
        }
        let conn = CdpConn::connect(self.port, &format!("/devtools/page/{target_id}")).await?;
        // enable 域失败不致命（部分页面禁调试 = 命令诚实失败）。
        let _ = conn
            .call("Page.enable", json!({}), cdp::CDP_CMD_TIMEOUT)
            .await;
        let _ = conn
            .call("Runtime.enable", json!({}), cdp::CDP_CMD_TIMEOUT)
            .await;
        self.conns
            .lock()
            .insert(target_id.to_string(), conn.clone());
        Ok(conn)
    }

    /// 摘除页面级连接缓存（失败兜底：连接死亡——tab 手关/崩溃/半开——后
    /// 缓存 conn 残留会让后续每次 act 撞满 30s 超时（send 无响应）；摘除后
    /// 下次 act 重连，tab 仍在则重连成功。session 层失败臂调用，F13）。
    pub fn evict_conn(&self, target_id: &str) {
        self.conns.lock().remove(target_id);
    }

    /// 只保留存活清单内的页面级连接缓存（F31，2026-11-15 五审：死连接 GC
    /// ——用户手关 tab 是唯一关闭路径（九动作工具面无 close_tab），其缓存
    /// conn 永不触发 act 失败摘除（tab 没了就不会再 act），长寿命会话反复
    /// 开关 tab = 死 socket/读任务堆积。GC 点选 tabs 快照心跳；摘除按设计
    /// 无害：摘后首次 act/attach 自动重连几 ms（evict_conn 同语义）。
    pub fn retain_conns(&self, live: &[String]) {
        self.conns
            .lock()
            .retain(|k, _| live.iter().any(|id| id == k));
    }

    /// 确保页面级连接在场（attach 前置：建立连接 + enable——load_state
    /// 徽标随即真实可问、后续命令就绪；不激活窗口）。失败不阻塞点名语义
    /// 由调用方决定（session 层 warn 继续——tab 在列表但连接失败的页面
    /// 照常可点名，首次 act 诚实报错）。
    pub async fn ensure_attached(&self, target_id: &str) -> Result<(), String> {
        self.page_conn(target_id).await.map(|_| ())
    }

    /// 浏览器级连接（懒建；tab 管理 / Browser.close）。
    async fn browser(&self) -> Result<Arc<CdpConn>, String> {
        self.browser_conn
            .get_or_try_init(|| async {
                let path = if self.browser_ws_path.is_empty() {
                    // DevToolsActivePort 次行缺席（罕见）：按 /json/version
                    // 的 webSocketDebuggerUrl 兜底。
                    let v = cdp::http_version(self.port).await?;
                    v.get("webSocketDebuggerUrl")
                        .and_then(|u| u.as_str())
                        .and_then(|u| u.split_once("/devtools/browser/").map(|(_, p)| p))
                        .map(|p| format!("/devtools/browser/{p}"))
                        .ok_or("浏览器级 WS path 未知（DevToolsActivePort 次行缺席且 /json/version 无 webSocketDebuggerUrl）")?
                } else {
                    self.browser_ws_path.clone()
                };
                CdpConn::connect(self.port, &path).await
            })
            .await
            .cloned()
    }

    /// Runtime.evaluate（returnByValue；exceptionDetails → Err）。
    async fn evaluate(&self, target_id: &str, expr: &str) -> Result<Value, String> {
        self.evaluate_to(target_id, expr, cdp::CDP_CMD_TIMEOUT)
            .await
    }

    /// [`Self::evaluate`] 的显式超时变体（就绪轮询用 [`POLL_CDP_TIMEOUT`]）。
    async fn evaluate_to(
        &self,
        target_id: &str,
        expr: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        let conn = self.page_conn(target_id).await?;
        self.eval_conn(&conn, None, None, expr, None, timeout).await
    }

    /// 核心 evaluate（conn + 可选 flatten 子会话/隔离 world 通路）——**全部**
    /// evaluate 的单一通路（主 frame / 同进程 iframe contextId / OOPIF 子会话
    /// 都走这里，对话框自愈臂才有全路由覆盖；2026-11-18 自查修正：此前
    /// SameProcess/Oopif 分支内联 call 绕过本函数，iframe 里开着 JS 对话框时
    /// DOM 读 evaluate 撞满 30s 超时且无 F33 探测）。returnByValue、
    /// exceptionDetails → Err、超时臂 F33/F39 对话框自愈（四轮复核活体修正：
    /// session 路由的 handleJavaScriptDialog 对 OOPIF frame target **失败**
    /// ——JS 对话框是 tab 级、挂 WebContents 的 DialogManager，frame target
    /// 的 Page 域不带对话框状态；`dismiss_fallback` 传页面 conn 时以
    /// page conn + None 兜底，这才是实测有效的关框通路）。既有页面路径
    /// [`Self::evaluate`] 是本函数的 `(page_conn, None, None)` 特例。
    /// `timeout` 由调用方给定：一次性读取用 [`cdp::CDP_CMD_TIMEOUT`]，
    /// 就绪轮询用 [`POLL_CDP_TIMEOUT`]（六轮复核：对话框开着时轮询撞满
    /// 超时，30s 的轮询超时把每次弹框阻塞代价放大到 30s+）。
    async fn eval_conn(
        &self,
        conn: &CdpConn,
        session: Option<&str>,
        context_id: Option<u64>,
        expr: &str,
        dismiss_fallback: Option<&CdpConn>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let mut params = json!({"expression": expr, "returnByValue": true, "awaitPromise": false});
        if let Some(ctx) = context_id {
            params["contextId"] = json!(ctx);
        }
        let res = match conn
            .call_in_session(session, "Runtime.evaluate", params, timeout)
            .await
        {
            Ok(v) => v,
            // F33（2026-11-15 六审）：evaluate 超时最常见的可恢复根因 =
            // JS 对话框（alert/confirm/prompt）暂停了页面 JS——不处理则
            // 每个 evaluate 都撞满 30s（wait_document_ready/wait_for 轮询
            // = 超时风暴）。自动关闭 + 诚实回报（轮询调用下一轮自愈）。
            Err(e) if e.contains("timeout") => {
                let dismissed = dismiss_dialog_if_open(conn, session).await
                    || match dismiss_fallback {
                        // 四轮复核：OOPIF frame target 的 session 路由
                        // dismiss 失败（对话框 tab 级）→ 页面会话兜底。
                        Some(p) => dismiss_dialog_if_open(p, None).await,
                        None => false,
                    };
                if dismissed {
                    // F39（2026-11-16 七审）：不带「可重试」的盲目鼓励——
                    // 若对话框由本动作的 JS 处理器**同步**弹出（el.click()
                    // → handler → alert），evaluate 被暂停前副作用已发生，
                    // 无脑重试 = 二次点击/二次提交。文案按「轮询自愈 +
                    // 副作用核验」双语义写，模型自行判断。
                    return Err(format!(
                        "{e}；检测到阻塞页面的 JS 对话框，已自动关闭（按拒绝处理）。\
                         轮询类调用重试即可自愈；若此动作本身带副作用（点击/输入/提交），\
                         副作用可能已在弹框前发生，重试前请先核实页面状态"
                    ));
                }
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        eval_result_value(res, expr)
    }

    /// 在 FrameRoute 上执行 evaluate（三路全走 [`Self::eval_conn`]——主
    /// frame / 同进程隔离 world / OOPIF 子会话同一对话框自愈臂）。
    async fn evaluate_routed(
        &self,
        target_id: &str,
        route: &FrameRoute,
        expr: &str,
    ) -> Result<Value, String> {
        match route {
            FrameRoute::Main => {
                let conn = self.page_conn(target_id).await?;
                self.eval_conn(&conn, None, None, expr, None, cdp::CDP_CMD_TIMEOUT)
                    .await
            }
            FrameRoute::SameProcess { context_id, .. } => {
                let conn = self.page_conn(target_id).await?;
                self.eval_conn(
                    &conn,
                    None,
                    Some(*context_id),
                    expr,
                    None,
                    cdp::CDP_CMD_TIMEOUT,
                )
                .await
            }
            FrameRoute::Oopif { session_id, .. } => {
                let browser = self.browser().await?;
                // dismiss 兜底 = 页面会话（对话框 tab 级；OOPIF frame
                // target 的 session 路由关不掉——四轮复核活体实测）。
                let page = self.page_conn(target_id).await?;
                self.eval_conn(
                    &browser,
                    Some(session_id),
                    None,
                    expr,
                    Some(&page),
                    cdp::CDP_CMD_TIMEOUT,
                )
                .await
            }
        }
    }

    /// DOM 只读三动作统一执行通路：frame 解析 → evaluate；context/target
    /// 失效错误逐出对应缓存 → 重新解析 → 重试一次（§3.7 自愈——页面导航
    /// 后隔离 world/OOPIF 换代是常态）。返回 (evaluate 结果, frame 回显)。
    async fn eval_dom_routed(
        &self,
        target_id: &str,
        frame: Option<&str>,
        expr: &str,
    ) -> Result<(Value, String), String> {
        let want = frame.filter(|f| !f.is_empty() && *f != "main");
        let (route, label) = match want {
            None => (self.resolve_main(target_id).await?, "main".to_string()),
            Some(w) => {
                let r = self.resolve_frame(target_id, w).await?;
                let label = w.to_string();
                (r, label)
            }
        };
        match self.evaluate_routed(target_id, &route, expr).await {
            Ok(v) => Ok((v, label)),
            Err(e) if is_stale_frame_error(&e) => {
                // 逐出该 route 的缓存条目 → 重新解析 → 重试一次。
                match &route {
                    FrameRoute::SameProcess { frame_id, .. } => {
                        self.frame_worlds.lock().remove(frame_id);
                    }
                    FrameRoute::Oopif {
                        oopif_target_id, ..
                    } => {
                        self.frame_sessions.lock().remove(oopif_target_id);
                    }
                    FrameRoute::Main => {}
                }
                let fresh = match want {
                    None => self.resolve_main(target_id).await?,
                    Some(w) => self.resolve_frame(target_id, w).await?,
                };
                let v = self.evaluate_routed(target_id, &fresh, expr).await?;
                Ok((v, label))
            }
            Err(e) => Err(e),
        }
    }

    /// 主 frame 通路（页面级 conn 现成）。
    async fn resolve_main(&self, target_id: &str) -> Result<FrameRoute, String> {
        self.page_conn(target_id).await?;
        Ok(FrameRoute::Main)
    }

    /// frame 参数解析（§3.7 双路径，分流顺序是正确性关键）：先查 OOPIF
    /// targets（frame 树是 DOM 概念与进程无关——OOPIF frame 也在树里，
    /// 先走 frameTree 会把 OOPIF frameId 误导入 createIsolatedWorld，拿
    /// 到跨进程 contextId 后 page session 的 evaluate 必报 context not
    /// found）；targets 无命中再走同进程 frame 树；都不中 = 诚实报错 +
    /// 全 frame 清单（发现机制，不另设 list_frames 动作）。
    async fn resolve_frame(&self, target_id: &str, want: &str) -> Result<FrameRoute, String> {
        // 1) OOPIF targets（浏览器级 conn；HTTP /json 不列 iframe target）。
        let browser = self.browser().await?;
        let targets_res = browser
            .call("Target.getTargets", json!({}), cdp::CDP_CMD_TIMEOUT)
            .await?;
        if let Some(list) = targets_res.get("targetInfos").and_then(|v| v.as_array()) {
            for t in list {
                if t.get("type").and_then(|v| v.as_str()) != Some("iframe") {
                    continue;
                }
                let url = t.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if !url.contains(want) {
                    continue;
                }
                let tid = t
                    .get("targetId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if tid.is_empty() {
                    continue;
                }
                let sid = self.oopif_session(&tid).await?;
                return Ok(FrameRoute::Oopif {
                    session_id: sid,
                    oopif_target_id: tid,
                });
            }
        }
        // 2) 同进程 frame 树（页面级 conn；渲染器绑定——对话框兜底见
        // [`call_with_dialog_arm`]）。
        let page = self.page_conn(target_id).await?;
        let tree_res = call_with_dialog_arm(&page, "Page.getFrameTree", json!({})).await?;
        // 响应解包：call() 返回 result 载荷，顶层是 `frameTree` 键——纯函数
        // 消费的是 frameTree **节点**（`{frame, childFrames}`）。不解包则
        // flatten 首层即 return，清单恒「（无）」/查找恒未命中（edge_live
        // 实测抓出——lib 测试只钉了节点形态，包络形态只有活体能验）。
        let tree = tree_res.get("frameTree").cloned().unwrap_or(Value::Null);
        if let Some(frame_id) = find_frame_id(&tree, want) {
            let context_id = self.frame_world(&page, &frame_id).await?;
            return Ok(FrameRoute::SameProcess {
                frame_id,
                context_id,
            });
        }
        // 3) 都不中 → 报错 + 清单（标注进程归属）。
        Err(format!(
            "frame 未找到（按 URL 子串匹配「{want}」）。当前可见 frame：{}",
            frame_url_list(&tree, &targets_res)
        ))
    }

    /// 同进程 iframe 隔离 world（缓存复用；世界数量随页面导航自动清）。
    /// createIsolatedWorld 同为渲染器绑定调用——对话框兜底同
    /// [`call_with_dialog_arm`]（缓存命中路径不发起 CDP 调用，天然免疫）。
    async fn frame_world(&self, page: &Arc<CdpConn>, frame_id: &str) -> Result<u64, String> {
        if let Some(ctx) = self.frame_worlds.lock().get(frame_id) {
            return Ok(*ctx);
        }
        let res = call_with_dialog_arm(
            page,
            "Page.createIsolatedWorld",
            json!({"frameId": frame_id}),
        )
        .await?;
        let ctx = res
            .get("executionContextId")
            .and_then(|v| v.as_u64())
            .ok_or("Page.createIsolatedWorld 缺 executionContextId")?;
        self.frame_worlds.lock().insert(frame_id.to_string(), ctx);
        Ok(ctx)
    }

    /// OOPIF attach 会话（缓存复用；flatten 模式同一 browser socket 多路复用）。
    async fn oopif_session(&self, oopif_target_id: &str) -> Result<String, String> {
        if let Some(sid) = self.frame_sessions.lock().get(oopif_target_id) {
            return Ok(sid.clone());
        }
        let browser = self.browser().await?;
        let res = browser
            .call(
                "Target.attachToTarget",
                json!({"targetId": oopif_target_id, "flatten": true}),
                cdp::CDP_CMD_TIMEOUT,
            )
            .await?;
        let sid = res
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or("Target.attachToTarget 缺 sessionId")?
            .to_string();
        self.frame_sessions
            .lock()
            .insert(oopif_target_id.to_string(), sid.clone());
        Ok(sid)
    }

    /// evaluate 结果字符串值。
    async fn eval_string(&self, target_id: &str, expr: &str) -> Result<String, String> {
        self.eval_string_to(target_id, expr, cdp::CDP_CMD_TIMEOUT)
            .await
    }

    /// [`Self::eval_string`] 的显式超时变体（就绪轮询用 [`POLL_CDP_TIMEOUT`]）。
    async fn eval_string_to(
        &self,
        target_id: &str,
        expr: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let v = self.evaluate_to(target_id, expr, timeout).await?;
        match v.get("value") {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(Value::Null) | None => Ok(String::new()),
            Some(other) => Ok(other.to_string()),
        }
    }

    /// readyState 轮询等待加载（超时 = 诚实报当前状态非报错）。返回
    /// `(状态, 对话框是否被自动关闭)`——调用方据此在结果面注记
    /// `dialog_dismissed`（六轮复核：等待期间的关框是自愈副作用，模型
    /// 必须可感知）。
    ///
    /// F14（2026-11-15 三审）：Page.navigate 的响应到达时导航仅「开始」
    /// （commit 前），旧文档 readyState 仍是 complete——无脑 poll 会在首轮
    /// 假命中旧文档的 complete 提前返回，模型立刻 extract 读到旧页内容
    /// （慢站必翻车）。以 performance.timeOrigin 为文档身份锚：新文档
    /// commit + load 完成 = origin 已变再 complete。same_doc（同 URL 去
    /// fragment）= 同文档导航无新文档，跳过身份闸。origin 读失败 = 旧文档
    /// 上下文已销毁（导航已 commit）——视为已换文档。
    ///
    /// 六轮复核（2026-11-18）：新页面加载期弹 alert 是常见形态（解析期
    /// alert 阻塞渲染器 = readyState 停滞 + 轮询撞框）——轮询用
    /// [`POLL_CDP_TIMEOUT`] 短超时，撞框由 eval_conn 超时臂自愈关框；
    /// 关框阻塞不是页面加载时间，deadline 一次性顺延（只顺延一次，
    /// 防 alert 循环页无界等待）。
    async fn wait_document_ready(
        &self,
        target_id: &str,
        timeout_ms: u64,
        old_origin: Option<String>,
        same_doc: bool,
    ) -> (String, bool) {
        let budget = Duration::from_millis(timeout_ms.min(u64::MAX / 2));
        let mut deadline = std::time::Instant::now() + budget;
        let mut dialog_dismissed = false;
        let mut deadline_extended = false;
        loop {
            let state = self
                .eval_string_to(target_id, "document.readyState", POLL_CDP_TIMEOUT)
                .await;
            if state.as_ref().is_err_and(|e| e.contains("已自动关闭")) {
                dialog_dismissed = true;
            }
            let committed = if same_doc {
                true
            } else {
                let origin = self
                    .eval_string_to(
                        target_id,
                        "String(performance.timeOrigin)",
                        POLL_CDP_TIMEOUT,
                    )
                    .await;
                if origin.as_ref().is_err_and(|e| e.contains("已自动关闭")) {
                    dialog_dismissed = true;
                }
                origin.as_deref().ok() != old_origin.as_deref()
            };
            if state.as_deref() == Ok("complete") && committed {
                return ("complete".to_string(), dialog_dismissed);
            }
            if dialog_dismissed && !deadline_extended {
                // 关框阻塞不计入加载预算；只顺延一次（alert 循环页不会
                // 把等待拖成无界）。
                deadline_extended = true;
                deadline = std::time::Instant::now() + budget;
            }
            if std::time::Instant::now() >= deadline {
                return (
                    state.unwrap_or_else(|_| "unknown".to_string()),
                    dialog_dismissed,
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

#[async_trait]
impl BrowserDriver for ChromiumCdpDriver {
    fn kind(&self) -> DriverKind {
        self.kind
    }

    fn capabilities(&self) -> DriverCaps {
        DriverCaps {
            trusted_input: true,
            screenshot: true,
            events: true,
        }
    }

    async fn list_tabs(&self) -> Result<Vec<TabInfo>, String> {
        let list = cdp::http_list(self.port).await?;
        Ok(cdp::page_targets(&list))
    }

    async fn open_tab(&self, url: Option<&str>) -> Result<TabInfo, String> {
        let browser = self.browser().await?;
        let res = browser
            .call(
                "Target.createTarget",
                json!({"url": url.unwrap_or("about:blank")}),
                cdp::CDP_CMD_TIMEOUT,
            )
            .await?;
        let id = res
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or("Target.createTarget 缺 targetId")?
            .to_string();
        // 快照拿 title/url（创建即导航场景 title 空，诚实展示）。
        tokio::time::sleep(Duration::from_millis(300)).await;
        let tabs = self.list_tabs().await?;
        Ok(tabs.into_iter().find(|t| t.id == id).unwrap_or(TabInfo {
            id,
            target_type: "page".to_string(),
            title: String::new(),
            url: url.unwrap_or("about:blank").to_string(),
            attached: false,
        }))
    }

    async fn close_tab(&self, target_id: &str) -> Result<(), String> {
        let browser = self.browser().await?;
        browser
            .call(
                "Target.closeTarget",
                json!({"targetId": target_id}),
                cdp::CDP_CMD_TIMEOUT,
            )
            .await?;
        self.conns.lock().remove(target_id);
        Ok(())
    }

    async fn activate_tab(&self, target_id: &str) -> Result<(), String> {
        let browser = self.browser().await?;
        browser
            .call(
                "Target.activateTarget",
                json!({"targetId": target_id}),
                cdp::CDP_CMD_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    async fn act(&self, target_id: &str, action: &WebAction) -> Result<Value, String> {
        match action {
            WebAction::Navigate { url, timeout_ms } => {
                let conn = self.page_conn(target_id).await?;
                // F33 同族（六轮复核 2026-11-18）：对话框开着时锚点
                // evaluate 撞满 30s 超时被 `.ok()` 吞错 → same_doc 保守
                // 判定（F14 闸退化），Page.navigate 与加载等待同样被阻塞
                // ——动作入口前置探测关闭（探针即关框，无框毫秒级）。
                let dismissed_entry = dismiss_dialog_if_open(&conn, None).await;
                // F14：导航前采文档身份锚 + 同文档判定（见
                // wait_document_ready 注释——commit 前旧文档 complete 会
                // 假命中）。
                let old_origin = self
                    .eval_string(target_id, "String(performance.timeOrigin)")
                    .await
                    .ok();
                let old_href = self.eval_string(target_id, "location.href").await.ok();
                let same_doc = matches!(
                    (&old_href, url.as_str()),
                    (Some(h), u) if h.split('#').next().unwrap_or("") == u.split('#').next().unwrap_or("")
                );
                conn.call("Page.navigate", json!({"url": url}), cdp::CDP_CMD_TIMEOUT)
                    .await?;
                let (load_state, dismissed_during) = self
                    .wait_document_ready(
                        target_id,
                        timeout_ms
                            .unwrap_or(NAV_WAIT_DEFAULT_MS)
                            .min(NAV_WAIT_MAX_MS),
                        old_origin,
                        same_doc,
                    )
                    .await;
                Ok(note_dialog_dismissed(
                    json!({
                        "ok": true,
                        "url": url,
                        "load_state": load_state,
                    }),
                    dismissed_entry || dismissed_during,
                ))
            }
            WebAction::Click { selector, x, y } => {
                if let Some(sel) = selector {
                    let expr = js_click(sel);
                    let out = self.eval_string(target_id, &expr).await?;
                    if out == "ERROR:no-element" {
                        return Err(format!("未找到元素: {sel}"));
                    }
                    Ok(json!({"ok": true, "selector": sel}))
                } else if let Some((x, y)) = x.zip(*y) {
                    // trusted 坐标点击（canvas 游戏路径）。
                    let conn = self.page_conn(target_id).await?;
                    // F33：对话框开着时 trusted 输入被渲染器静默吞（命令
                    // ok 但页面无反应）——派发前探测关闭 + 回执注明。
                    let dismissed = dismiss_dialog_if_open(&conn, None).await;
                    // F55（2026-11-23 live 实证）：导航后首个合成输入事件会被
                    // 输入管线静默吞——press+release 双双蒸发（5/6 现场，连
                    // document 捕获级监听都收不到），第二个事件起稳定落盘，
                    // 与坐标无关（(200,150) 首点同样被吞、(120,80) 作为第二
                    // 点击精确命中）。selector 点击（JS eval）不走输入管线
                    // 故无此症。先派 mouseMoved 暖场吸收这次丢弃——press 永不
                    // 做首个事件，语义上也对齐真人输入（移动→按下→抬起）。
                    conn.call(
                        "Input.dispatchMouseEvent",
                        json!({"type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0}),
                        cdp::CDP_CMD_TIMEOUT,
                    )
                    .await?;
                    conn.call(
                        "Input.dispatchMouseEvent",
                        json!({"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1, "buttons": 1}),
                        cdp::CDP_CMD_TIMEOUT,
                    )
                    .await?;
                    conn.call(
                        "Input.dispatchMouseEvent",
                        json!({"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1, "buttons": 0}),
                        cdp::CDP_CMD_TIMEOUT,
                    )
                    .await?;
                    Ok(note_dialog_dismissed(
                        json!({"ok": true, "x": x, "y": y}),
                        dismissed,
                    ))
                } else {
                    Err("click 需要 selector 或 x+y 坐标（二选一）".to_string())
                }
            }
            WebAction::Type { selector, text } => {
                let expr = js_type(selector, text);
                let out = self.eval_string(target_id, &expr).await?;
                if out == "ERROR:no-element" {
                    // 不回显 text（可能是凭据明文——守卫③）。
                    return Err(format!("未找到元素: {selector}"));
                }
                // 字符数而非内容（同上，结果面不回显文本）。
                Ok(json!({"ok": true, "selector": selector, "chars": text.chars().count()}))
            }
            WebAction::Key {
                key,
                phase,
                hold_ms,
                selector,
            } => {
                // 焦点承接（可选 selector）：trusted 键事件需要页面已有焦点
                // 元素（真实 mousedown 才聚焦、合成 click 不聚焦）。带
                // selector 时先 el.focus()（无点击副作用），并验证焦点真落
                // 在目标上——不可聚焦元素（div 无 tabindex / disabled）诚实
                // 报错，不给「静默丢键」留门。
                if let Some(sel) = selector {
                    let expr = js_focus(sel);
                    let out = self.eval_string(target_id, &expr).await?;
                    match out.as_str() {
                        "OK" => {}
                        "ERROR:no-element" => return Err(format!("未找到元素: {sel}")),
                        "ERROR:not-focusable" => {
                            return Err(format!(
                                "元素不可聚焦（disabled/readonly/无 tabindex）: {sel}——键盘事件将被渲染器丢弃"
                            ));
                        }
                        other => return Err(format!("焦点承接失败: {other}")),
                    }
                }
                let conn = self.page_conn(target_id).await?;
                // F33：对话框开着时 trusted 键事件整包被渲染器静默丢弃——
                // 派发前探测关闭（焦点承接的 evaluate 若撞对话框由其超时
                // 臂自行处理，无需此处重复）。
                let dismissed = dismiss_dialog_if_open(&conn, None).await;
                // F52：复合键名（ctrl+shift+a）解析——修饰物理帧序列 + 基键
                // （base.extra_mods 已并入组合位）。无 '+' 单键 = 空修饰序列。
                let (mut mods, spec) = parse_combo(key)?;
                // F41：大写字母键自动 shift 包裹（组合序列未显式含 shift 时
                // 补挂——"ctrl+A" = Shift 物理帧 + A 帧 modifiers=10，与
                // "ctrl+shift+A" 等效）。
                if spec.shifted && !mods.iter().any(|m| m.key == "Shift") {
                    mods.push(key_spec("shift")?);
                }
                match phase {
                    KeyPhase::Down | KeyPhase::Up if spec.shifted || !mods.is_empty() => {
                        return Err(
                            "大写字母键/组合键只支持 phase=press（自动修饰键包裹）；按住场景请先发修饰键 phase=down、再发基键 phase=down（注意：修饰状态跨调用不传递，组合键请用复合键名 ctrl+shift+a）".to_string(),
                        );
                    }
                    KeyPhase::Down => {
                        dispatch_key_down(&conn, &spec).await?;
                    }
                    KeyPhase::Up => {
                        dispatch_key_up(&conn, &spec).await?;
                    }
                    KeyPhase::Press => {
                        for m in &mods {
                            dispatch_key_down(&conn, m).await?;
                        }
                        dispatch_key_down(&conn, &spec).await?;
                        tokio::time::sleep(Duration::from_millis(hold_ms.unwrap_or(60).min(3000)))
                            .await;
                        dispatch_key_up(&conn, &spec).await?;
                        // 修饰键释放倒序（物理键盘：后按先放）。
                        for m in mods.iter().rev() {
                            dispatch_key_up(&conn, m).await?;
                        }
                    }
                }
                Ok(note_dialog_dismissed(
                    json!({"ok": true, "key": key, "phase": format!("{phase:?}").to_lowercase(),
                           "focused": selector.is_some()}),
                    dismissed,
                ))
            }
            WebAction::Scroll { x, y, dx, dy } => {
                let conn = self.page_conn(target_id).await?;
                // F33：滚轮与点击同族（对话框开着 = 静默吞）。
                let dismissed = dismiss_dialog_if_open(&conn, None).await;
                // 视口中心缺省（mouseWheel 必须带坐标）。
                let (cx, cy) = match (x, y) {
                    (Some(x), Some(y)) => (*x, *y),
                    _ => {
                        let w = self
                            .evaluate(target_id, "window.innerWidth")
                            .await
                            .ok()
                            .and_then(|v| v.get("value").and_then(|v| v.as_f64()))
                            .unwrap_or(800.0);
                        let h = self
                            .evaluate(target_id, "window.innerHeight")
                            .await
                            .ok()
                            .and_then(|v| v.get("value").and_then(|v| v.as_f64()))
                            .unwrap_or(600.0);
                        (w / 2.0, h / 2.0)
                    }
                };
                conn.call(
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseWheel", "x": cx, "y": cy, "deltaX": dx, "deltaY": dy}),
                    cdp::CDP_CMD_TIMEOUT,
                )
                .await?;
                Ok(note_dialog_dismissed(
                    json!({"ok": true, "dx": dx, "dy": dy}),
                    dismissed,
                ))
            }
            WebAction::ExtractText { selector } => {
                let expr = js_extract(selector.as_deref());
                let out = self.eval_string(target_id, &expr).await?;
                if out == "ERROR:no-element" {
                    return Err(format!(
                        "未找到元素: {}",
                        selector.as_deref().unwrap_or("body")
                    ));
                }
                let (text, truncated) = truncate_head_tail(&out, EXTRACT_HEAD, EXTRACT_TAIL);
                Ok(json!({
                    "ok": true,
                    "chars": out.chars().count(),
                    "text": text,
                    "truncated": truncated,
                }))
            }
            WebAction::Screenshot => {
                if !self.capabilities().screenshot {
                    return Err("当前驱动不支持截图".to_string());
                }
                let conn = self.page_conn(target_id).await?;
                // F33 同族（六轮复核 2026-11-18）：视口 metrics evaluate
                // 渲染器绑定，对话框开着时五个 `.ok()` 全吞错 → vw/vh/dpr
                // 全 None → 连坐 device_px 降级截图（F32 坐标归一被击穿，
                // 缩放屏上模型量坐标恒脱靶）。动作入口前置探测关闭。
                let dismissed = dismiss_dialog_if_open(&conn, None).await;
                // F32（2026-11-15 六审）：坐标系归一。裸 captureScreenshot
                // 按设备缩放输出**物理像素**（Windows 125%/150% 缩放是常态
                // 而非例外），而 Input.dispatchMouseEvent 的 x/y 是 **CSS
                // 像素**——模型从截图量出的坐标直接喂点击会按 DPR 比例
                // 偏移，锚定主场景（截图→视觉→坐标点击）在缩放屏上恒脱
                // 靶。clip scale=1 输出 CSS 像素图，与输入坐标 1:1。视口
                // 读取失败 = 裸截图诚实降级（结果 coordinate_space 注明
                // device_px，模型可自查）。
                let vw = self
                    .eval_string(target_id, "String(window.innerWidth)")
                    .await
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok());
                let vh = self
                    .eval_string(target_id, "String(window.innerHeight)")
                    .await
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok());
                let dpr = self
                    .eval_string(target_id, "String(window.devicePixelRatio)")
                    .await
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok());
                // F38：clip 锚当前滚动位置（视口左上角的文档坐标）。
                let sx = self
                    .eval_string(target_id, "String(window.scrollX)")
                    .await
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .unwrap_or(0.0);
                let sy = self
                    .eval_string(target_id, "String(window.scrollY)")
                    .await
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .unwrap_or(0.0);
                // captureScreenshot 同为渲染器绑定调用（七轮复核发现 2）：
                // 入口探针关框后页面恢复、排队 alert 立即弹的窄窗竞态仍在
                // ——包对话框兜底臂（撞框 = 注解错误可重试，非裸 30s）。
                let res = call_with_dialog_arm(
                    &conn,
                    "Page.captureScreenshot",
                    screenshot_params(vw.zip(vh), (sx, sy)),
                )
                .await?;
                let data = res
                    .get("data")
                    .and_then(|d| d.as_str())
                    .ok_or("Page.captureScreenshot 缺 data")?;
                Ok(note_dialog_dismissed(
                    json!({
                        "ok": true,
                        "png_base64": data,
                        "viewport": {"width": vw, "height": vh},
                        "dpr": dpr,
                        "coordinate_space": if vw.is_some() && vh.is_some() {
                            "css_px"
                        } else {
                            "device_px"
                        },
                    }),
                    dismissed,
                ))
            }
            WebAction::WaitFor {
                selector,
                timeout_ms,
            } => {
                let timeout = timeout_ms
                    .unwrap_or(WAIT_FOR_DEFAULT_MS)
                    .min(WAIT_FOR_MAX_MS);
                let expr = js_exists(selector);
                let conn = self.page_conn(target_id).await?;
                // F33 同族（六轮复核 2026-11-18）：等待中的元素常与对话框
                // 并存（页面 alert 挡住渲染/脚本）——存在性轮询撞框会假报
                // 「未命中」。入口前置探测 + 轮询短超时（撞框由 eval_conn
                // 超时臂自愈），关框阻塞不计入等待预算（deadline 一次性
                // 顺延，防 alert 循环页无界）。
                let mut dialog_dismissed = dismiss_dialog_if_open(&conn, None).await;
                let mut deadline_extended = false;
                let budget = Duration::from_millis(timeout);
                let mut deadline = std::time::Instant::now() + budget;
                // F15：报实际等待时长而非配额上限（秒中即中却报「等了 10s」
                // 是结果面撒谎，模型按此判断页面状态会被误导）。
                let started = std::time::Instant::now();
                let waited = |d: Duration| d.as_millis() as u64;
                loop {
                    let found = match self
                        .eval_string_to(target_id, &expr, POLL_CDP_TIMEOUT)
                        .await
                    {
                        Ok(s) => s == "true",
                        Err(e) => {
                            if e.contains("已自动关闭") {
                                dialog_dismissed = true;
                            }
                            false
                        }
                    };
                    if dialog_dismissed && !deadline_extended {
                        deadline_extended = true;
                        deadline = std::time::Instant::now() + budget;
                    }
                    if found {
                        return Ok(note_dialog_dismissed(
                            json!({"ok": true, "found": true, "waited_ms": waited(started.elapsed())}),
                            dialog_dismissed,
                        ));
                    }
                    if std::time::Instant::now() >= deadline {
                        return Ok(note_dialog_dismissed(
                            json!({"ok": false, "found": false, "waited_ms": waited(started.elapsed())}),
                            dialog_dismissed,
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            WebAction::Inspect { selector, frame } => {
                let expr = js_inspect(selector);
                let (v, frame_label) = self
                    .eval_dom_routed(target_id, frame.as_deref(), &expr)
                    .await?;
                let out = v
                    .get("value")
                    .and_then(|x| x.as_str())
                    .ok_or("inspect 返回非字符串（页面异常）")?;
                if out == "ERROR:no-element" {
                    return Err(format!("未找到元素: {selector}"));
                }
                let parsed: Value = serde_json::from_str(out)
                    .map_err(|e| format!("inspect 结果 JSON 解析失败: {e}"))?;
                // 扁平组装（§3.1 返回形态：selector/frame 回显 + JS 字段）。
                let mut out_json = json!({"ok": true, "selector": selector, "frame": frame_label});
                if let (Some(obj), Some(tgt)) = (parsed.as_object(), out_json.as_object_mut()) {
                    for (k, val) in obj {
                        tgt.insert(k.clone(), val.clone());
                    }
                }
                Ok(out_json)
            }
            WebAction::SnapshotDom {
                selector,
                frame,
                max_depth,
                max_nodes,
            } => {
                // 钳制在 driver 层（run_script 编排步骤同源收敛——工具面
                // schema 只是模型面指引，两条通路都过这里）。
                let depth = max_depth
                    .unwrap_or(SNAP_DEPTH_DEFAULT)
                    .clamp(1, SNAP_DEPTH_MAX);
                let nodes_cap = max_nodes
                    .unwrap_or(SNAP_NODES_DEFAULT)
                    .clamp(1, SNAP_NODES_MAX);
                let expr = js_snapshot(selector.as_deref(), depth, nodes_cap);
                let (v, frame_label) = self
                    .eval_dom_routed(target_id, frame.as_deref(), &expr)
                    .await?;
                let out = v
                    .get("value")
                    .and_then(|x| x.as_str())
                    .ok_or("snapshot 返回非字符串（页面异常）")?;
                if out == "ERROR:no-element" {
                    return Err(format!(
                        "未找到元素: {}",
                        selector.as_deref().unwrap_or("body")
                    ));
                }
                let parsed: Value = serde_json::from_str(out)
                    .map_err(|e| format!("snapshot 结果 JSON 解析失败: {e}"))?;
                let mut out_json = json!({"ok": true, "frame": frame_label});
                if let Some(sel) = selector {
                    out_json["selector"] = json!(sel);
                }
                if let (Some(obj), Some(tgt)) = (parsed.as_object(), out_json.as_object_mut()) {
                    for (k, val) in obj {
                        tgt.insert(k.clone(), val.clone());
                    }
                }
                Ok(out_json)
            }
            WebAction::GetDom {
                selector,
                frame,
                max_chars,
            } => {
                let cap = max_chars
                    .unwrap_or(GET_DOM_CHARS_DEFAULT)
                    .clamp(200, GET_DOM_CHARS_MAX) as usize;
                let expr = js_get_dom(selector);
                let (v, frame_label) = self
                    .eval_dom_routed(target_id, frame.as_deref(), &expr)
                    .await?;
                let out = v
                    .get("value")
                    .and_then(|x| x.as_str())
                    .ok_or("get_dom 返回非字符串（页面异常）")?;
                if out == "ERROR:no-element" {
                    return Err(format!("未找到元素: {selector}"));
                }
                let total = out.chars().count();
                if total <= cap {
                    return Ok(json!({
                        "ok": true, "selector": selector, "frame": frame_label,
                        "html": out, "truncated": false, "chars": total,
                    }));
                }
                // head+tail 截断（复用 extract 同款形态；注记内嵌）。
                let head = cap * 2 / 3;
                let tail = cap - head;
                let (text, truncated) = truncate_head_tail(out, head, tail);
                Ok(json!({
                    "ok": true, "selector": selector, "frame": frame_label,
                    "html": text, "truncated": truncated, "chars": total,
                }))
            }
        }
    }

    async fn tab_origin(&self, target_id: &str) -> Result<String, String> {
        self.eval_string(target_id, crate::creds::JS_ORIGIN).await
    }

    async fn is_password_input(&self, target_id: &str, selector: &str) -> Result<bool, String> {
        let expr = crate::creds::js_is_password_input(selector);
        Ok(self.eval_string(target_id, &expr).await? == "true")
    }

    async fn tab_ready_state(&self, target_id: &str) -> Result<String, String> {
        // 只有已附着（连接存活）的 tab 能问 readyState；未附着诚实 Err。
        let exists = self.conns.lock().contains_key(target_id);
        if !exists {
            return Err(format!("tab {target_id} 未附着"));
        }
        self.eval_string(target_id, "document.readyState").await
    }
}

// ---------------------------------------------------------------------------
// JS 对话框守卫（F33，2026-11-15 六审）
// ---------------------------------------------------------------------------

/// 截图参数构造（F32 纯函数面；F38 滚动词域修正）：视口尺寸可读时加
/// clip scale:1 归一到 CSS 像素坐标系（与 Input.dispatchMouseEvent 坐标
/// 1:1——截图→视觉→坐标点击闭环的前提）；尺寸不可读/非法 = 裸截图诚实
/// 降级（物理像素）。
///
/// F38（2026-11-16 七审）：clip 的 x/y 是**文档坐标**（page-relative），
/// 锚 (0,0) 截的是文档顶部——页面一滚动截图就与用户所见画面脱节（坐标
/// 也对不上：截图里量到的元素在视口坐标里要减去 scroll）。锚点改取
/// scrollX/scrollY（当前视口左上角在文档中的位置）+ 显式
/// captureBeyondViewport:true 走「整页栅格化后按文档坐标裁剪」的确定性
/// 路径（Chrome fullPage 同款管线，clip 语义在这条路径上有明确定义）。
fn screenshot_params(viewport: Option<(f64, f64)>, scroll: (f64, f64)) -> Value {
    match viewport {
        Some((w, h)) if w > 0.0 && h > 0.0 => json!({
            "format": "png",
            "captureBeyondViewport": true,
            "clip": {"x": scroll.0, "y": scroll.1, "width": w, "height": h, "scale": 1},
        }),
        _ => json!({"format": "png"}),
    }
}

/// 若页面有未处理 JS 对话框（alert/confirm/prompt）则按「拒绝」自动关闭
/// （Page.handleJavaScriptDialog accept=false，Playwright 缺省同款），返回
/// 是否关了对话框。对话框打开时页面 JS 暂停：evaluate 恒 30s 超时、
/// **trusted 输入被渲染器静默吞**（命令返回 ok 但页面毫无反应——比报错
/// 更阴险）。挂点：evaluate 超时臂 + 三个 trusted 输入臂（click 坐标/
/// scroll/key）派发前置探针。无对话框时 handleJavaScriptDialog 报错 =
/// 本函数 false，开销一次 2s 超时上界的 CDP 往返（常态毫秒级）。
/// `session` = flatten 子会话通路。四轮复核活体修正：session 路由对
/// OOPIF frame target 的对话框**关不掉**（JS 对话框是 tab 级、挂
/// WebContents 的 DialogManager，frame target 的 Page 域不带对话框状态
/// ——edge_live OOPIF 弹框用例首跑实证）；OOPIF 臂的真正关框通路是
/// `eval_conn` 的 `dismiss_fallback`（page conn + None）。此处 session
/// 参数保留先试（失败毫秒级、无害），主 frame / 同进程场景仍走本通路
/// （三轮实证有效）。
async fn dismiss_dialog_if_open(conn: &CdpConn, session: Option<&str>) -> bool {
    conn.call_in_session(
        session,
        "Page.handleJavaScriptDialog",
        json!({"accept": false}),
        Duration::from_secs(2),
    )
    .await
    .is_ok()
}

/// 在结果 JSON 里注明对话框被自动关闭（仅命中时；回执面诚实留痕）。
fn note_dialog_dismissed(mut out: Value, dismissed: bool) -> Value {
    if dismissed {
        out["dialog_dismissed"] = json!(true);
    }
    out
}

/// 渲染器绑定调用的对话框兜底（同进程 route 路径专用，2026-11-18 三轮
/// 复核活体抓出）：同进程 iframe 的 JS 对话框阻塞渲染器主线程，
/// `Page.getFrameTree`/`Page.createIsolatedWorld` 由渲染器处理，同样撞满
/// 30s 超时——eval_conn 的自愈臂救不了（根本走不到 evaluate，此前本路径
/// 裸超时无自愈，重试永远失败）。超时 → 探测关框 → 回注解错误（重试
/// 自愈；契约与 [`Self::eval_conn`] 超时臂同形）。浏览器级调用
/// （Target.getTargets / attachToTarget）不经此——browser 进程处理，
/// 不被页面对话框阻塞。
///
/// 注解用短版（无 F39 副作用警示）：三个接线点（getFrameTree /
/// createIsolatedWorld / captureScreenshot——七轮复核接入）皆无副作用的
/// 一次性调用，重试天然安全；eval_conn 臂服务 evaluate（常带副作用），
/// 完整警示归它。「轮询类」限定词已删（八轮复核：接线点皆非轮询，
/// 限定词错配会误导模型按「非轮询所以不重试」处理）。
async fn call_with_dialog_arm(
    page: &CdpConn,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    match page.call(method, params, cdp::CDP_CMD_TIMEOUT).await {
        Ok(v) => Ok(v),
        Err(e) if e.contains("timeout") => {
            if dismiss_dialog_if_open(page, None).await {
                return Err(format!(
                    "{e}；检测到阻塞页面的 JS 对话框，已自动关闭（按拒绝处理）。\
                     重试即可自愈"
                ));
            }
            Err(e)
        }
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// frame 路由（DOM 只读动作，2026-11-18 计划 §3.7）
// ---------------------------------------------------------------------------

/// frame 参数解析结果（执行通路）。
enum FrameRoute {
    /// 主 frame（页面级 conn 直用）。
    Main,
    /// 同进程 iframe（隔离 world contextId；保留 frame_id 供失效逐出）。
    SameProcess { frame_id: String, context_id: u64 },
    /// 跨域 OOPIF（flatten 子会话 sessionId；保留 target_id 供失效逐出）。
    Oopif {
        session_id: String,
        oopif_target_id: String,
    },
}

/// 失效错误判别（逐出自愈触发器，§3.7）：页面导航后隔离 world 换 context、
/// OOPIF 换 target、会话关闭——CDP 错误文本钉这四类。
fn is_stale_frame_error(e: &str) -> bool {
    const NEEDLES: [&str; 4] = [
        "Cannot find context",
        "No frame with given id",
        "Target closed",
        "Session closed",
    ];
    NEEDLES.iter().any(|n| e.contains(n))
}

/// Runtime.evaluate 响应 → result 值（exceptionDetails → Err）。
fn eval_result_value(res: Value, expr: &str) -> Result<Value, String> {
    let _ = expr;
    if res.get("exceptionDetails").is_some() {
        let msg = res
            .pointer("/exceptionDetails/exception/description")
            .and_then(|d| d.as_str())
            .or_else(|| {
                res.pointer("/exceptionDetails/exception/details/0/text")
                    .and_then(|t| t.as_str())
            })
            .unwrap_or("JS exception");
        return Err(format!("JS 异常: {msg}"));
    }
    Ok(res.get("result").cloned().unwrap_or(Value::Null))
}

/// frameTree JSON → [(frameId, url)] 扁平清单（先根后子；嵌套 childFrames
/// 递归）。frameTree 形态：`{frame: {id, url, ...}, childFrames: [...]}`。
fn flatten_frame_tree(v: &Value, out: &mut Vec<(String, String)>) {
    let Some(frame) = v.get("frame") else { return };
    let id = frame
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let url = frame
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if !id.is_empty() {
        out.push((id, url));
    }
    if let Some(kids) = v.get("childFrames").and_then(|x| x.as_array()) {
        for k in kids {
            flatten_frame_tree(k, out);
        }
    }
}

/// frameTree 里按 URL 子串找 frameId（首个命中——多匹配与 querySelector
/// 首个语义一致）。
fn find_frame_id(tree: &Value, url_sub: &str) -> Option<String> {
    let mut out = Vec::new();
    flatten_frame_tree(tree, &mut out);
    out.into_iter()
        .find(|(_, url)| url.contains(url_sub))
        .map(|(id, _)| id)
}

/// frame URL 清单（报错面，§3.7）：OOPIF targets 先列（cross-process
/// 标注）+ 同进程 frame 树（仅 frame 树出现 = same-process；URL 已在
/// OOPIF 列表的跳过）。清单即发现机制——模型据此换更精确子串重试。
fn frame_url_list(tree: &Value, targets_res: &Value) -> String {
    let mut items: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(list) = targets_res.get("targetInfos").and_then(|v| v.as_array()) {
        for t in list {
            if t.get("type").and_then(|x| x.as_str()) != Some("iframe") {
                continue;
            }
            let url = t.get("url").and_then(|x| x.as_str()).unwrap_or("");
            if url.is_empty() || !seen.insert(url.to_string()) {
                continue;
            }
            items.push(format!("{url} [cross-process]"));
        }
    }
    let mut frames = Vec::new();
    flatten_frame_tree(tree, &mut frames);
    for (_, url) in frames {
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        items.push(format!("{url} [same-process]"));
    }
    if items.is_empty() {
        return "（无）".to_string();
    }
    items.join("；")
}

// ---------------------------------------------------------------------------
// JS 片段（选择器动作路径；selector/text 经 js_str 引号安全嵌入）
// ---------------------------------------------------------------------------

/// DOM 点击：mousedown（focus 处理器）+ 原生 click（一次 click 事件）。
fn js_click(selector: &str) -> String {
    let sel = cdp::js_str(selector);
    format!(
        "(() => {{ const el = document.querySelector({sel}); \
         if (!el) return 'ERROR:no-element'; \
         el.scrollIntoView({{block: 'center', inline: 'center'}}); \
         const r = el.getBoundingClientRect(); \
         const opts = {{bubbles: true, cancelable: true, view: window, \
           clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, button: 0}}; \
         el.dispatchEvent(new MouseEvent('mousedown', opts)); \
         el.dispatchEvent(new MouseEvent('mouseup', opts)); \
         el.click(); \
         return 'OK'; }})()"
    )
}

/// 文本输入：native value setter + input/change（React/Vue 受控组件兼容，
/// guest 版实证形态）。
fn js_type(selector: &str, text: &str) -> String {
    let sel = cdp::js_str(selector);
    let txt = cdp::js_str(text);
    format!(
        "(() => {{ const el = document.querySelector({sel}); \
         if (!el) return 'ERROR:no-element'; \
         el.focus(); \
         const proto = el.tagName === 'TEXTAREA' \
           ? window.HTMLTextAreaElement.prototype \
           : window.HTMLInputElement.prototype; \
         const desc = Object.getOwnPropertyDescriptor(proto, 'value'); \
         if (desc && desc.set) {{ desc.set.call(el, {txt}); }} else {{ el.value = {txt}; }} \
         el.dispatchEvent(new Event('input', {{bubbles: true}})); \
         el.dispatchEvent(new Event('change', {{bubbles: true}})); \
         return 'OK'; }})()"
    )
}

/// 焦点承接（key 动作可选前置）：el.focus() + 焦点落点验证（trusted 键
/// 事件需页面有焦点元素；无点击副作用——Puppeteer elementHandle.focus()
/// 同款模式）。不可聚焦元素（disabled/readonly/无 tabindex）返回
/// ERROR:not-focusable，调用方诚实报错而非静默丢键。
fn js_focus(selector: &str) -> String {
    let sel = cdp::js_str(selector);
    format!(
        "(() => {{ const el = document.querySelector({sel}); \
         if (!el) return 'ERROR:no-element'; \
         el.focus(); \
         return document.activeElement === el ? 'OK' : 'ERROR:not-focusable'; }})()"
    )
}

/// 文本提取：innerText || textContent。**硬规则：不读输入框 .value**
/// （凭据打字后 extract 是现实路径，DOM value 有明文，计划 §5）。
fn js_extract(selector: Option<&str>) -> String {
    match selector {
        Some(sel) => {
            let sel = cdp::js_str(sel);
            format!(
                "(() => {{ const el = document.querySelector({sel}); \
                 if (!el) return 'ERROR:no-element'; \
                 return String(el.innerText || el.textContent || ''); }})()"
            )
        }
        None => "String(document.body.innerText || document.body.textContent || '')".to_string(),
    }
}

/// 元素存在性探针（wait_for 轮询用）。
fn js_exists(selector: &str) -> String {
    let sel = cdp::js_str(selector);
    format!("String(!!document.querySelector({sel}))")
}

/// snapshot 钳制（缺省 + 上下限；真实页面 div 嵌套常超 6 层——默认 12、
/// 上限 30 防频繁截断）。
const SNAP_DEPTH_DEFAULT: u32 = 12;
const SNAP_DEPTH_MAX: u32 = 30;
const SNAP_NODES_DEFAULT: u32 = 300;
const SNAP_NODES_MAX: u32 = 2000;
/// get_dom 输出字符上限（缺省 12000 / 钳 [200, 50000]）。
const GET_DOM_CHARS_DEFAULT: u32 = 12_000;
const GET_DOM_CHARS_MAX: u32 = 50_000;

/// DOM 元素结构化检视（只读；返回 JSON 字符串——宿主解析）。
///
/// 语义（计划 §3.2 property/attribute 分层）：open/checked/disabled/required
/// 读 property（动态状态准——用户点击后 attribute 不更新）；expanded/
/// role/name/type 读 attribute；属性白名单输出，`value` 恒不在名单（凭据
/// 面硬规则）。`el.open === undefined` 归一 null（2026-11-18 探针实测：
/// 非 details 元素 open 是 undefined，JSON.stringify 会整键丢掉——显式
/// norm 保键位稳定）。visible = 有几何 + display/visibility 非隐藏。
fn js_inspect(selector: &str) -> String {
    let sel = cdp::js_str(selector);
    format!(
        r#"(() => {{
  const el = document.querySelector({sel});
  if (!el) return 'ERROR:no-element';
  const norm = (v) => (v === undefined ? null : v);
  const WL = ['id','class','name','type','role','placeholder','title','alt','lang','dir','href','src','action','method','for','disabled','checked','required','readonly','open','selected','multiple','autofocus','autocomplete'];
  const attrs = {{}};
  for (const a of el.attributes) {{
    const n = a.name.toLowerCase();
    const hit = WL.includes(n) || n.startsWith('aria-') || n.startsWith('data-');
    if (!hit) continue;
    let v = a.value;
    if ((n === 'href' || n === 'src' || n === 'action') && v.length > 200) v = v.slice(0, 200) + '…';
    attrs[n] = v;
  }}
  const r = el.getBoundingClientRect();
  const cs = getComputedStyle(el);
  const visible = r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none';
  return JSON.stringify({{
    tag: el.tagName.toLowerCase(),
    id: el.id || null,
    name: el.getAttribute('name'),
    type: el.getAttribute('type'),
    role: el.getAttribute('role'),
    open: norm(el.open),
    checked: norm(el.checked),
    disabled: norm(el.disabled),
    expanded: el.getAttribute('aria-expanded'),
    required: norm(el.required),
    visible,
    rect: {{x: r.x, y: r.y, w: r.width, h: r.height}},
    attributes: attrs,
  }});
}})()"#
    )
}

/// 有界 DOM 树快照（只读；返回 JSON 字符串——宿主解析）。
///
/// 语义（计划 §3.4）：深度优先，depth/节点数双闸触顶 truncated:true；
/// text 只给无元素子节点的叶子（容器 textContent 是后代拼接会与子行
/// 重复）；id≤80/cls≤120 截断（防构建 hash 类名）；class 一律
/// getAttribute('class')（SVGElement.className 是 SVGAnimatedString，
/// 直接取序列化成对象垃圾）；shadowRoot 递归穿透（shadow:true 标记，
/// depth 预算同池）；不进 iframe contentDocument（iframe 元素本身入树，
/// src 在 inspect 白名单内可见）。布尔投影 !!（open/disabled 是 property）。
fn js_snapshot(selector: Option<&str>, max_depth: u32, max_nodes: u32) -> String {
    let sel = match selector {
        Some(s) => cdp::js_str(s),
        None => "null".to_string(),
    };
    format!(
        r#"(() => {{
  const selEl = {sel} === null ? null : document.querySelector({sel});
  const root = {sel} === null ? document.body : selEl;
  if (!root) return 'ERROR:no-element';
  const MD = {max_depth}, MN = {max_nodes};
  let emitted = 0, truncated = false;
  const nodes = [];
  const clip = (s, n) => {{ s = String(s); return s.length > n ? s.slice(0, n) + '…' : s; }};
  const walk = (el, depth, inShadow) => {{
    if (depth > MD || emitted >= MN) {{ truncated = true; return; }}
    emitted++;
    const o = {{ tag: el.tagName.toLowerCase(), depth, childCount: el.childNodes.length }};
    if (el.id) o.id = clip(el.id, 80);
    const cls = el.getAttribute('class');
    if (cls) o.cls = clip(cls, 120);
    const role = el.getAttribute('role');
    if (role) o.role = role;
    if (el.children.length === 0) {{
      const t = (el.textContent || '').trim().slice(0, 120);
      if (t) o.text = t;
    }}
    if (el.open !== undefined) o.open = !!el.open;
    const exp = el.getAttribute('aria-expanded');
    if (exp !== null) o.expanded = exp;
    if (el.disabled !== undefined) o.disabled = !!el.disabled;
    if (inShadow) o.shadow = true;
    nodes.push(o);
    for (const k of el.children) walk(k, depth + 1, inShadow);
    if (el.shadowRoot) for (const k of el.shadowRoot.children) walk(k, depth + 1, true);
  }};
  walk(root, 0, false);
  return JSON.stringify({{ ts_ms: Date.now(), nodes, truncated, nodes_emitted: emitted }});
}})()"#,
        sel = sel,
        max_depth = max_depth,
        max_nodes = max_nodes,
    )
}

/// 子树 outerHTML 脱敏序列化（只读；返回 HTML 字符串——宿主截断）。
///
/// 语义（计划 §3.3）：clone 后剥 input value 属性、清 textarea
/// textContent（当前值在 textContent）、**后代 script/style 整体删除**
/// （`remove()`——edge_live 断言标签不在场；根元素自身是 script/style 时
/// 只清内容保标签，避免输出空串）。
/// **根元素自脱敏**：querySelectorAll 只匹配后代——selector 直指密码框
/// 本身时根也要剥（edge_live 实测抓出的缺口）。
/// select/option 不动（选项索引是页面数据非打字值）。双层安全：打字值走
/// native setter 设 property 天然不进 outerHTML——这里剥的是页面源码
/// value 属性（SSO 预填场景）；两层各自有效缺一不可。已知信息损失：
/// textarea 页面自带初始文本也被清（宁过度脱敏）。
fn js_get_dom(selector: &str) -> String {
    let sel = cdp::js_str(selector);
    format!(
        r#"(() => {{
  const el = document.querySelector({sel});
  if (!el) return 'ERROR:no-element';
  const copy = el.cloneNode(true);
  // 根元素自身命中脱敏对象时 querySelectorAll 覆盖不到（只匹配后代）
  // ——根也要处理（edge_live 实测：selector 直指密码框本身）。
  if (copy.matches('input')) copy.removeAttribute('value');
  if (copy.matches('textarea')) copy.textContent = '';
  if (copy.matches('script, style')) copy.textContent = '';
  copy.querySelectorAll('input').forEach((i) => i.removeAttribute('value'));
  copy.querySelectorAll('textarea').forEach((t) => {{ t.textContent = ''; }});
  copy.querySelectorAll('script, style').forEach((s) => s.remove());
  return copy.outerHTML;
}})()"#
    )
}

/// head+tail 截断（guest 同款形态），返回 (文本, 是否截断)。
pub fn truncate_head_tail(s: &str, head: usize, tail: usize) -> (String, bool) {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= head + tail {
        return (s.to_string(), false);
    }
    let h: String = chars[..head].iter().collect();
    let t: String = chars[chars.len() - tail..].iter().collect();
    (format!("{h}\n…[已截断]…\n{t}"), true)
}

// ---------------------------------------------------------------------------
// trusted 键盘（Input.dispatchKeyEvent）
// ---------------------------------------------------------------------------

/// 单键规格：CDP key 参数（key/text/code/windowsVirtualKeyCode）。
/// `shifted` = 该键派发需包裹 shift 按下期（F41：大写字母）。
/// `extra_mods` = 组合键带来的额外修饰位（F52：ctrl+a 的 a 帧带 2）。
struct KeySpec {
    key: String,
    code: String,
    vk: u32,
    shifted: bool,
    extra_mods: u32,
}

/// 键名 → CDP 规格（游戏 WASD + 表单键 + 方向/修饰；单字符 ASCII 兜底按
/// 大写 VK 码推导）。不认识的键名诚实拒绝。
///
/// F41（2026-11-16 八审）：大写字母键 = shift 包裹语义（Puppeteer 同款）——
/// 此前整体 `to_ascii_lowercase()` 后匹配，模型发 `key="A"` 静默键入小写
/// "a"：表单大写丢失、游戏 shift+W 冲刺语义荡然无存。现大写单 ASCII 字母
/// 保留大写 key/text/VK 并标记 `shifted`，Press 相位自动包 shift down/up；
/// Down/Up 相位遇大写诚实报错（不猜用户想按住什么）。
fn key_spec(name: &str) -> Result<KeySpec, String> {
    let n = name.trim();
    // F41：大写单 ASCII 字母最优先（必须在 named lowercase 匹配之前——
    // 否则 "W" 被 named "w" 分支吃掉，游戏 sprint 最常发的形态照样静默
    // 小写）。"w"（小写）走 named 分支不包 shift；"W"（大写）= shift+w。
    {
        let mut chars = n.chars();
        let (first, rest) = (chars.next(), chars.as_str());
        if rest.is_empty()
            && let Some(c) = first.filter(|c| c.is_ascii_uppercase())
        {
            return Ok(KeySpec {
                key: c.to_string(),
                code: format!("Key{c}"),
                vk: c as u32,
                shifted: true,
                extra_mods: 0,
            });
        }
    }
    let spec = |key: &str, code: &str, vk: u32| KeySpec {
        key: key.to_string(),
        code: code.to_string(),
        vk,
        shifted: false,
        extra_mods: 0,
    };
    let lower = n.to_ascii_lowercase();
    match lower.as_str() {
        "w" => Ok(spec("w", "KeyW", 87)),
        "a" => Ok(spec("a", "KeyA", 65)),
        "s" => Ok(spec("s", "KeyS", 83)),
        "d" => Ok(spec("d", "KeyD", 68)),
        "enter" | "return" => Ok(spec("Enter", "Enter", 13)),
        "escape" | "esc" => Ok(spec("Escape", "Escape", 27)),
        "space" => Ok(spec(" ", "Space", 32)),
        "tab" => Ok(spec("Tab", "Tab", 9)),
        "backspace" => Ok(spec("Backspace", "Backspace", 8)),
        "delete" => Ok(spec("Delete", "Delete", 46)),
        "arrowleft" | "left" => Ok(spec("ArrowLeft", "ArrowLeft", 37)),
        "arrowright" | "right" => Ok(spec("ArrowRight", "ArrowRight", 39)),
        "arrowup" | "up" => Ok(spec("ArrowUp", "ArrowUp", 38)),
        "arrowdown" | "down" => Ok(spec("ArrowDown", "ArrowDown", 40)),
        "shift" => Ok(spec("Shift", "ShiftLeft", 16)),
        "ctrl" | "control" => Ok(spec("Control", "ControlLeft", 17)),
        "alt" => Ok(spec("Alt", "AltLeft", 18)),
        _ => {
            // 单字符可打印 ASCII 兜底（'5' → VK 53）；大写字母已在函数
            // 头部提前截获（named 分支之前）；其他诚实拒绝。
            let mut chars = n.chars();
            let (c, rest) = (chars.next(), chars.as_str());
            if !rest.is_empty() {
                return Err(format!("不支持的键名: {n}"));
            }
            let c = c.ok_or_else(|| format!("不支持的键名: {n}"))?;
            if c.is_ascii_graphic() {
                Ok(spec(
                    &c.to_string(),
                    &format!("Key{}", c.to_ascii_uppercase()),
                    c.to_ascii_uppercase() as u32,
                ))
            } else {
                Err(format!("不支持的键名: {n}"))
            }
        }
    }
}

async fn dispatch_key_down(conn: &CdpConn, spec: &KeySpec) -> Result<(), String> {
    conn.call(
        "Input.dispatchKeyEvent",
        key_down_payload(spec),
        cdp::CDP_CMD_TIMEOUT,
    )
    .await?;
    Ok(())
}

/// keyDown 载荷构造（F46，2026-11-16 十审：纯函数面；F50 十二审重修 text
/// 判据）。
///
/// text 字段 = char 语义——textarea 换行/文本框字符插入靠它（裸 keydown
/// 只触发 DOM keydown 事件，不落字符：Puppeteer/Playwright 的 Enter 定义
/// 都带 `text:'\r'`，缺失 = Enter 在 textarea 静默不换行）。
///
/// **F50（2026-11-16 十二审，Edge 活体 F49 fail-fast 实证）**：text 是
/// CDP optional **非 nullable** 字段——显式 `"text": null` = Invalid
/// parameters（旧写法 `vk<32 → json!(null)`：Shift/Ctrl/Alt/Tab/Esc/
/// Backspace 全部命令报错，shift 包裹大写键整族瘫痪）；方向键/Delete
/// （vk 37-46）旧写法落进 `32..127` 分支被给成键名字符串 = keypress 事件
/// 携带假字符（text 语义 = 该按键产生的字符，功能键不产生）。现判据 =
/// 单字符 ASCII（可打印或空格）才带 text，Enter 特带 "\r"，其余功能键
/// **省略字段**（Puppeteer 无字符键同款形态）。unmodifiedText = 去 shift
/// 形态（大写 → 小写；Chromium 用它做无修饰键语义推断），同样仅在有
/// text 时出现。
///
/// **F50 第二段（modifiers）**：帧恒带 `modifiers` 位掩码（`modifier_bits`）——
/// 活体实证缺省时页面 `event.shiftKey` 恒 false，shift 包裹只剩字符面。
fn key_down_payload(spec: &KeySpec) -> Value {
    let text: Option<String> = if spec.key == "Enter" {
        Some("\r".to_string())
    } else if let Some(c) = spec
        .key
        .chars()
        .next()
        .filter(|_| spec.key.chars().count() == 1)
        && (c.is_ascii_graphic() || c == ' ')
    {
        Some(spec.key.clone())
    } else {
        None
    };
    let unmodified = match (&text, spec.shifted) {
        (Some(t), true) => Some(t.to_lowercase()),
        (Some(t), false) => Some(t.clone()),
        (None, _) => None,
    };
    let mut v = serde_json::Map::new();
    v.insert("type".into(), json!("keyDown"));
    v.insert("key".into(), json!(spec.key));
    v.insert("code".into(), json!(spec.code));
    v.insert("windowsVirtualKeyCode".into(), json!(spec.vk));
    v.insert("nativeVirtualKeyCode".into(), json!(spec.vk));
    if let Some(t) = text {
        v.insert("text".into(), json!(t));
    }
    if let Some(u) = unmodified {
        v.insert("unmodifiedText".into(), json!(u));
    }
    v.insert("autoRepeat".into(), json!(false));
    v.insert("location".into(), json!(0));
    v.insert("modifiers".into(), json!(modifier_bits(spec)));
    Value::Object(v)
}

/// 修饰键位掩码（CDP Input.dispatchKeyEvent 的 `modifiers`：Alt=1 Ctrl=2
/// Shift=8；键表无 Meta，真支持时按位补 4）。**F50 第二段（2026-11-16
/// 十二审活体实证）**：帧不带 modifiers 时 Chromium 不推断修饰状态——
/// shift 包裹的 Shift/A 帧全缺 modifiers，页面 `event.shiftKey` 恒
/// false（字符面由 text 传达，修饰状态面丢失 = 游戏 sprint/页面快捷键
/// 检测全瞎）；F49 修复后的活体当场抓出。Puppeteer 同场景先置位再发帧。
/// 物理键盘语义：keyup 事件也携带释放前的按下状态——down/up 同公式
/// （Shift up 帧 modifiers=8，页面 keyup:Shift:shift=true 断言钉死）。
fn modifier_bits(spec: &KeySpec) -> u32 {
    let own = match spec.key.as_str() {
        "Shift" => 8,
        "Control" => 2,
        "Alt" => 1,
        _ => 0,
    };
    own | if spec.shifted { 8 } else { 0 } | spec.extra_mods
}

/// F52（2026-11-16 十三轮）：复合键名解析——`ctrl+shift+a` 形态。
///
/// 背景（活体坐实）：修饰状态是 per-call 无状态的——「先 ctrl down 再 a
/// down」的显式按住路径里，a 帧 modifiers 恒 0（页面 event.ctrlKey=false），
/// 组合键（Ctrl+C/V/A、游戏修饰键语义）修饰面传不过去。Puppeteer 单调用
/// 组合（press('KeyC', {ctrl:true})）靠内部 _modifiers 状态；无状态 API 的
/// 对应形态 = 复合键名一次调用表达完整组合。
///
/// 语义：前缀段（可多段）必须是 ctrl/alt/shift（大小写不敏感），尾段 =
/// 基键（走 key_spec 全规则，含大写/WASD/方向键）。base 帧 modifiers =
/// own 位 ∪ shifted ∪ 组合位（页面 ctrlKey/shiftKey 逐位真）；修饰键自身
/// down/up 物理帧照发（own 位）。显式 shift + 小写字母 base → 升级大写
/// 形态（物理字符面一致：shift+a 产出 'A'；shift+符号的布局相关映射不做
/// ——用户应直接发目标字符，诚实边界）。Down/Up 相位遇组合 = 拒绝（修饰
/// 按下语义单调用无法诚实表达，指路 press）。
fn parse_combo(name: &str) -> Result<(Vec<KeySpec>, KeySpec), String> {
    let n = name.trim();
    if !n.contains('+') {
        return Ok((Vec::new(), key_spec(n)?));
    }
    let segs: Vec<&str> = n.split('+').map(str::trim).collect();
    if segs.len() < 2 || segs.iter().any(|s| s.is_empty()) {
        return Err(format!("非法组合键形态: {name}（示例 ctrl+shift+a）"));
    }
    let mut mods = Vec::new();
    let mut shift_in_mods = false;
    for s in &segs[..segs.len() - 1] {
        let m = key_spec(s)?;
        if !matches!(m.key.as_str(), "Shift" | "Control" | "Alt") {
            return Err(format!(
                "组合键前缀段必须是修饰键（ctrl/alt/shift）: {name}"
            ));
        }
        if m.key == "Shift" {
            shift_in_mods = true;
        }
        mods.push(m);
    }
    let mut base = key_spec(segs[segs.len() - 1])?;
    // 显式 shift + 小写单字母 → 升级大写（shifted 路径复用：text/key/e.key
    // 全大写，物理字符面一致）。
    if shift_in_mods {
        let single = base
            .key
            .chars()
            .next()
            .filter(|_| base.key.chars().count() == 1);
        if let Some(c) = single.filter(|c| c.is_ascii_lowercase()) {
            base = key_spec(&c.to_ascii_uppercase().to_string())?;
        }
    }
    base.extra_mods = mods.iter().fold(0u32, |acc, m| {
        acc | match m.key.as_str() {
            "Shift" => 8,
            "Control" => 2,
            "Alt" => 1,
            _ => 0,
        }
    });
    Ok((mods, base))
}

/// keyUp 载荷（纯函数面，与 key_down_payload 对称可测）。
fn key_up_payload(spec: &KeySpec) -> Value {
    json!({
        "type": "keyUp",
        "key": spec.key,
        "code": spec.code,
        "windowsVirtualKeyCode": spec.vk,
        "nativeVirtualKeyCode": spec.vk,
        "modifiers": modifier_bits(spec),
    })
}

async fn dispatch_key_up(conn: &CdpConn, spec: &KeySpec) -> Result<(), String> {
    conn.call(
        "Input.dispatchKeyEvent",
        key_up_payload(spec),
        cdp::CDP_CMD_TIMEOUT,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
