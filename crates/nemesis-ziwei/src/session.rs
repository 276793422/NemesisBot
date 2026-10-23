//! session —— 会话管理器（T1.3）：attach 编排 / 状态机 / detach 卸载 / estop。
//!
//! 状态机（总控 §5）：`Injecting → Handshaking → Ready ⇄ Busy → Unloading`
//! （任一步失败 → 会话不入表，失败原因透出给调用方；入表即 Ready）。
//!
//! attach 时序（防握手窗口错配——payload 只重试连 10s）：
//! 1. 先建管道 server（NamedPipeServer 落定）；
//! 2. `tokio::join!`（等握手， spawn_blocking 注入）——payload 连回与注入
//!    并行，两个 15/20s 窗口互不先死；
//! 3. 握手四重校验：win32 客户端 PID == 目标 PID、令牌、协议版本、架构。
//!
//! 槽位纪律：`pending` 集合占位（注入窗口内并发 attach / detach 看到已占
//! 用），成功才入 `sessions`；失败清 pending。避免占位 Session 携带半成品
//! PipeLink。
//!
//! 全局槽（`SESSION_MANAGER`）：AppState 有 69 处测试字面量不可扩字段——
//! L1 COMMANDS_REGISTRY 同款 OnceLock 全局，gateway 装配时 set。

