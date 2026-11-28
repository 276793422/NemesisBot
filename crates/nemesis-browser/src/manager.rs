//! BrowserManager：会话注册表 + 操作事件环形缓冲（日志子页签「编排操作路」
//! 数据源）+ 浏览器选择解析（计划 D3/D14/D15）。
//!
//! 选择解析序（D3）：显式配置（prefs.kind / prefs.executable）> Chrome（检测
//! 到 = 用户主动安装的偏好信号）> 平台保底（Windows=Edge 系统组件必在；
//! Linux=chromium）。Firefox 二期（D14）：选中即诚实报错，不进默认序。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::detect::{self, DriverKind};
use crate::session::BrowserSession;

/// 操作事件环形缓冲容量（日志页初始加载回放窗口）。
const EVENT_RING_CAP: usize = 500;

/// 人面哨兵（实例级所有权台账）：Dashboard/WSAPI 无会话身份，统一记
/// "human"——bot 面检查时视为「非本会话」（不可代关），人面自身免检。
pub(crate) const HUMAN_OWNER: &str = "human";

/// 浏览器偏好（config `browser` 段投影；gateway 侧闭包实时读 config——
/// 设置页改完下次 spawn 即生效）。
#[derive(Debug, Clone, Default)]
pub struct BrowserPrefs {
    /// "auto" | "edge" | "chrome" | "brave" | "vivaldi" | "chromium" | "firefox" | "custom"
    pub kind: String,
    /// 自定义可执行文件路径（kind=custom 时必填；其他 kind 为空）。
    pub executable: String,
}

/// 操作事件（编排操作路：spawn/attach/release/close/设置变更；AI 动作路
/// 走 tool_event SSE——两路数据源，计划 D15 日志子页签）。
#[derive(Debug, Clone)]
pub struct BrowserEvent {
    pub ts_ms: i64,
    pub session_id: String,
    /// spawn / attach / release / close / release_tab / detect / config
    pub action: String,
    /// 目标（tab id / kind / 空）。
    pub target: String,
    pub ok: bool,
    pub detail: String,
    pub duration_ms: u64,
}

impl BrowserEvent {
    pub fn to_json(&self) -> Value {
        json!({
            "ts_ms": self.ts_ms,
            "session_id": self.session_id,
            "action": self.action,
            "target": self.target,
            "ok": self.ok,
            "detail": self.detail,
            "duration_ms": self.duration_ms,
        })
    }
}

/// 会话注册表 + 事件面。`Arc<BrowserManager>` 由 gateway 构造、注入
/// SharedResources 与 web AppState（工具面与 WSAPI 面同源）。
pub struct BrowserManager {
    /// home 目录（profile 根 = `<home>/browser/`，计划 Q9）。
    home: PathBuf,
    sessions: Mutex<Vec<Arc<BrowserSession>>>,
    /// 实例级所有权台账（实例 id → 启动者根会话键；2026-11-28 所有权批，
    /// 详见交付报告 2026-11-28_host-browser会话级四动作交付报告 §2）：
    /// 「只能关自己开的」——close_browser 双闸的实例半闸数据源。
    owners: Mutex<HashMap<String, String>>,
    seq: AtomicUsize,
    events: Mutex<VecDeque<BrowserEvent>>,
    /// 实时事件出口（gateway 接 EventHub.publish("browser_event") → SSE）。
    sink: Option<Arc<dyn Fn(&Value) + Send + Sync>>,
    /// 偏好实时读取闭包（读 config `browser` 段——设置页热生效）。
    prefs_getter: Box<dyn Fn() -> BrowserPrefs + Send + Sync>,
}

