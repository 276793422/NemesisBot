//! 会话编排层（计划 §4）：浏览器进程生命周期（spawn / 探测就绪 / 关闭）+
//! 专用 profile + tab 注册表快照 + 「工作 tab」指针 + AI 操作徽标。
//!
//! 契约（计划 D1/D2/D8/D13）：
//! - 专用 profile：`--user-data-dir=<home>/browser/<kind_slug>/`——Chrome 136
//!   起默认目录上调试旗标被忽略（防附身偷 cookie），专用目录是硬前提；
//!   副产品 = 登录态持久 + 与日常浏览器完全隔离。
//! - 可见窗口（非 headless）：人机共享驾驶，用户直接看真窗口。
//! - estop / gateway 退出**不杀窗口**（明文例外，防后人顺手统一加杀窗）；
//!   关闭只走显式 close()（Dashboard 按钮 / manager）。
//! - 「工作 tab」指针：AI 只看被点名 tab（点名即授权，不做全浏览器扫描）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Local};
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::cdp::{self, CdpConn};
use crate::driver::{BrowserDriver, ChromiumCdpDriver, DriverKind, TabInfo, WebAction};

/// spawn 前等待 DevToolsActivePort 的轮询间隔。
const PORT_POLL_MS: u64 = 250;
/// 关闭时 Browser.close 后等待进程自行退出的上限。
const CLOSE_WAIT_SECS: u64 = 5;

/// 运行中的浏览器会话（`Arc<BrowserSession>` 由 manager 持有）。
pub struct BrowserSession {
    /// 实例 id（"b1"/"b2"…，manager 序号分配）。
    pub id: String,
    /// 实例序号（展示位）。
    pub seq: u32,
    pub kind: DriverKind,
    pub executable: PathBuf,
    /// 专用 profile 目录（cookie 落盘，登录态持久）。
    pub profile_dir: PathBuf,
    pub port: u16,
    pub started_at: DateTime<Local>,
    pub pid: u32,
    driver: ChromiumCdpDriver,
    /// 工作 tab（AI 操作面；None = 未点名）。
    working_tab: Mutex<Option<String>>,
    /// AI 操作中徽标（tabs.list 投影；浏览器内做不到——无扩展）。
    operating: AtomicBool,
    /// 进程已退出标记（reaper 任务置位；is_alive 双保险）。
    exited: Arc<AtomicBool>,
}

