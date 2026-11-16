//! BrowserManager：会话注册表 + 操作事件环形缓冲（日志子页签「编排操作路」
//! 数据源）+ 浏览器选择解析（计划 D3/D14/D15）。
//!
//! 选择解析序（D3）：显式配置（prefs.kind / prefs.executable）> Chrome（检测
//! 到 = 用户主动安装的偏好信号）> 平台保底（Windows=Edge 系统组件必在；
//! Linux=chromium）。Firefox 二期（D14）：选中即诚实报错，不进默认序。

use std::collections::VecDeque;
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
    /// 报错；全未检出诚实报错（带已探测清单）。
    pub fn resolve_browser(
        &self,
        kind_override: Option<&str>,
        executable_override: Option<&str>,
    ) -> Result<(DriverKind, PathBuf), String> {
        // ① 自定义可执行文件路径最高优先（spawn 覆盖 > 配置）。
        if let Some(exe) = executable_override.filter(|s| !s.trim().is_empty()) {
            let p = PathBuf::from(exe.trim());
            if !p.is_file() {
                return Err(format!("自定义浏览器路径不存在: {}", p.display()));
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
            return Err(format!(
                "{} 驱动为二期（WebDriver BiDi，P4 落地后转可用）；当前请选 Chromium 家族浏览器（Edge/Chrome/Brave/Vivaldi/Chromium）",
                k.display_name()
            ));
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
    pub async fn spawn(
        &self,
        kind_override: Option<&str>,
        executable_override: Option<&str>,
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
                    "已有 {} 实例（{}）在运行：浏览器 profile 按类型单实例，请先关闭它（Dashboard 实例卡「关闭」）再开新的",
                    kind.display_name(),
                    s.id
                ));
            }
        }
        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("b{n}");
        let result = BrowserSession::spawn(
            id.clone(),
            n as u32,
            kind,
            executable.clone(),
            &self.profile_root(),
        )
        .await;
        match result {
            Ok(session) => {
                self.sessions.lock().push(session.clone());
                self.record_event(
                    &id,
                    "spawn",
                    kind.slug(),
                    true,
                    &format!("{} @ 127.0.0.1:{}", executable.display(), session.port),
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

    /// 会话列表快照（列表子页签实例卡）。
    pub fn sessions_snapshot(&self) -> Vec<Value> {
        self.sessions.lock().iter().map(|s| s.summary()).collect()
    }

    pub fn session(&self, id: &str) -> Option<Arc<BrowserSession>> {
        self.sessions.lock().iter().find(|s| s.id == id).cloned()
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
        let sessions = self.sessions();
        for s in &sessions {
            if s.is_alive().await {
                return Ok(s.clone());
            }
        }
        if sessions.is_empty() {
            Err(
                "无浏览器会话：请在 Dashboard（执行单元 → 浏览器）先启动浏览器，再调用本工具"
                    .to_string(),
            )
        } else {
            Err("浏览器会话均已退出：请在 Dashboard（执行单元 → 浏览器）重新启动后再试".to_string())
        }
    }

    /// 关闭会话并从注册表摘除（Dashboard 关闭按钮；estop/gateway 退出不走）。
    pub async fn close_session(&self, id: &str) -> Result<Value, String> {
        let t0 = std::time::Instant::now();
        let session = self
            .session(id)
            .ok_or_else(|| format!("会话 {id} 不存在"))?;
        let result = session.close().await;
        // 摘除 + 收缩序号占用（不重用 id——序号单调递增）。
        self.sessions.lock().retain(|s| s.id != id);
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

/// spawn 后端口文件清理辅助（session 内已做；此处仅为 manager 层单测锚）。
pub fn cleanup_stale_port_file(profile_dir: &std::path::Path) {
    let p = profile_dir.join(crate::cdp::DEVTOOLS_PORT_FILE);
    if p.exists() {
        let _ = std::fs::remove_file(p);
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