impl BrowserManager {
    pub fn new(
        home: PathBuf,
        prefs_getter: Box<dyn Fn() -> BrowserPrefs + Send + Sync>,
        sink: Option<Arc<dyn Fn(&Value) + Send + Sync>>,
    ) -> Self {
        Self {
            home,
            sessions: Mutex::new(Vec::new()),
            owners: Mutex::new(HashMap::new()),
            seq: AtomicUsize::new(0),
            events: Mutex::new(VecDeque::with_capacity(EVENT_RING_CAP)),
            sink,
            prefs_getter,
        }
    }

    pub fn profile_root(&self) -> PathBuf {
        self.home.join("browser")
    }

    fn prefs(&self) -> BrowserPrefs {
        (self.prefs_getter)()
    }

    /// 浏览器选择解析（D3 解析序）→ (kind, executable)。Firefox 二期诚实
    /// 报错（kind 臂 + custom 两臂都闸，见 [`gecko_family_rejection`]）；
    /// 全未检出诚实报错（带已探测清单）。
    pub fn resolve_browser(
        &self,
        kind_override: Option<&str>,
        executable_override: Option<&str>,
    ) -> Result<(DriverKind, PathBuf), String> {
        // ① 自定义可执行文件路径最高优先（spawn 覆盖 > 配置）。custom 的
        // 协议假设是 Chromium CDP——Gecko 系可执行名在此诚实拒绝，否则
        // 拿着 Chromium 旗标启动 Firefox（--user-data-dir 它根本不认），
        // 死在莫名其妙的调试端口等待上。
        if let Some(exe) = executable_override.filter(|s| !s.trim().is_empty()) {
            let p = PathBuf::from(exe.trim());
            if !p.is_file() {
                return Err(format!("自定义浏览器路径不存在: {}", p.display()));
            }
            if let Some(err) = gecko_family_rejection(&p) {
                return Err(err);
            }
            return Ok((DriverKind::Custom, p));
        }
        if let Some(k) = kind_override {
            self.resolve_kind(k)
        } else {
            let prefs = self.prefs();
            if !prefs.executable.trim().is_empty() {
                let p = PathBuf::from(prefs.executable.trim());
                if !p.is_file() {
                    return Err(format!("配置的浏览器路径不存在: {}", p.display()));
                }
                if let Some(err) = gecko_family_rejection(&p) {
                    return Err(err);
                }
                return Ok((DriverKind::Custom, p));
            }
            let kind_str = prefs.kind.trim();
            if !kind_str.is_empty() && kind_str != "auto" {
                return self.resolve_kind(kind_str);
            }
            // ② auto：Chrome 检测到 = 偏好信号 > 平台保底。
            if let Some(p) = detect::find_executable(DriverKind::Chrome) {
                return Ok((DriverKind::Chrome, p));
            }
            let fallback = if cfg!(windows) {
                DriverKind::Edge
            } else {
                DriverKind::Chromium
            };
            match detect::find_executable(fallback) {
                Some(p) => Ok((fallback, p)),
                None => Err(format!(
                    "未检测到可用浏览器（已探测 chrome/{}；可在 Dashboard 设置页指定浏览器或自定义路径）",
                    fallback.slug()
                )),
            }
        }
    }

    /// 显式 kind 解析（spawn 覆盖 / 配置钉死）。Firefox 二期诚实报错。
    fn resolve_kind(&self, kind: &str) -> Result<(DriverKind, PathBuf), String> {
        let k = DriverKind::from_slug(kind)
            .ok_or_else(|| format!("未知浏览器 kind: {kind}（值域 edge/chrome/brave/vivaldi/chromium/firefox/auto/custom）"))?;
        if k.is_phase2() {
            return Err(phase2_unavailable_err(k.display_name()));
        }
        match detect::find_executable(k) {
            Some(p) => Ok((k, p)),
            None => Err(format!(
                "{} 未检测到（注册表/已知路径/PATH 三路均未命中）",
                k.display_name()
            )),
        }
    }