#[cfg(windows)]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};

    use nemesis_ziwei_profile::{DataStructure, Profile};

    use crate::payload_file::ensure_payload;
    use crate::pipe::{PipeError, PipeLink};

    /// 单会话超时预算（read_mem/卸载应答）。
    pub(super) const CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
    /// estop 全量卸载总预算。
    const ESTOP_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

    /// 会话状态（总控 §5）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SessionState {
        Injecting,
        Handshaking,
        Ready,
        Busy,
        Unloading,
        Failed(String),
    }

    impl SessionState {
        pub fn as_str(&self) -> &str {
            match self {
                SessionState::Injecting => "injecting",
                SessionState::Handshaking => "handshaking",
                SessionState::Ready => "ready",
                SessionState::Busy => "busy",
                SessionState::Unloading => "unloading",
                SessionState::Failed(_) => "failed",
            }
        }
    }

    /// 一个已附加的目标会话。
    pub struct Session {
        pub profile_id: String,
        pub profile_name: String,
        pub pid: u32,
        pub arch: String,
        pub pipe_name: String,
        pub attached_at: chrono::DateTime<chrono::Utc>,
        pub target_exe: String,
        /// 结构名 → 运行时地址（attach 注入；M3 起由锚解析自动化）。
        struct_addrs: HashMap<String, u64>,
        /// 函数键（语义名 / `{语义}::chain{i}`）→ 运行时地址（M4 resolve 引擎）。
        fn_addrs: Mutex<HashMap<String, u64>>,
        /// 函数键 → 锚点地址（resolve 产出；hook 补丁点=nop 垫所在）。
        /// 与 fn_addrs 分表：调用需函数入口（执行序章），hook 需锚点（窃取
        /// 窗口）——夹具两者可不同（锚在编译器序章之后）。
        fn_anchors: Mutex<HashMap<String, u64>>,
        /// 在场 hook 绑定（id ↔ 语义名 ↔ 地址；M4 hook 订阅账本）。
        hooks: Mutex<Vec<HookBinding>>,
        /// poll 任务句柄（detach / estop 时 abort）。
        poll_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
        state: Mutex<SessionState>,
        link: tokio::sync::Mutex<PipeLink>,
    }

    /// 在场 hook 绑定（T4.2/T4.6：主函数与链步骤共用一本账）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct HookBinding {
        pub id: u32,
        pub semantic: String,
        pub addr: u64,
    }

    /// payload 侧 hook 槽上限（同函数单点 + 最多 8 个补丁点）。
    pub const HOOK_CAP: u32 = 8;

    impl Session {
        pub fn state(&self) -> SessionState {
            self.state.lock().unwrap().clone()
        }
        pub(crate) fn set_state(&self, s: SessionState) {
            *self.state.lock().unwrap() = s;
        }
        /// 会话链路（read_mem/invoke 路由；串行互斥——payload 命令循环单线程）。
        pub(crate) fn link(&self) -> &tokio::sync::Mutex<PipeLink> {
            &self.link
        }
        /// 结构地址查表（调度器/invoke 用；缺失 = None = 诚实未解析）。
        pub fn struct_addr(&self, name: &str) -> Option<u64> {
            self.struct_addrs.get(name).copied()
        }
        /// 函数地址查表（call_fn/hook 用；缺失 = None = 先 resolve）。
        pub fn fn_addr(&self, key: &str) -> Option<u64> {
            self.fn_addrs.lock().unwrap().get(key).copied()
        }
        /// 函数地址整表替换（resolve 批解析后原子入册）。
        pub fn set_fn_addrs(&self, map: HashMap<String, u64>) {
            *self.fn_addrs.lock().unwrap() = map;
        }
        /// 锚点地址查表（hook 用；缺失 = None = 先 resolve）。
        pub fn fn_anchor(&self, key: &str) -> Option<u64> {
            self.fn_anchors.lock().unwrap().get(key).copied()
        }
        /// 锚点地址整表替换（resolve 批解析后原子入册；仅 resolve 写）。
        pub fn set_fn_anchors(&self, map: HashMap<String, u64>) {
            *self.fn_anchors.lock().unwrap() = map;
        }
        /// 当前已解析函数键列表。
        pub fn fn_keys(&self) -> Vec<String> {
            let mut keys: Vec<String> = self.fn_addrs.lock().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        }
        /// 在场 hook 绑定快照。
        pub fn hooks(&self) -> Vec<HookBinding> {
            self.hooks.lock().unwrap().clone()
        }
        /// 分配 hook id（1..=HOOK_CAP；复用已释放槽位；满 = None）。
        fn alloc_hook_id(&self) -> Option<u32> {
            let hooks = self.hooks.lock().unwrap();
            (1..=HOOK_CAP).find(|&id| hooks.iter().all(|h| h.id != id))
        }
        fn record_hook(&self, binding: HookBinding) {
            self.hooks.lock().unwrap().push(binding);
        }
        fn remove_hook_by_semantic(&self, semantic: &str) -> Option<HookBinding> {
            let mut hooks = self.hooks.lock().unwrap();
            let pos = hooks.iter().position(|h| h.semantic == semantic)?;
            Some(hooks.remove(pos))
        }
        /// 存 poll 任务句柄（attach_and_schedule 装配，一次）。
        pub fn set_poll_tasks(&self, handles: Vec<tokio::task::JoinHandle<()>>) {
            *self.poll_tasks.lock().unwrap() = handles;
        }
        /// abort 全部 poll 任务（detach / estop；同步、不等待任务退出）。
        pub fn abort_poll_tasks(&self) {
            for h in self.poll_tasks.lock().unwrap().drain(..) {
                h.abort();
            }
        }
        /// 状态快照（status 用）。
        pub fn snapshot(&self) -> serde_json::Value {
            serde_json::json!({
                "profile_id": self.profile_id,
                "profile_name": self.profile_name,
                "pid": self.pid,
                "arch": self.arch,
                "target_exe": self.target_exe,
                "state": self.state().as_str(),
                "attached_at": self.attached_at.to_rfc3339(),
                "fn_resolved": self.fn_addrs.lock().unwrap().len(),
                "hooks": self.hooks.lock().unwrap().len(),
            })
        }
    }

    // link（内含管道句柄）不入 Debug——日志/unwrap_err 场景只需身份与状态。
    impl std::fmt::Debug for Session {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Session")
                .field("profile_id", &self.profile_id)
                .field("pid", &self.pid)
                .field("arch", &self.arch)
                .field("state", &self.state())
                .finish_non_exhaustive()
        }
    }

    /// 会话管理器。
    pub struct SessionManager {
        sessions: Mutex<HashMap<String, Arc<Session>>>,
        pending: Mutex<HashSet<String>>,
        workspace: PathBuf,
        payload_dir: Option<String>,
        max_sessions: u32,
        /// 报警事件汇（gateway 装配；None = 不调度 poll）。
        event_sink: Mutex<Option<crate::scheduler::EventSink>>,
        /// capture 回流存储（workspace/ziwei/runs）。
        capture: Arc<crate::capture::CaptureStore>,
    }

    /// attach 失败（诚实分类透出）。
    #[derive(Debug, thiserror::Error)]
    pub enum AttachError {
        #[error("会话已存在（profile {0}），先 detach 再 attach")]
        AlreadyAttached(String),
        #[error(
            "会话数已达上限（{max}）：每 profile 一个常驻执行引擎，可调大 config ziwei.max_sessions"
        )]
        TooManySessions { max: u32 },
        #[error("目标进程不存在（pid={pid}）")]
        ProcessAbsent { pid: u32 },
        #[error("{0}")]
        Other(String),
    }

    impl SessionManager {
        pub fn new(
            workspace: &std::path::Path,
            payload_dir: Option<String>,
            max_sessions: u32,
        ) -> Self {
            SessionManager {
                sessions: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashSet::new()),
                workspace: workspace.to_path_buf(),
                payload_dir,
                max_sessions: max_sessions.max(1),
                event_sink: Mutex::new(None),
                capture: Arc::new(crate::capture::CaptureStore::new(workspace)),
            }
        }

        /// 装配报警事件汇（gateway 启动时；后到覆盖先到）。
        pub fn set_event_sink(&self, sink: crate::scheduler::EventSink) {
            *self.event_sink.lock().unwrap() = Some(sink);
        }

        /// 当前事件汇（attach_and_schedule 判断是否调度 poll）。
        pub fn event_sink(&self) -> Option<crate::scheduler::EventSink> {
            self.event_sink.lock().unwrap().clone()
        }

        /// capture 存储（WSAPI 会话视图读回流数据用）。
        pub fn capture(&self) -> Arc<crate::capture::CaptureStore> {
            self.capture.clone()
        }

        /// 工作区根（动态工具桥现读 profile 用）。
        pub fn workspace(&self) -> &std::path::Path {
            &self.workspace
        }

        /// 当前会话数。
        pub fn session_count(&self) -> usize {
            self.sessions.lock().unwrap().len()
        }

        /// 状态快照（WSAPI ziwei.status / sessions.list）。
        pub fn list_sessions(&self) -> Vec<serde_json::Value> {
            let map = self.sessions.lock().unwrap();
            let mut rows: Vec<_> = map.values().map(|s| s.snapshot()).collect();
            rows.sort_by(|a, b| {
                a["profile_id"]
                    .as_str()
                    .unwrap_or("")
                    .cmp(b["profile_id"].as_str().unwrap_or(""))
            });
            rows
        }

        /// 附加：profile × pid → 注入 → 握手 → Ready（T1.3；无结构地址注入）。
        pub async fn attach(
            &self,
            profile: &Profile,
            pid: u32,
        ) -> Result<Arc<Session>, AttachError> {
            self.attach_with_struct_addrs(profile, pid, HashMap::new())
                .await
        }

        /// 附加 + 结构地址注入（`struct:Name` → 运行时地址；M3 前由调用方
        /// 显式提供，M3 起锚解析自动化）。
        pub async fn attach_with_struct_addrs(
            &self,
            profile: &Profile,
            pid: u32,
            struct_addrs: HashMap<String, u64>,
        ) -> Result<Arc<Session>, AttachError> {
            // T5.1 状态机闸：非 Active 一律拒绝注入（stale → 找回引导）。
            crate::verify::pre_attach_guard(profile).map_err(AttachError::Other)?;
            let key = profile.profile_id.clone();
            {
                let mut pending = self.pending.lock().unwrap();
                let map = self.sessions.lock().unwrap();
                if map.contains_key(&key) || pending.contains(&key) {
                    return Err(AttachError::AlreadyAttached(key));
                }
                if map.len() + pending.len() >= self.max_sessions as usize {
                    return Err(AttachError::TooManySessions {
                        max: self.max_sessions,
                    });
                }
                pending.insert(key.clone());
            }
            let result = self.attach_inner(profile, pid, struct_addrs).await;
            match result {
                Ok(session) => {
                    self.pending.lock().unwrap().remove(&key);
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(key.clone(), session.clone());
                    tracing::info!(
                        "[ziwei] 会话就绪：profile={} pid={} arch={} pipe={}",
                        profile.profile_id,
                        session.pid,
                        session.arch,
                        session.pipe_name
                    );
                    Ok(session)
                }
                Err(e) => {
                    self.pending.lock().unwrap().remove(&key);
                    tracing::warn!(
                        "[ziwei] attach 失败（profile={} pid={}）：{e}",
                        profile.profile_id,
                        pid
                    );
                    Err(e)
                }
            }
        }

        async fn attach_inner(
            &self,
            profile: &Profile,
            pid: u32,
            struct_addrs: HashMap<String, u64>,
        ) -> Result<Arc<Session>, AttachError> {
            if !process_alive(pid) {
                return Err(AttachError::ProcessAbsent { pid });
            }
            let arch = profile.target.machine.payload_suffix().to_string();

            // payload 落盘（T1.6：内嵌 first-use 写 / payload_dir 覆盖）
            let payload = ensure_payload(&self.workspace, self.payload_dir.as_deref(), &arch)
                .map_err(|e| AttachError::Other(e.to_string()))?;

            // 管道名 + 令牌
            let rand6: String = {
                use rand::Rng;
                let mut rng = rand::thread_rng();
                (0..6).map(|_| rng.gen_range(b'a'..=b'z') as char).collect()
            };
            let pipe_name = format!(r"\\.\pipe\ZiWei_{pid}_{rand6}");
            let token: String = {
                use rand::RngCore;
                let mut buf = [0u8; 16];
                rand::rngs::OsRng.fill_bytes(&mut buf);
                buf.iter().map(|b| format!("{b:02x}")).collect()
            };

            // 先建 server（payload 连接重试窗口只有 10s，server 必须先落定）
            let server = crate::pipe::create_server(&pipe_name)
                .map_err(|e| AttachError::Other(format!("管道创建失败（{pipe_name}）：{e}")))?;

            // 注入（阻塞 WinAPI → spawn_blocking）与握手并行
            let inject_params = nemesis_injector::ziwei_attach::ZiweiAttachParams {
                pid,
                dll_path: payload.path.clone(),
                token: token.clone(),
                pipe_name: pipe_name.clone(),
            };
            let inject = async {
                let outcome = tokio::task::spawn_blocking(move || {
                    nemesis_injector::ziwei_attach::attach_inject(&inject_params)
                })
                .await
                .map_err(|e| AttachError::Other(format!("注入任务 join 失败: {e}")))?;
                outcome.map_err(|e| AttachError::Other(e.to_string()))
            };
            let (hs, outcome) = tokio::join!(
                crate::pipe::finish_handshake(server, &token, pid, &arch),
                inject
            );
            outcome.map_err(|e| {
                if hs.is_err() {
                    AttachError::Other(format!("{e}（握手同时未完成：注入失败即无连回，属预期）"))
                } else {
                    e
                }
            })?;
            let link = hs.map_err(|e| {
                // 握手失败但注入可能已成功（payload 驻留无主）——诚实报错；
                // 无管道无法远程卸载，目标进程退出即随进程消失。
                let msg = match &e {
                    PipeError::ConnectTimeout(_) => {
                        format!("payload 未在窗口内连回（注入可能失败或被拦截）：{e}")
                    }
                    PipeError::TokenMismatch | PipeError::PidMismatch { .. } => {
                        format!("握手身份校验失败（疑似外来进程抢连，已断开）：{e}")
                    }
                    _ => format!("握手失败：{e}"),
                };
                AttachError::Other(msg)
            })?;

            let session = Arc::new(Session {
                profile_id: profile.profile_id.clone(),
                profile_name: profile.name.clone(),
                pid,
                arch,
                pipe_name,
                attached_at: chrono::Utc::now(),
                target_exe: profile.target.exe_name.clone(),
                struct_addrs,
                fn_addrs: Mutex::new(HashMap::new()),
                fn_anchors: Mutex::new(HashMap::new()),
                hooks: Mutex::new(Vec::new()),
                poll_tasks: Mutex::new(Vec::new()),
                state: Mutex::new(SessionState::Ready),
                link: tokio::sync::Mutex::new(link),
            });
            Ok(session)
        }

        /// 卸载会话：发 unload 应答后自卸（payload FreeLibraryAndExitThread）。
        pub async fn detach(&self, profile_id: &str) -> Result<String, String> {
            let session = {
                let mut map = self.sessions.lock().unwrap();
                map.remove(profile_id)
                    .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?
            };
            session.set_state(SessionState::Unloading);
            session.abort_poll_tasks(); // 先停调度，避免卸载窗口读竞态
            let mut link = session.link().lock().await;
            let resp =
                tokio::time::timeout(CMD_TIMEOUT, link.roundtrip("{\"cmd\":\"unload\"}")).await;
            match resp {
                Ok(Ok(resp)) if resp.contains("\"ok\":true") => {
                    let msg = format!(
                        "已卸载（profile={profile_id} pid={}）；payload 已 FreeLibrary 自卸",
                        session.pid
                    );
                    tracing::info!("[ziwei] {msg}");
                    Ok(msg)
                }
                Ok(Ok(resp)) => {
                    let msg =
                        format!("卸载应答异常（profile={profile_id}）：{resp}；payload 可能仍驻留");
                    tracing::warn!("[ziwei] {msg}");
                    Err(msg)
                }
                Ok(Err(PipeError::Closed)) => {
                    // payload 已先走（FreeLibrary 断管）——视为已卸
                    let msg = format!(
                        "已卸载（profile={profile_id} pid={}；对端先断管=自卸完成）",
                        session.pid
                    );
                    tracing::info!("[ziwei] {msg}");
                    Ok(msg)
                }
                Ok(Err(e)) => {
                    let msg = format!("卸载失败（profile={profile_id}）：{e}；payload 可能仍驻留");
                    tracing::warn!("[ziwei] {msg}");
                    Err(msg)
                }
                Err(_) => {
                    let msg = format!(
                        "卸载超时（profile={profile_id}，{}s）：payload 可能仍驻留（进程退出即随进程消失）",
                        CMD_TIMEOUT.as_secs()
                    );
                    tracing::warn!("[ziwei] {msg}");
                    Err(msg)
                }
            }
        }

        /// 取会话（read_mem/invoke 路由用）。
        pub fn get(&self, profile_id: &str) -> Option<Arc<Session>> {
            self.sessions.lock().unwrap().get(profile_id).cloned()
        }

        /// 对会话执行 read_mem + profile 结构解码（dispatch 入口；Busy 标记）。
        pub async fn read_struct(
            &self,
            profile_id: &str,
            addr: u64,
            ds: &DataStructure,
        ) -> Result<serde_json::Value, String> {
            let session = self
                .get(profile_id)
                .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?;
            session.set_state(SessionState::Busy);
            let result = async {
                let mut link = session.link().lock().await;
                let data = tokio::time::timeout(
                    CMD_TIMEOUT,
                    crate::pipe::read_mem(&mut link, addr, ds.size),
                )
                .await
                .map_err(|_| "read_mem 超时".to_string())?
                .map_err(|e| e.to_string())?;
                crate::dispatch::decode_struct(&data, ds).map_err(|e| e.to_string())
            }
            .await;
            session.set_state(SessionState::Ready);
            result
        }

        // -------------------------------------------------------------------
        // M4：函数地址解析 / call_fn / hook 订阅退订 / capture 拉取
        // -------------------------------------------------------------------

        /// 函数地址解析入册（M4/T4.2 支撑）：profile 全函数 locate + 链步骤
        /// → resolve 引擎（byte_pattern 活体扫描 / rva 直算）→ fn_addrs 整表替换。
        /// 返回报告 JSON（resolved 映射 + notes 诚实清单）。
        #[cfg(windows)]
        pub async fn resolve_functions(
            &self,
            profile_id: &str,
        ) -> Result<serde_json::Value, String> {
            let session = self
                .get(profile_id)
                .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?;
            let profile = crate::profile::ProfileStore::new(&self.workspace)
                .load(profile_id)
                .map_err(|e| e.to_string())?;
            let locates = crate::resolve::collect_profile_locates(&profile);
            if locates.is_empty() {
                return Err("profile 无函数 locate 可解析（functions 为空）".to_string());
            }
            session.set_state(SessionState::Busy);
            let result = async {
                let mut link = session.link().lock().await;
                let report =
                    crate::resolve::resolve_locates(&mut link, session.pid, locates).await?;
                session.set_fn_addrs(report.addrs.clone());
                session.set_fn_anchors(report.addrs.clone());
                Ok::<_, String>(report.to_json())
            }
            .await;
            session.set_state(SessionState::Ready);
            if let Ok(v) = &result {
                tracing::info!(
                    "[ziwei] 函数解析完成（profile={profile_id}）：resolved={} notes={}",
                    v["resolved"].as_object().map(|m| m.len()).unwrap_or(0),
                    v["notes"].as_array().map(|a| a.len()).unwrap_or(0)
                );
            }
            result
        }

        /// call_fn（T4.1）：目标内调函数（地址由调用方解析——语义名→addr 在
        /// 工具层裁决）。超时预算 = CMD_TIMEOUT + timeout_ms（payload 应答前
        /// 命令循环被占用，须大于看门狗窗口）。
        #[cfg(windows)]
        pub async fn call_fn_addr(
            &self,
            profile_id: &str,
            addr: u64,
            conv_code: u32,
            args: [u64; 4],
            timeout_ms: u32,
        ) -> Result<serde_json::Value, String> {
            let session = self
                .get(profile_id)
                .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?;
            session.set_state(SessionState::Busy);
            let result = async {
                let mut link = session.link().lock().await;
                let budget = CMD_TIMEOUT + std::time::Duration::from_millis(u64::from(timeout_ms));
                let outcome = tokio::time::timeout(
                    budget,
                    crate::pipe::call_fn(&mut link, addr, conv_code, args, timeout_ms),
                )
                .await
                .map_err(|_| format!("call_fn 总预算超时（{budget:?}）：payload 无应答"))?
                .map_err(|e| e.to_string())?;
                Ok::<_, String>(outcome.to_json(addr))
            }
            .await;
            session.set_state(SessionState::Ready);
            result
        }

        /// hook 订阅（T4.2/T4.6）：语义名（函数或链步骤键）→ hook_set 补丁。
        /// 幂等：同语义名已订阅 = 直接返回现状（不重复补丁）。
        #[cfg(windows)]
        pub async fn hook_start(
            &self,
            profile_id: &str,
            semantic: &str,
        ) -> Result<serde_json::Value, String> {
            let session = self
                .get(profile_id)
                .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?;
            if let Some(b) = session.hooks().iter().find(|h| h.semantic == semantic) {
                return Ok(serde_json::json!({
                    "ok": true, "id": b.id, "semantic": b.semantic,
                    "addr": format!("{:#x}", b.addr), "already": true,
                }));
            }
            // hook 补丁点 = 锚点（nop 垫所在），非函数入口——调用才需要入口
            // （执行序章），hook 需要窃取窗口（锚在编译器序章之后时两者不同）。
            let addr = session
                .fn_anchor(semantic)
                .or_else(|| session.fn_addr(semantic))
                .ok_or_else(|| {
                    format!("函数 {semantic} 地址未解析（先 ziwei.resolve 或检查锚在内存是否在场）")
                })?;
            let Some(id) = session.alloc_hook_id() else {
                return Err(format!("hook 槽已满（{HOOK_CAP}）：先退订不需要的捕获点"));
            };
            session.set_state(SessionState::Busy);
            let result = async {
                let mut link = session.link().lock().await;
                tokio::time::timeout(CMD_TIMEOUT, crate::pipe::hook_set(&mut link, id, addr))
                    .await
                    .map_err(|_| "hook_set 超时".to_string())?
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
            .await;
            session.set_state(SessionState::Ready);
            match result {
                Ok(()) => {
                    session.record_hook(HookBinding {
                        id,
                        semantic: semantic.to_string(),
                        addr,
                    });
                    tracing::info!(
                        "[ziwei] hook 订阅（profile={profile_id}）：id={id} {semantic} @ {addr:#x}"
                    );
                    Ok(serde_json::json!({
                        "ok": true, "id": id, "semantic": semantic,
                        "addr": format!("{addr:#x}"), "already": false,
                    }))
                }
                Err(e) => Err(format!("hook 订阅失败（{semantic}）：{e}")),
            }
        }

        /// hook 退订（还原入口 5×0x90；stub 常驻不回收——payload 纪律）。
        #[cfg(windows)]
        pub async fn hook_stop(
            &self,
            profile_id: &str,
            semantic: &str,
        ) -> Result<serde_json::Value, String> {
            let session = self
                .get(profile_id)
                .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?;
            let binding = session
                .remove_hook_by_semantic(semantic)
                .ok_or_else(|| format!("hook 不在场（{semantic}）"))?;
            session.set_state(SessionState::Busy);
            let result = async {
                let mut link = session.link().lock().await;
                tokio::time::timeout(
                    CMD_TIMEOUT,
                    crate::pipe::hook_unset(&mut link, binding.addr),
                )
                .await
                .map_err(|_| "hook_unset 超时".to_string())?
                .map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
            .await;
            session.set_state(SessionState::Ready);
            match result {
                Ok(()) => {
                    tracing::info!(
                        "[ziwei] hook 退订（profile={profile_id}）：id={} {semantic}",
                        binding.id
                    );
                    Ok(serde_json::json!({
                        "ok": true, "id": binding.id, "semantic": semantic,
                        "addr": format!("{:#x}", binding.addr),
                    }))
                }
                Err(e) => {
                    // 退订失败回滚账本（地址仍被补丁——账实必须一致）
                    session.record_hook(binding);
                    Err(format!("hook 退订失败（{semantic}）：{e}"))
                }
            }
        }

        /// capture 拉取（T4.3）：poll_events → 事件批按 hook 绑定归类 →
        /// capture.decode（regs[0] 为指针时读内存解码）→ CaptureStore 回流。
        /// 返回本批 summary（含 dropped 诚实计数）。
        #[cfg(windows)]
        pub async fn poll_captures(&self, profile_id: &str) -> Result<serde_json::Value, String> {
            let session = self
                .get(profile_id)
                .ok_or_else(|| format!("会话不存在（profile {profile_id}）"))?;
            let profile = crate::profile::ProfileStore::new(&self.workspace)
                .load(profile_id)
                .map_err(|e| e.to_string())?;
            // hook id → decode 结构定义（semantic 归属 → FunctionDef.capture.decode）
            let bindings = session.hooks();
            let mut decode_of: std::collections::HashMap<u32, DataStructure> =
                std::collections::HashMap::new();
            for b in &bindings {
                let Some(f) = profile.functions.iter().find(|f| f.semantic == b.semantic) else {
                    continue; // 链步骤（::chainN）无独立 decode——v1 只记 regs
                };
                if let Some(spec) = &f.capture
                    && let Some(struct_name) = spec
                        .decode
                        .as_deref()
                        .and_then(|d| d.strip_prefix("struct:"))
                    && let Some(ds) = profile
                        .data_structures
                        .iter()
                        .find(|s| s.name == struct_name)
                {
                    decode_of.insert(b.id, ds.clone());
                }
            }
            session.set_state(SessionState::Busy);
            let result = async {
                let mut link = session.link().lock().await;
                let (events, dropped) =
                    tokio::time::timeout(CMD_TIMEOUT, crate::pipe::poll_events(&mut link, 1024))
                        .await
                        .map_err(|_| "poll_events 超时".to_string())?
                        .map_err(|e| e.to_string())?;
                drop(link);
                let mut stored = 0usize;
                let mut records: Vec<serde_json::Value> = Vec::with_capacity(events.len());
                for ev in &events {
                    let mut record = serde_json::json!({
                        "source": "hook",
                        "id": ev.id,
                        "seq": ev.seq,
                        "tid": ev.tid,
                        "regs": ev.regs,
                    });
                    // decode：regs[0] 视为结构指针（v1 约定；profile ArgSpec 注记）
                    if let Some(ds) = decode_of.get(&ev.id)
                        && ev.regs[0] != 0
                    {
                        let mut link = session.link().lock().await;
                        match crate::pipe::read_mem(&mut link, ev.regs[0], ds.size).await {
                            Ok(data) => match crate::dispatch::decode_struct(&data, ds) {
                                Ok(value) => {
                                    record["decoded"] = value;
                                }
                                Err(e) => {
                                    record["decode_error"] =
                                        serde_json::json!(format!("decode: {e}"));
                                }
                            },
                            Err(e) => {
                                record["decode_error"] =
                                    serde_json::json!(format!("read_mem: {e}"));
                            }
                        }
                    }
                    if self.capture.append(profile_id, record.clone()).is_ok() {
                        stored += 1;
                    }
                    records.push(record);
                }
                Ok::<_, String>(serde_json::json!({
                    "ok": true,
                    "count": events.len(),
                    "stored": stored,
                    "dropped": dropped,
                    // 返回完整 record（含 decoded/decode_error）——只回 regs 会让
                    // 调用方拿到的 decoded 恒 null（回流面只剩 capture.read_recent）
                    "events": records,
                }))
            }
            .await;
            session.set_state(SessionState::Ready);
            result
        }

        /// estop 全量卸载（§9.4）：逐会话 detach，30s 总预算，逐条审计日志。
        pub async fn estop_unload_all(&self) -> Vec<String> {
            let ids: Vec<String> = {
                let map = self.sessions.lock().unwrap();
                map.keys().cloned().collect()
            };
            let mut out = Vec::new();
            let deadline = tokio::time::Instant::now() + ESTOP_BUDGET;
            for id in ids {
                if tokio::time::Instant::now() >= deadline {
                    out.push(format!("{id}: estop 预算耗尽，未卸载"));
                    continue;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(remaining, self.detach(&id)).await {
                    Ok(Ok(msg)) => out.push(format!("{id}: {msg}")),
                    Ok(Err(msg)) => out.push(format!("{id}: {msg}")),
                    Err(_) => out.push(format!("{id}: estop 预算内未完成")),
                }
            }
            out
        }
    }

    /// 目标进程存活（OpenProcess + GetExitCodeProcess 快照；注入前诚实预检）。
    fn process_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess};
        unsafe {
            let h = OpenProcess(0x0400, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
            if h.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(h, &mut code);
            let _ = CloseHandle(h);
            ok != 0 && code == STILL_ACTIVE as u32
        }
    }

    // -----------------------------------------------------------------------
    // 全局槽（AppState 不可扩字段——L1 COMMANDS_REGISTRY 同款 OnceLock）
    // -----------------------------------------------------------------------
    static SESSION_MANAGER: OnceLock<Arc<SessionManager>> = OnceLock::new();

    /// gateway 装配点（幂等；重复 set 忽略后到者）。
    pub fn set_global(manager: Arc<SessionManager>) -> bool {
        SESSION_MANAGER.set(manager).is_ok()
    }

    /// 全局访问（未装配 = None；WSAPI/CLI 诚实报「未装配」）。
    pub fn global() -> Option<Arc<SessionManager>> {
        SESSION_MANAGER.get().cloned()
    }

    /// estop 卸载钩子注册（gateway 装配时调；重复注册覆盖，见 estop_hook）。
    pub fn register_estop_hook() {
        crate::estop_hook::set_unload_hook(Box::new(|| {
            match global() {
                None => vec!["ziwei: 会话管理器未装配，无会话可卸".to_string()],
                Some(mgr) => {
                    // 钩子调用方上下文不可假设：gateway estop watcher 是
                    // tokio 任务——async 上下文里 Handle::block_on 必炸
                    //（「Cannot start a runtime from within a runtime」：
                    // tokio enter_runtime 对已进入 runtime 的线程 panic），
                    // block_in_place 在 current_thread runtime 上也炸。
                    // 对齐 LLM 建议钩子的桥接惯法：专用线程 + 独立
                    // current_thread runtime + mpsc 收报告，任何调用方都安全。
                    // 卸载内部自带 30s 总预算 + 单会话超时，recv 有界。
                    let (tx, rx) = std::sync::mpsc::channel();
                    match std::thread::Builder::new()
                        .name("ziwei-estop-unload".to_string())
                        .spawn(move || {
                            let report = match tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                            {
                                Ok(rt) => rt.block_on(mgr.estop_unload_all()),
                                Err(e) => vec![format!("临时 runtime 创建失败: {e}")],
                            };
                            let _ = tx.send(report);
                        }) {
                        Ok(_) => match rx.recv() {
                            Ok(report) => {
                                report.into_iter().map(|r| format!("ziwei: {r}")).collect()
                            }
                            Err(_) => vec!["ziwei: 卸载线程提前退出（无报告）".to_string()],
                        },
                        Err(e) => vec![format!("ziwei: 卸载线程创建失败: {e}")],
                    }
                }
            }
        }));
    }
}