impl BrowserSession {
    /// 以调试旗标启动浏览器并探测就绪（阻塞至端点可用或超时诚实报错）。
    pub async fn spawn(
        id: String,
        seq: u32,
        kind: DriverKind,
        executable: PathBuf,
        profile_root: &Path,
    ) -> Result<Arc<Self>, String> {
        let profile_dir = profile_root.join(kind.slug());
        std::fs::create_dir_all(&profile_dir)
            .map_err(|e| format!("创建 profile 目录 {}: {e}", profile_dir.display()))?;

        // F37（2026-11-15 六审）：孤儿收养——gateway 重启后注册表为空，但旧
        // 窗口仍在屏上持有 profile（D13「不杀窗」契约的副产品）。直接 spawn
        // 撞单实例墙：二号进程把 URL 转发给旧实例后自退，被预清理删掉的
        // DevToolsActivePort 永不重写 → 120s 假超时 + 屏上留下不可管理的
        // 孤儿窗口（Dashboard 列不出也关不掉）。spawn 前先一次性探端口文
        // 件：端点活着 = 直接收编（不 spawn；pid 未知 = 0，close 走
        // Browser.close 优雅路，F36 守卫保证强杀兜底对 0 号诚实 no-op）。
        // 必须在端口文件预清理**之前**——否则探的就是自己刚删掉的文件。
        if let Some((port, ws_path)) = cdp::read_devtools_active_port(&profile_dir)
            && cdp::http_version(port).await.is_ok()
        {
            let driver = ChromiumCdpDriver::new(port, kind, ws_path);
            // 初始工作 tab 与正常 spawn 同语义（首个 page target）。
            let working = driver
                .list_tabs()
                .await
                .ok()
                .and_then(|tabs| tabs.first().map(|t| t.id.clone()));
            tracing::info!(
                "[Browser] 收养孤儿浏览器会话（端口 {port}，profile {}）",
                profile_dir.display()
            );
            // 收养会话没有子进程句柄可 reaper——起等价探针：端口死 =
            // 视同退出置位旗标（F27 死当投影对收养会话同样成立；3s
            // 周期一次性 GET；loopback 连接拒绝是可靠的死亡信号）。
            let exited = Arc::new(AtomicBool::new(false));
            {
                let flag = exited.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        if cdp::http_version(port).await.is_err() {
                            flag.store(true, Ordering::Release);
                            break;
                        }
                    }
                });
            }
            return Ok(Arc::new(Self {
                id,
                seq,
                kind,
                executable,
                profile_dir,
                port,
                started_at: Local::now(),
                pid: 0,
                driver,
                working_tab: Mutex::new(working),
                operating: AtomicBool::new(false),
                exited,
            }));
        }

        // 端口文件先清——上一会话的残留会被误读成新端口（本轮 launch 前该
        // profile 必无活进程：manager 按 profile 单实例；有活进程的形态在
        // 上方收养臂已接走）。
        let port_file = profile_dir.join(cdp::DEVTOOLS_PORT_FILE);
        if port_file.exists() {
            let _ = nemesis_utils::fsutil::remove_file_resilient(&port_file);
        }

        let stderr_log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(profile_dir.join("browser-stderr.log"))
            .map_err(|e| format!("打开 stderr 日志: {e}"))?;

        // 旗标集（计划 D1/D2/D3）：动态端口 + 专用 profile + 免首跑引导 +
        // 可见窗口（不加 --headless；不加 --enable-automation——无黄色警告条，
        // 自研客户端语义）。Edge 特有：--edge-skip-compat-relaunch 防 compat
        // relaunch 丢旗标个案。
        let mut args: Vec<String> = vec![
            "--remote-debugging-port=0".to_string(),
            format!("--user-data-dir={}", profile_dir.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--window-size=1400,900".to_string(),
            "about:blank".to_string(),
        ];
        if kind == DriverKind::Edge {
            args.insert(2, "--edge-skip-compat-relaunch".to_string());
        }

        let mut cmd = tokio::process::Command::new(&executable);
        // F51a（2026-11-16 十二审收尾，cargo 挂死 20 分钟活体实证）：三流必须
        // **全部**显式设置——stdin 缺省 inherit 时 std 的 Windows 句柄继承
        // 限制（PROC_THREAD_ATTRIBUTE_HANDLE_LIST）失效，浏览器进程把上层
        // 调用方的输出管道写端一并继承走：headless `run`/脚本驱动 agent 调
        // host_browser 时，NDJSON stdout 管道因 Edge 持有写端永不 EOF，消费
        // 方脚本挂死。stdin(null) + stdout(null) + stderr(日志文件) 三流全脱
        // 后只传显式句柄（Playwright stdio:[ignore,ignore,pipe] 同款纪律）。
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(stderr_log));
        // Windows：控制台形态不弹窗（house 规则）。
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        // env 窄版闸（家族闸纪律：三方进程 spawn 点显式接闸——
        // BUG-20261123-012 横向修齐）：浏览器进程不继承 bot 内部命名
        // 空间变量（NEMESISBOT_VAULT_PASSPHRASE 主口令通道等）。窄剥
        // 不碰 PATH/HOME 等浏览器启动底座。
        let _stripped_env = nemesis_utils::env_sanitize::sanitize_tokio_command_narrow(&mut cmd);

        let child = cmd
            .spawn()
            .map_err(|e| format!("启动浏览器 {}: {e}", executable.display()))?;
        let pid = child.id().unwrap_or(0);
        // F29（2026-11-15 五审）：stale 端口文件闸的判定起点（须早于浏览器
        // 写文件——spawn 前取，新浏览器写入的 mtime 必然晚于此）。
        let spawn_wall_start = std::time::SystemTime::now();

        // 游离 spawn + reaping：reaper 持有 child 等退出置位标记；kill_on_drop
        // 缺省 false——manager/session 生命周期与进程脱钩（gateway 退出不杀窗，
        // D13）。注意：kill_on_drop=false 时 reaper task 泄漏到进程退出，与
        // 「不杀窗」契约一致。
        let exited = Arc::new(AtomicBool::new(false));
        {
            let mut child = child;
            let flag = exited.clone();
            tokio::spawn(async move {
                let _ = child.wait().await;
                flag.store(true, Ordering::Release);
            });
        }

        // 等 DevToolsActivePort（≤ SPAWN_READY_SECS；动态端口标准做法——读
        // 文件拿端口，Playwright/Puppeteer 同款）。三个失败臂都要杀孤儿：
        // 进程已起而装配中断 = 窗口留在屏幕上且 manager 未注册（Dashboard
        // 关闭按钮够不着，2026-11-15 复审 F10 根修）。
        macro_rules! fail {
            ($msg:expr) => {{
                kill_process(pid);
                return Err($msg);
            }};
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(cdp::SPAWN_READY_SECS);
        let (port, ws_path) = loop {
            if exited.load(Ordering::Acquire) {
                let tail = tail_file(&profile_dir.join("browser-stderr.log"), 40);
                fail!(format!(
                    "浏览器进程提前退出（pid {pid}）。stderr 尾部：\n{tail}"
                ));
            }
            // F29（2026-11-15 五审）：stale 端口文件闸——上方预清理 remove
            // 失败（AV/索引器瞬时句柄，Windows 常态）时旧文件残留，会被误读
            // 成新端口 → wait_for_endpoint 撞死端口失败 → 杀掉健康新浏览器
            // + 误导性报错。只认 spawn 起点之后写入的文件；metadata 瞬时
            // sharing violation 按未就绪继续轮询（250ms 后重读，不误杀）。
            if let Some(pp) = cdp::read_devtools_active_port(&profile_dir) {
                let fresh = std::fs::metadata(&port_file)
                    .and_then(|m| m.modified())
                    .map(|t| t >= spawn_wall_start)
                    .unwrap_or(false);
                if fresh {
                    break pp;
                }
            }
            if std::time::Instant::now() >= deadline {
                fail!(format!(
                    "DevToolsActivePort 未在 {}s 内出现（profile: {}；企业策略 RemoteDebuggingAllowed 可能关死了 CDP）",
                    cdp::SPAWN_READY_SECS,
                    profile_dir.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(PORT_POLL_MS)).await;
        };

        if let Err(e) = cdp::wait_for_endpoint(port).await {
            fail!(e);
        }

        let driver = ChromiumCdpDriver::new(port, kind, ws_path);

        // 初始工作 tab = 第一个 page target（spawn 开的 about:blank）；用户
        // 之后随时重点名（D7：人先做，AI 后接管）。
        let working = driver
            .list_tabs()
            .await
            .ok()
            .and_then(|tabs| tabs.first().map(|t| t.id.clone()));

        Ok(Arc::new(Self {
            id,
            seq,
            kind,
            executable,
            profile_dir,
            port,
            started_at: Local::now(),
            pid,
            driver,
            working_tab: Mutex::new(working),
            operating: AtomicBool::new(false),
            exited,
        }))
    }

    pub fn driver(&self) -> &ChromiumCdpDriver {
        &self.driver
    }

    /// 仅测试构造（F27/F28 回归锁）：不走真 spawn。is_alive 语义由 exited
    /// 旗标与端口探测臂决定（exited=true 短路 false；false + 假 HTTP 端口
    /// = true）。
    #[cfg(test)]
    pub(crate) fn test_with(id: &str, kind: DriverKind, exited: bool, port: u16) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            seq: 1,
            kind,
            executable: PathBuf::from("test-browser"),
            profile_dir: std::env::temp_dir().join(format!("nb-browser-test-{id}")),
            port,
            started_at: Local::now(),
            pid: 0,
            driver: ChromiumCdpDriver::new(port, kind, String::new()),
            working_tab: Mutex::new(None),
            operating: AtomicBool::new(false),
            exited: Arc::new(AtomicBool::new(exited)),
        })
    }

    /// AI 操作徽标置位/摘除（act 内 RAII 包裹）。
    pub fn set_operating(&self, on: bool) {
        self.operating.store(on, Ordering::Release);
    }

    pub fn operating(&self) -> bool {
        self.operating.load(Ordering::Acquire)
    }

    /// tab 注册表快照 + 工作指针/附着/加载态徽标投影。
    pub async fn tabs_snapshot(&self) -> Result<Vec<Value>, String> {
        let tabs = self.driver.list_tabs().await?;
        // F31（2026-11-15 五审）：死连接缓存 GC——用户手关 tab 是唯一关闭
        // 路径（九动作工具面无 close_tab），其缓存 conn 永不触发 act 失败
        // 摘除（tab 没了就不会再 act），长寿命会话反复开关 tab = 死 socket
        // 堆积。快照心跳点 retain GC（摘除无害：摘后首次 act/attach 自动
        // 重连几 ms）。
        self.driver
            .retain_conns(&tabs.iter().map(|t| t.id.clone()).collect::<Vec<_>>());
        let working = self.working_tab.lock().clone();
        let mut out = Vec::with_capacity(tabs.len());
        for t in &tabs {
            out.push(tab_project(t, working.as_deref(), self).await);
        }
        Ok(out)
    }

    /// 点名工作 tab（attach：附着 + 激活前置）。目标必须存在于列表（诚实报错）。
    pub async fn set_working_tab(&self, target_id: &str) -> Result<TabInfo, String> {
        let tabs = self.driver.list_tabs().await?;
        let tab = tabs
            .iter()
            .find(|t| t.id == target_id)
            .ok_or_else(|| format!("tab {target_id} 不存在（当前 {} 个 tab）", tabs.len()))?
            .clone();
        let _ = self.driver.activate_tab(target_id).await; // 激活失败不致命
        // 附着（建立页面级连接 + enable）——「已附着」徽标 + 后续命令就绪。
        // 2026-11-15 复审 F11 根修：旧写法 tab_ready_state().ok() 是 no-op
        // （未缓存 tab 直接 Err），attach 后 load_state 恒 unknown 直到首次
        // act——ensure_attached 真建连接；失败 warn 不阻塞点名（tab 在列表
        // 但连接失败的页面照常可点名，首次 act 诚实报错）。
        if let Err(e) = self.driver.ensure_attached(target_id).await {
            tracing::warn!("[Browser] attach 预连接失败（不阻塞点名）: {e}");
        }
        *self.working_tab.lock() = Some(target_id.to_string());
        Ok(tab)
    }

    /// 释放工作 tab（解除点名，不关页面）。
    pub fn clear_working_tab(&self) {
        *self.working_tab.lock() = None;
    }

    pub fn working_tab_id(&self) -> Option<String> {
        self.working_tab.lock().clone()
    }

    fn require_working_tab(&self) -> Result<String, String> {
        self.working_tab.lock().clone().ok_or_else(|| {
            "未点名工作标签页：先由用户在 Dashboard 点选（browser.attach），AI 只操作被点名 tab"
                .to_string()
        })
    }

    /// 在工作 tab 上执行单个动作（AI 操作面入口；operating 徽标包裹）。
    pub async fn act(&self, action: &WebAction) -> Result<Value, String> {
        let tab = self.require_working_tab()?;
        // F34（2026-11-15 六审）：工作 tab 被用户手关 = 死指针。act 撞死
        // target 报传输层错误（ws connect 404 / connection closed），而模型
        // 无自恢复面（attach 点名是 Dashboard 用户专属语义）——不预检则
        // 每次尝试都撞传输层错误死循环。tab 不在存活列表 = 清指针 + 诚实
        // 指引重新点名；list_tabs 自身失败（浏览器整死）不添噪，act 自会
        // 诚实失败。
        if let Ok(tabs) = self.driver.list_tabs().await
            && !tabs.iter().any(|t| t.id == tab)
        {
            self.driver.evict_conn(&tab);
            *self.working_tab.lock() = None;
            return Err(
                    "工作标签页已被关闭（窗口内手动关闭）：请让用户在 Dashboard 重新点选一个标签页（browser.attach）后重试"
                        .to_string(),
                );
        }
        // F35（2026-11-15 六审）：operating 徽标 RAII——act 的 future 被
        // cancel（任务 abort）或 panic 时 set_operating(false) 恒执行，
        // 「AI 操作中」徽标不再永久卡死。
        let _operating = OperatingGuard(self);
        let result = self.driver.act(&tab, action).await;
        if result.is_err() {
            // F13：失败摘连接缓存（tab 手关/半开死后不再每次撞 30s 超时）。
            self.driver.evict_conn(&tab);
        }
        result
    }

    /// 工作 tab 的 location.origin（凭据守卫①）。
    pub async fn working_tab_origin(&self) -> Result<String, String> {
        let tab = self.require_working_tab()?;
        match self.driver.tab_origin(&tab).await {
            Ok(v) => Ok(v),
            Err(e) => {
                self.driver.evict_conn(&tab);
                Err(e)
            }
        }
    }

    /// 工作 tab 上目标元素是否密码框（凭据守卫②）。
    pub async fn working_tab_is_password(&self, selector: &str) -> Result<bool, String> {
        let tab = self.require_working_tab()?;
        match self.driver.is_password_input(&tab, selector).await {
            Ok(v) => Ok(v),
            Err(e) => {
                self.driver.evict_conn(&tab);
                Err(e)
            }
        }
    }

    /// 进程存活探测（HTTP 端点 2s 内应答）。
    pub async fn is_alive(&self) -> bool {
        if self.exited.load(Ordering::Acquire) {
            return false;
        }
        cdp::http_version(self.port).await.is_ok()
    }

    /// 关闭：Browser.close（优雅，落盘 cookie/session）→ 等 CLOSE_WAIT_SECS
    /// → 仍活着按 pid 强杀。estop 与 gateway 退出**不**走这里（D13：不杀窗
    /// 是明文例外）。
    pub async fn close(&self) -> Result<Value, String> {
        // 优雅路：浏览器级 WS 发 Browser.close。
        let graceful = async {
            let path = cdp::read_devtools_active_port(&self.profile_dir)
                .map(|(_, p)| p)
                .unwrap_or_default();
            let conn = CdpConn::connect(self.port, &path).await?;
            conn.call("Browser.close", json!({}), Duration::from_secs(5))
                .await
        };
        let graceful = graceful.await;

        // 等退出。
        let deadline = std::time::Instant::now() + Duration::from_secs(CLOSE_WAIT_SECS);
        while std::time::Instant::now() < deadline {
            if self.exited.load(Ordering::Acquire) || !process_alive(self.pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        // 仍活着 → 强杀兜底（信息性：优雅关闭失败才走）。
        let killed = if self.exited.load(Ordering::Acquire) || !process_alive(self.pid) {
            false
        } else {
            kill_process(self.pid);
            true
        };
        Ok(json!({
            "ok": true,
            "graceful": graceful.is_ok(),
            "killed": killed,
        }))
    }

    /// 会话摘要（列表子页签实例卡数据源）。
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "seq": self.seq,
            "kind": self.kind.slug(),
            "kind_name": self.kind.display_name(),
            "phase2": self.kind.is_phase2(),
            "executable": self.executable.display().to_string(),
            "profile": self.profile_dir.display().to_string(),
            "port": self.port,
            "started_at": self.started_at.to_rfc3339(),
            "pid": self.pid,
            // F27（2026-11-15 五审）：死当投影——用户手关窗口（共享驾驶的
            // 常态操作）后 reaper ms 级置位，summary 同步可读。前端凭此渲染
            // 「已退出」置灰 + 跳过死实例轮询；没有这个字段，死实例卡恒
            // 绿点「运行中」、点名面板 tabs 恒「读取中…」（拉取失败被吞）。
            "alive": !self.exited.load(Ordering::Acquire),
            "working_tab": self.working_tab.lock().clone(),
            "operating": self.operating(),
        })
    }
}