    /// spawn 浏览器会话（编排操作路；事件全留痕）。
    ///
    /// `owner`：Some(根会话键) = bot 面（实例 + 初始 tab 绑定到该会话）；
    /// None = 人面（Dashboard/WSAPI，记 "human" 哨兵，免检语义不变）。
    pub async fn spawn(
        &self,
        kind_override: Option<&str>,
        executable_override: Option<&str>,
        owner: Option<&str>,
    ) -> Result<Arc<BrowserSession>, String> {
        let t0 = std::time::Instant::now();
        let (kind, executable) = self.resolve_browser(kind_override, executable_override)?;
        // profile 单实例闸（F16，2026-11-15 三审）：同 kind 活体已在场则
        // 诚实拒绝——profile 目录按 kind 定死（`<home>/browser/<slug>`），
        // 双实例撞同一 user-data-dir（SingletonLock 冲突：二号进程把 URL
        // 转发给一号后自退 = spawn 假失败；锁损坏形态 = cookie 库损坏）。
        // session.rs「本轮 launch 前该 profile 必无活进程」的前提靠这道闸
        // 兑现，此前只是注释声明、无人强制。死会话（窗口被手关）不挡道。
        // 并发边界：双 spawn 精确同时仍可双双过闸（人手双击的窗口是秒级
        // 间隔，检查后串行 spawn 已覆盖现实路径；进程内 spawn 互斥不为
        // 这个极端窗口加锁）。
        for s in self.sessions() {
            if s.kind == kind && s.is_alive().await {
                return Err(format!(
                    "已有 {} 实例（{}）在运行：浏览器 profile 按类型单实例，请先关闭它（Dashboard 实例卡「关闭」，或由启动它的会话 close_browser 收尾）再开新的",
                    kind.display_name(),
                    s.id
                ));
            }
        }
        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("b{n}");
        let owner_norm = owner
            .map(str::to_string)
            .unwrap_or_else(|| HUMAN_OWNER.to_string());
        let result = BrowserSession::spawn(
            id.clone(),
            n as u32,
            kind,
            executable.clone(),
            &self.profile_root(),
            &owner_norm,
        )
        .await;
        match result {
            Ok(session) => {
                self.owners.lock().insert(id.clone(), owner_norm.clone());
                self.sessions.lock().push(session.clone());
                self.record_event(
                    &id,
                    "spawn",
                    kind.slug(),
                    true,
                    &format!(
                        "{} @ 127.0.0.1:{}（owner: {owner_norm}）",
                        executable.display(),
                        session.port
                    ),
                    t0.elapsed().as_millis() as u64,
                );
                Ok(session)
            }
            Err(e) => {
                self.record_event(
                    &id,
                    "spawn",
                    kind.slug(),
                    false,
                    &e,
                    t0.elapsed().as_millis() as u64,
                );
                Err(e)
            }
        }
    }

    /// 会话列表快照（列表子页签实例卡；行带 owner——实例启动者投影）。
    pub fn sessions_snapshot(&self) -> Vec<Value> {
        let owners = self.owners.lock();
        self.sessions
            .lock()
            .iter()
            .map(|s| {
                let mut v = s.summary();
                if let Some(o) = owners.get(&s.id) {
                    v["owner"] = json!(o);
                }
                v
            })
            .collect()
    }

    pub fn session(&self, id: &str) -> Option<Arc<BrowserSession>> {
        self.sessions.lock().iter().find(|s| s.id == id).cloned()
    }

    /// 实例启动者查询（None = 台账无条目——重启前的旧会话或测试构造）。
    pub fn owner_of(&self, id: &str) -> Option<String> {
        self.owners.lock().get(id).cloned()
    }

    /// 全部会话（注册表序；工具侧「缺省 = 第一个活会话」选择面）。
    pub fn sessions(&self) -> Vec<Arc<BrowserSession>> {
        self.sessions.lock().clone()
    }