// ---------------------------------------------------------------------------
// 非 Windows stub（编译面保持一致，运行期诚实报错）
// ---------------------------------------------------------------------------
#[cfg(not(windows))]
mod imp {
    use std::sync::Arc;

    use nemesis_ziwei_profile::{DataStructure, Profile};

    /// 会话状态（非 Windows 恒空转）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum SessionState {
        Injecting,
        Handshaking,
        Ready,
        Busy,
        Unloading,
        Failed(String),
    }

    impl SessionState {
        pub fn as_str(&self) -> &str {
            match self {
                SessionState::Injecting => "injecting",
                SessionState::Handshaking => "handshaking",
                SessionState::Ready => "ready",
                SessionState::Busy => "busy",
                SessionState::Unloading => "unloading",
                SessionState::Failed(_) => "failed",
            }
        }
    }

    /// 会话（非 Windows 永不实例化）。
    pub struct Session {
        pub profile_id: String,
        pub profile_name: String,
        pub pid: u32,
        pub arch: String,
        pub pipe_name: String,
        pub attached_at: chrono::DateTime<chrono::Utc>,
        pub target_exe: String,
    }

    impl Session {
        pub fn state(&self) -> SessionState {
            SessionState::Failed("non-windows".into())
        }
        pub fn snapshot(&self) -> serde_json::Value {
            serde_json::json!({"profile_id": self.profile_id, "state": "failed"})
        }
        pub fn struct_addr(&self, _name: &str) -> Option<u64> {
            None
        }
        pub fn set_poll_tasks(&self, _handles: Vec<tokio::task::JoinHandle<()>>) {}
        pub fn abort_poll_tasks(&self) {}
    }

    /// 会话管理器（非 Windows stub：attach 恒诚实报错）。
    pub struct SessionManager {
        _p: (),
    }

    #[derive(Debug, thiserror::Error)]
    pub enum AttachError {
        #[error("会话已存在（profile {0}），先 detach 再 attach")]
        AlreadyAttached(String),
        #[error("会话数已达上限（{max}）")]
        TooManySessions { max: u32 },
        #[error("目标进程不存在（pid={pid}）")]
        ProcessAbsent { pid: u32 },
        #[error("{0}")]
        Other(String),
    }

    impl SessionManager {
        pub fn new(_workspace: &std::path::Path, _payload_dir: Option<String>, _max: u32) -> Self {
            SessionManager { _p: () }
        }
        pub fn session_count(&self) -> usize {
            0
        }
        pub fn set_event_sink(&self, _sink: crate::scheduler::EventSink) {}
        pub fn event_sink(&self) -> Option<crate::scheduler::EventSink> {
            None
        }
        pub fn capture(&self) -> Arc<crate::capture::CaptureStore> {
            Arc::new(crate::capture::CaptureStore::new(std::path::Path::new(".")))
        }
        pub fn workspace(&self) -> &std::path::Path {
            std::path::Path::new(".")
        }
        pub fn list_sessions(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }
        pub async fn attach(
            &self,
            _profile: &Profile,
            _pid: u32,
        ) -> Result<Arc<Session>, AttachError> {
            Err(AttachError::Other(
                "紫微系统附加仅支持 Windows（注入/具名管道能力）".to_string(),
            ))
        }
        pub async fn attach_with_struct_addrs(
            &self,
            _profile: &Profile,
            _pid: u32,
            _struct_addrs: std::collections::HashMap<String, u64>,
        ) -> Result<Arc<Session>, AttachError> {
            Err(AttachError::Other(
                "紫微系统附加仅支持 Windows（注入/具名管道能力）".to_string(),
            ))
        }
        pub async fn detach(&self, profile_id: &str) -> Result<String, String> {
            Err(format!(
                "紫微系统附加仅支持 Windows（profile {profile_id}）"
            ))
        }
        pub fn get(&self, _profile_id: &str) -> Option<Arc<Session>> {
            None
        }
        pub async fn read_struct(
            &self,
            _profile_id: &str,
            _addr: u64,
            _ds: &DataStructure,
        ) -> Result<serde_json::Value, String> {
            Err("紫微系统附加仅支持 Windows".to_string())
        }
        pub async fn estop_unload_all(&self) -> Vec<String> {
            Vec::new()
        }
    }

    static SESSION_MANAGER: std::sync::OnceLock<Arc<SessionManager>> = std::sync::OnceLock::new();

    pub fn set_global(manager: Arc<SessionManager>) -> bool {
        SESSION_MANAGER.set(manager).is_ok()
    }
    pub fn global() -> Option<Arc<SessionManager>> {
        SESSION_MANAGER.get().cloned()
    }
    pub fn register_estop_hook() {}
}

pub use imp::*;

#[cfg(test)]
mod tests;