/// tab 行投影（工作指针/附着态/加载态徽标；加载态只问已附着的 tab——
/// 全列表逐 tab 附着开销不可接受，其余诚实「未附着」）。
async fn tab_project(t: &TabInfo, working: Option<&str>, session: &BrowserSession) -> Value {
    let is_working = working == Some(t.id.as_str());
    let load_state = if is_working {
        match session.driver().tab_ready_state(&t.id).await {
            Ok(s) => s,
            Err(_) => {
                // F13 同族（三审补）：快照路径失败同样摘死连接——连接死透时
                // 只摘缓存才能让下轮 2s 跟随重连复活；导航窗口内的瞬时失败
                // 摘后重连自愈（几 ms），比死连接粘着恒 unknown 好。
                session.driver().evict_conn(&t.id);
                "unknown".to_string()
            }
        }
    } else {
        "detached".to_string()
    };
    json!({
        "target_id": t.id,
        "title": t.title,
        "url": t.url,
        "attached": t.attached,
        "working": is_working,
        "load_state": load_state,
        "operating": is_working && session.operating(),
    })
}

/// operating 徽标 RAII 守卫（F35，2026-11-15 六审）：析构恒摘旗标——
/// act future 被 cancel（任务 abort）或 panic 两条非正常退出路径都安全，
/// 「AI 操作中」徽标不再永久卡死（旧写法 set_operating(true)…(false) 在
/// 取消点跳过 false 臂）。
struct OperatingGuard<'a>(&'a BrowserSession);