    /// 缺省会话选择 = 第一个**活**会话（F28，2026-11-15 五审：注册表序
    /// 扫描 is_alive；空/全死两臂诚实报错。工具侧与 WSAPI handler 缺省臂
    /// 统一走这里——此前 handler 缺省臂取注册表第一个、死活不论，用户手
    /// 关窗口后撞死会话报传输层错误，与工具侧「均已退出」文案分裂）。
    pub async fn pick_default_session(&self) -> Result<Arc<BrowserSession>, String> {
        self.pick_default_session_for(None).await.map(|(s, _)| s)
    }

    /// 带所有者偏好的缺省挑选（2026-11-28 所有权批堵洞④；挑选策略是
    /// 浏览器域策略，收进 manager 单一真相——工具面与 WSAPI handler 同源）。
    ///
    /// - `None`（人面）：旧行为 = 第一个活会话。
    /// - `Some(根会话键)`（bot 面）：偏好序 **本会话所启 > human/无主 >
    ///   他会话所启**；落到他会话实例时带诚实注记（调用方应把注记并入
    ///   结果投影）。同 kind 单实例是 profile 架构约束，驾驶层只能共享，
    ///   偏好序是缓解不是隔离。
    pub async fn pick_default_session_for(
        &self,
        caller: Option<&str>,
    ) -> Result<(Arc<BrowserSession>, Option<String>), String> {
        let Some(root) = caller else {
            // 人面：维持旧行为（第一个活会话）。
            let sessions = self.sessions();
            for s in &sessions {
                if s.is_alive().await {
                    return Ok((s.clone(), None));
                }
            }
            return Err(self.no_alive_session_err(&sessions));
        };
        let sessions = self.sessions();
        // guard 作用域块（生成器布局分析对 drop-before-await 的误报解法：
        // 内层块封死 guard 逃逸路径，await 全在块外）。
        let (own, neutral, others) = {
            let owners = self.owners.lock();
            let mut own = Vec::new();
            let mut neutral = Vec::new();
            let mut others = Vec::new();
            for s in sessions {
                let holder = owners.get(&s.id).map(String::as_str);
                if holder == Some(root) {
                    own.push(s);
                } else if holder.is_none() || holder == Some(HUMAN_OWNER) {
                    neutral.push(s);
                } else {
                    others.push((s, holder.unwrap_or(HUMAN_OWNER).to_string()));
                }
            }
            (own, neutral, others)
        };
        for s in own.into_iter().chain(neutral) {
            if s.is_alive().await {
                return Ok((s, None));
            }
        }
        // 末选：他会话所启的活实例（带诚实注记；none = 该序全死/为空）。
        for (s, holder) in &others {
            if s.is_alive().await {
                return Ok((
                    s.clone(),
                    Some(format!(
                        "默认接管了会话 {holder} 启动的实例 {}（非本会话启动：操作前建议先用 tabs 确认现场）",
                        s.id
                    )),
                ));
            }
        }
        Err(self.no_alive_session_err(&self.sessions()))
    }

    /// 空/全死两臂的诚实报错（双向指路：bot 自 spawn 或人面 Dashboard）。
    fn no_alive_session_err(&self, sessions: &[Arc<BrowserSession>]) -> String {
        if sessions.is_empty() {
            "无浏览器会话：可用 host_browser 的 spawn 动作自行启动（实例与初始标签页自动绑到本会话），或请用户在 Dashboard（执行单元 → 浏览器）手动启动"
                .to_string()
        } else {
            "浏览器会话均已退出：可用 host_browser 的 spawn 动作重新启动，或请用户在 Dashboard（执行单元 → 浏览器）重启"
                .to_string()
        }
    }

    /// 关闭会话并从注册表摘除（Dashboard 关闭按钮；estop/gateway 退出不走）。
    ///
    /// `caller`：Some(根会话键) = bot 面，**双闸**（2026-11-28 所有权批
    /// 堵洞③）——① caller 是实例启动者；② 实例内全部活 tab 均为 caller
    /// 所有（防关 A 的实例连带扫掉窗口里别人的页面）。任一不满足 → 拒绝
    /// + 异己清单 + Dashboard 指路。None = 人面免检（Dashboard 语义不变）。
    pub async fn close_session(&self, id: &str, caller: Option<&str>) -> Result<Value, String> {
        let t0 = std::time::Instant::now();
        let session = self
            .session(id)
            .ok_or_else(|| format!("会话 {id} 不存在"))?;
        if let Some(root) = caller {
            // ① 实例半闸：caller 是启动者（台账 human/无条目都算异己）。
            let holder = self.owner_of(id);
            if holder.as_deref() != Some(root) {
                let err = format!(
                    "浏览器实例 {id} 由 {} 启动，本会话无权关闭：只关自己开的；请对方自行 close_browser 收尾，或由人在 Dashboard 关闭",
                    holder.as_deref().unwrap_or("未知会话"),
                );
                self.record_event(
                    id,
                    "close",
                    "",
                    false,
                    &err,
                    t0.elapsed().as_millis() as u64,
                );
                return Err(err);
            }
            // ② 连坐半闸：全部活 tab 归 caller；异己清单进报错。
            let foreign = session.foreign_tabs(caller).await;
            if !foreign.is_empty() {
                let rows = foreign
                    .iter()
                    .map(|f| {
                        format!(
                            "「{}」{}（owner: {}）",
                            f["title"].as_str().unwrap_or(""),
                            f["url"].as_str().unwrap_or(""),
                            f["owner"].as_str().unwrap_or("无主"),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("；");
                let err = format!(
                    "浏览器实例 {id} 内还有非本会话的标签页，关闭会连带扫掉它们，已拒绝：{rows}；请对方自行处理，或由人在 Dashboard 关闭",
                );
                self.record_event(
                    id,
                    "close",
                    "",
                    false,
                    &err,
                    t0.elapsed().as_millis() as u64,
                );
                return Err(err);
            }
        }
        let result = session.close().await;
        // 摘除 + 收缩序号占用（不重用 id——序号单调递增）；台账同步摘条目。
        self.sessions.lock().retain(|s| s.id != id);
        self.owners.lock().remove(id);
        match &result {
            Ok(v) => self.record_event(
                id,
                "close",
                "",
                true,
                &v.to_string(),
                t0.elapsed().as_millis() as u64,
            ),
            Err(e) => self.record_event(id, "close", "", false, e, t0.elapsed().as_millis() as u64),
        }
        result
    }

    /// 点名工作 tab（Dashboard 标签页表 / 点名面板；编排留痕与 close 同规格
    /// ——成败两臂都进环形缓冲 + SSE，设置页 hint「attach 全留痕」的兑现）。
    pub async fn attach_tab(&self, id: &str, target_id: &str) -> Result<Value, String> {
        let t0 = std::time::Instant::now();
        let session = self
            .session(id)
            .ok_or_else(|| format!("会话 {id} 不存在"))?;
        let result = session
            .set_working_tab(target_id)
            .await
            .map(|tab| serde_json::json!({ "session_id": session.id, "tab": tab }));
        match &result {
            Ok(v) => self.record_event(
                id,
                "attach",
                target_id,
                true,
                &v.to_string(),
                t0.elapsed().as_millis() as u64,
            ),
            Err(e) => self.record_event(
                id,
                "attach",
                target_id,
                false,
                e,
                t0.elapsed().as_millis() as u64,
            ),
        }
        result
    }

    /// 释放工作 tab（解除点名不关页面；编排留痕与 close 同规格）。
    pub fn release_tab(&self, id: &str) -> Result<Value, String> {
        let session = self
            .session(id)
            .ok_or_else(|| format!("会话 {id} 不存在"))?;
        session.clear_working_tab();
        self.record_event(id, "release", "", true, "释放工作 tab", 0);
        Ok(serde_json::json!({ "session_id": session.id, "released": true }))
    }

    /// 操作事件记录（环形缓冲 + 实时 sink）。
    pub fn record_event(
        &self,
        session_id: &str,
        action: &str,
        target: &str,
        ok: bool,
        detail: &str,
        duration_ms: u64,
    ) {
        let ev = BrowserEvent {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            session_id: session_id.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            ok,
            detail: detail.to_string(),
            duration_ms,
        };
        {
            let mut ring = self.events.lock();
            if ring.len() >= EVENT_RING_CAP {
                ring.pop_front();
            }
            ring.push_back(ev.clone());
        }
        if let Some(sink) = &self.sink {
            sink(&ev.to_json());
        }
    }

    /// 日志查询（`browser.logs` WSAPI；session_id=None = 全部）。
    pub fn logs(&self, session_id: Option<&str>, limit: usize) -> Vec<Value> {
        let ring = self.events.lock();
        ring.iter()
            .filter(|e| session_id.is_none_or(|sid| e.session_id == sid))
            .rev()
            .take(limit.max(1))
            .map(|e| e.to_json())
            .collect()
    }

    /// 枚举系统已装浏览器（`browser.detect` WSAPI 数据源）。
    pub fn detect(&self) -> Vec<detect::DetectedBrowser> {
        self.record_event("b0", "detect", "", true, "枚举系统浏览器", 0);
        detect::detect_installed()
    }
}

/// 二期驱动不可用文案（resolve_kind 臂与 custom 路径 Gecko 守卫共用——
/// 单一真相源，两处口径漂移时用户看到两种说法）。
fn phase2_unavailable_err(display: &str) -> String {
    format!(
        "{display} 驱动为二期（WebDriver BiDi，P4 落地后转可用）；当前请选 Chromium 家族浏览器（Edge/Chrome/Brave/Vivaldi/Chromium）"
    )
}

/// Gecko 系已知可执行名（file_stem 小写）：Firefox 官方形态 + 常见 Gecko
/// 分支（LibreWolf/Waterfox/PaleMoon）。显式 denylist 而非启发式——名单外
/// 的自定义路径照常按 Chromium CDP 假设放行（custom 的合同就是 Chromium
/// 协议浏览器，Chromium 分支 exe 名五花八门，启发式会误伤）。
const GECKO_FAMILY_STEMS: [&str; 6] = [
    "firefox",
    "firefox-bin",
    "librewolf",
    "waterfox",
    "waterfox-bin",
    "palemoon",
];

/// custom 路径 Gecko 系守卫：命中返回二期诚实报错（提前避免「拿着
/// Chromium 旗标启动 Firefox → 调试端口等待超时」的莫名失败——修复在
/// 解析层，CLI/WSAPI/工具/Dashboard 设置页四个入口同源兜住）。
fn gecko_family_rejection(executable: &std::path::Path) -> Option<String> {
    let stem = executable.file_stem()?.to_str()?.to_lowercase();
    if GECKO_FAMILY_STEMS.contains(&stem.as_str()) {
        Some(format!(
            "自定义路径指向 Gecko 系浏览器（{stem}）：{}",
            phase2_unavailable_err("Firefox 及其分支")
        ))
    } else {
        None
    }
}

/// spawn 后端口文件清理辅助（session 内已做；此处仅为 manager 层单测锚）。
pub fn cleanup_stale_port_file(profile_dir: &std::path::Path) {
    let p = profile_dir.join(crate::cdp::DEVTOOLS_PORT_FILE);
    if p.exists() {
        let _ = nemesis_utils::fsutil::remove_file_resilient(p);
    }
}

/// 日志页默认拉取条数。
pub const DEFAULT_LOG_LIMIT: usize = 200;

/// 等待会话就绪的辅助（WSAPI spawn 返回前已就绪；此处为测试/工具复用）。
pub async fn wait_session_alive(session: &BrowserSession, secs: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if session.is_alive().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[cfg(test)]
mod tests;