impl Drop for OperatingGuard<'_> {
    fn drop(&mut self) {
        self.0.set_operating(false);
    }
}

/// 进程是否还活着（强杀前的窗口期判定）。
fn process_alive(pid: u32) -> bool {
    // F36（2026-11-15 六审）：pid=0 是保留值——Windows tasklist 语义查不到
    // （System Idle Process 占 0）；Unix kill(0,0) 语义是「探测本进程组」
    // 恒真。spawn 竞态的 unwrap_or(0) 与收养会话（孤儿收养 pid 未知=0）
    // 都不许被误判成活进程（收养会话靠 exited 探针旗标判活，不走这里）。
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        // BUG-20261122-017：gateway 以 windows 子系统构建，控制台子进程
        // （tasklist）缺 CREATE_NO_WINDOW 会弹黑窗——同文件 kill_process
        // 已有旗标（纪律漂移补齐）。
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&format!("\"{pid}\""))
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        unsafe { kill(pid as i32, 0) == 0 }
    }
}

/// 强杀进程（优雅关闭超时后的兜底）。
fn kill_process(pid: u32) {
    // F36（2026-11-15 六审）：pid=0 禁杀——Unix `kill(0, SIGKILL)` 的语义是
    // 「信号发给整个进程组」= 网关自己被杀（不是杀不动，是杀反了）。可达
    // 路径：spawn 竞态 `child.id().unwrap_or(0)` → fail! 兜底；收养会话
    // （pid=0）close 的强杀兜底臂。0 号一律诚实 no-op。
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
    }
    #[cfg(not(windows))]
    {
        unsafe {
            kill(pid as i32, 9);
        }
    }
}

// 非 Windows 的 kill：只声明 libc 符号（引入 libc crate 为这一个调用不值）。
#[cfg(not(windows))]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// 读文件尾部 N 行（spawn 失败诊断现场）。
///
/// F30（2026-11-15 五审）：字节读 + UTF-8 lossy——旧 `read_to_string` 严格
/// UTF-8，中文 Windows 上浏览器 stderr 是本地代码页（GBK）→ 读取 Err →
/// 尾部静默变空串，「浏览器进程提前退出」恰恰在最需要诊断现场的时刻丢
/// 现场。非 UTF-8 字节 lossy 替换（ASCII 骨架——路径/URL/错误码——保真，
/// 本地化文案可能花屏，好过空）。
fn tail_file(path: &Path, max_lines: usize) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let s = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = s.lines().collect();
            let start = lines.len().saturating_sub(max_lines);
            lines[start..].join("\n")
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests;
