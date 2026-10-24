//! plugin_roaming — NemesisBot QUIC 漫游插件（C-ABI 解冻第二例，新式交互样板）。
//!
//! 常驻服务形态：插件自持 tokio runtime，QUIC 中继 + rendezvous 注册表 +
//! 客户端连接面 iroh 端点全跑在插件内；同步 FFI 导出内部 `block_on` 投
//! runtime（宿主调用侧 `spawn_blocking`——拍板 10）。宿主回调经 init 传入的
//! vtable（日志转发不落盘；workspace 路径自取）。
//!
//! 面（v2 = 19 导出全实现）：生命周期 6（中继服务端）+ transport 2（dial/
//! close_conn，M3）+ identity 4（roaming.key 自产自销，M3 起 secret 不出
//! 插件）+ trust 4（trust.toml 文件操作 + accept 闸同源裁决，M3）+
//! registry 2（名册发现循环，M2）+ endpoint 2（客户端连接面端点，M3）。
//!
//! 停机语义：`roaming_stop`/`roaming_endpoint_stop` 优雅停对应面（gateway
//! 进程停机链专用）；**estop 不触达传输面**（中继/端点是传输设施不是 agent
//! 活动面）。Library 进程生命周期不卸载（插件持 runtime/线程，unload 活着的
//! 东西是 UB——vm_display 同款）。

mod endpoint;
mod identity;
mod registry_client;
mod relay_server;
mod rendezvous_server;
#[allow(dead_code)]
mod roaming; // 契约副本（与 crates/nemesis-plugin/src/roaming.rs 逐字节相等，drift 测试钉死）
mod types;

#[cfg(test)]
mod drift_tests;
#[cfg(test)]
mod identity_tests;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use identity::Identity;
use registry_client::RegistryLoopConfig;
use roaming::{
    HOST_ROAMING_ABI_VERSION, HostRoamingVTable, ROAMING_LOG_ERROR, ROAMING_LOG_INFO,
    ROAMING_LOG_WARN,
};

/// 插件状态（init 建，shutdown 收）。
struct RoamingState {
    settings: types::RelaySettings,
    workspace: PathBuf,
    runtime: Arc<tokio::runtime::Runtime>,
    relay: Mutex<Option<relay_server::RelayServerHandle>>,
    /// 名册发现循环（M2；None = 未启动）。
    registry: Mutex<Option<registry_client::RegistryLoopHandle>>,
    /// 客户端连接面端点（M3；None = 未启动）。
    endpoint: Mutex<Option<Arc<endpoint::EndpointHandle>>>,
}

static HOST: std::sync::OnceLock<HostRoamingVTable> = std::sync::OnceLock::new();
static STATE: Mutex<Option<RoamingState>> = Mutex::new(None);

// 插件返回值缓冲（thread_local——并发 FFI 调用者互不覆盖；契约：宿主收到
// 指针后立即拷贝，同线程下次调用前失效）。
thread_local! {
    static OUT: core::cell::RefCell<String> = const { core::cell::RefCell::new(String::new()) };
}

fn set_out(s: String) -> *const c_char {
    OUT.with(|c| {
        let mut buf = c.borrow_mut();
        *buf = s;
        // C 字符串 NUL 终止必须显式：String 堆缓冲不保证 NUL 结尾，宿主
        // CStr::from_ptr 会越界扫进 capacity 残余的陈旧堆字节（冒烟实录：
        // JSON 之后跟了日志格式化残骸 → 「返回非 JSON: trailing characters」）。
        buf.push('\0');
        buf.as_ptr() as *const c_char
    })
}

fn err_json(err_code: &str, err_msg: &str) -> String {
    serde_json::json!({ "ok": false, "err_code": err_code, "err_msg": err_msg }).to_string()
}

/// 插件日志出口（vtable 转发回宿主 tracing；插件不落自己的日志文件——拍板 5）。
pub fn plugin_log(level: i32, tag: &str, msg: &str) {
    if let Some(vt) = HOST.get()
        && let Some(f) = vt.log
        && let (Ok(t), Ok(m)) = (CString::new(tag), CString::new(msg))
    {
        f(level, t.as_ptr(), m.as_ptr());
    }
}

// ---------------------------------------------------------------------------
// 生命周期（6）
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn roaming_abi_version() -> u32 {
    HOST_ROAMING_ABI_VERSION
}

/// 初始化：装宿主回调表 + 解析配置（`{"relay":{...}}`）+ 建 runtime。
/// 返回 0 = 成功；-1 空指针 / -2 ABI 失配 / -3 配置解析失败 / -4 workspace
/// 路径不可得 / -5 runtime 创建失败。
///
/// # Safety
/// `vtable` 与 `config_json` 必须为合法 NUL 结尾 C 字符串；vtable 所指内存
/// 在进程生命周期内有效（宿主用 static）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_init(
    vtable: *const HostRoamingVTable,
    config_json: *const c_char,
) -> c_int {
    if vtable.is_null() || config_json.is_null() {
        return -1;
    }
    let vt = unsafe { &*vtable };
    if vt.abi_version != HOST_ROAMING_ABI_VERSION {
        return -2;
    }
    let Some(wsf) = vt.get_workspace_dir else {
        return -4;
    };
    // 返回值是**长度语义**（写入字节数 >0 = 成功；-1 = 缓冲不足/空指针——
    // vm-display 同款约定，契约注释同文）。首版误判 `!= 0` 把成功写入的
    // 路径长度当失败，init 恒 -4。
    let mut buf = [0 as c_char; 1024];
    if wsf(buf.as_mut_ptr(), buf.len()) < 0 {
        return -4;
    }
    let workspace = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .to_string();
    if workspace.trim().is_empty() {
        return -4;
    }
    let cfg_raw = unsafe { CStr::from_ptr(config_json) }.to_string_lossy();
    let cfg: serde_json::Value = match serde_json::from_str(&cfg_raw) {
        Ok(v) => v,
        Err(e) => {
            // 宿主日志通道还没装——落 stderr 兜底可观测。
            eprintln!("[plugin_roaming] init 配置解析失败: {e}");
            return -3;
        }
    };
    let Some(relay) = cfg.get("relay") else {
        return -3;
    };
    let settings = types::parse_relay_settings(relay);

    let _ = HOST.set(HostRoamingVTable {
        abi_version: vt.abi_version,
        log: vt.log,
        get_workspace_dir: vt.get_workspace_dir,
        audit_event: vt.audit_event,
        notify_incoming: vt.notify_incoming,
        notify_members: vt.notify_members,
    });

    let mut st = STATE.lock();
    match st.as_mut() {
        Some(s) => {
            // 幂等重入 = 刷新配置（运行中中继不受影响——端口/TLS 变更需
            // stop/start，宿主状态面 requires_restart 语义承接）。
            s.settings = settings;
            s.workspace = PathBuf::from(workspace);
        }
        None => {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(r) => Arc::new(r),
                Err(e) => {
                    eprintln!("[plugin_roaming] runtime 创建失败: {e}");
                    return -5;
                }
            };
            *st = Some(RoamingState {
                settings,
                workspace: PathBuf::from(workspace),
                runtime,
                relay: Mutex::new(None),
                registry: Mutex::new(None),
                endpoint: Mutex::new(None),
            });
        }
    }
    plugin_log(ROAMING_LOG_INFO, "roaming", "init ok（配置已装载）");
    0
}

/// # Safety
/// 无指针入参；须在 [`roaming_init`] 成功后调用。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_start() -> *const c_char {
    let (settings, workspace, runtime) = {
        let st = STATE.lock();
        match st.as_ref() {
            Some(s) => (s.settings.clone(), s.workspace.clone(), s.runtime.clone()),
            None => {
                return set_out(err_json(
                    "not_initialized",
                    "roaming_init 未调用（宿主装配顺序错误）",
                ));
            }
        }
    };
    // fail-closed（与宿主三路收口纪律同源）：跳板无鉴权裸奔 = 公网白蹭 +
    // 存在性探测面。宿主（gateway 闸 / CLI 显式意志）先查 token 再调 start，
    // 此处兜底同值拒绝。
    if settings.token.trim().is_empty() {
        return set_out(err_json(
            "token_missing",
            "relay.token 为空：无鉴权中继拒绝启动（fail-closed）",
        ));
    }
    // 幂等：已运行直接回当前形态。
    if let Some(payload) = snapshot_if_running() {
        return set_out(payload);
    }

    let result = runtime.block_on(relay_server::spawn_relay_server(&settings, &workspace));
    let handle = match result {
        Ok(h) => h,
        Err(e) => {
            plugin_log(ROAMING_LOG_ERROR, "relay", &format!("启动失败: {e}"));
            return set_out(err_json("spawn_failed", &e));
        }
    };
    plugin_log(
        ROAMING_LOG_INFO,
        "relay",
        &format!(
            "自建中继已启动 http={:?} quic={:?}",
            handle.http_addr, handle.quic_addr
        ),
    );
    // 落槽；并发双跑（理论态：宿主纪律串行调用，此处防御兜底）后到者优雅
    // 关停保先到实例——原 QuicRelayManager 同款语义。
    enum Outcome {
        Inserted,
        Duplicate(relay_server::RelayServerHandle),
        NoState(relay_server::RelayServerHandle),
    }
    let outcome = {
        let st = STATE.lock();
        match st.as_ref() {
            Some(s) => {
                let mut slot = s.relay.lock();
                if slot.is_none() {
                    *slot = Some(handle);
                    Outcome::Inserted
                } else {
                    Outcome::Duplicate(handle)
                }
            }
            None => Outcome::NoState(handle),
        }
    };
    if let Outcome::Duplicate(mut dup) | Outcome::NoState(mut dup) = outcome {
        plugin_log(
            ROAMING_LOG_WARN,
            "relay",
            "并发双跑命中：关停后到句柄，保先到实例",
        );
        runtime.block_on(dup.shutdown());
    }
    set_out(
        snapshot_if_running()
            .unwrap_or_else(|| serde_json::json!({ "ok": true, "running": false }).to_string()),
    )
}

/// # Safety
/// 无指针入参；init 前调用返回 not_initialized JSON（不炸）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_stop() -> *const c_char {
    let (runtime, taken) = {
        let st = STATE.lock();
        match st.as_ref() {
            Some(s) => {
                let h = s.relay.lock().take();
                (s.runtime.clone(), h)
            }
            None => {
                return set_out(serde_json::json!({ "ok": true, "running": false }).to_string());
            }
        }
    };
    match taken {
        None => set_out(serde_json::json!({ "ok": true, "running": false }).to_string()),
        Some(mut h) => {
            plugin_log(
                ROAMING_LOG_INFO,
                "relay",
                "自建中继停机（运行时开关/停机链）",
            );
            runtime.block_on(h.shutdown());
            set_out(serde_json::json!({ "ok": true, "running": false }).to_string())
        }
    }
}

/// # Safety
/// 无指针入参；init 前调用返回 not_initialized JSON（不炸）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_status() -> *const c_char {
    let st = STATE.lock();
    let Some(s) = st.as_ref() else {
        return set_out(err_json(
            "not_initialized",
            "roaming_init 未调用（宿主装配顺序错误）",
        ));
    };
    let slot = s.relay.lock();
    // 名册循环运行状态（Phase 2 观测性：失败态出插件——fail_streak/last_error/
    // last_ok/last_fail 毫秒；宿主 merge 出 rendezvous 段最终形态）。循环未挂
    // = Null（宿主回落宿主侧快照）。
    let (registry_running, registry_sync) = {
        let reg = s.registry.lock();
        let running = reg.as_ref().map(|h| h.running()).unwrap_or(false);
        let sync = reg
            .as_ref()
            .map(|h| {
                let st = h.status_snapshot();
                serde_json::json!({
                    "registry_url": st.registry_url,
                    "fail_streak": st.fail_streak,
                    "last_error": st.last_error,
                    "last_ok_ms": st.last_ok_ms,
                    "last_fail_ms": st.last_fail_ms,
                    "last_member_count": st.last_member_count,
                })
            })
            .unwrap_or(serde_json::Value::Null);
        (running, sync)
    };
    // 客户端连接面快照（M3）：running/node_id/拨号路径观测（宿主桥
    // roaming_status_snapshot 提取 endpoint 段）。
    let endpoint_slot = s.endpoint.lock();
    let mut v = serde_json::json!({
        "ok": true,
        "initialized": true,
        "config_enabled": s.settings.enabled,
        "running": slot.as_ref().map(|h| h.running()).unwrap_or(false),
        "registry_running": registry_running,
        "registry_sync": registry_sync,
        "endpoint": endpoint_slot.as_ref().map(|h| {
            serde_json::json!({
                "running": true,
                "node_id": h.node_id_hex(),
                "direct_addrs": h.direct_addrs().iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "dial_paths": h.dial_paths_snapshot(),
            })
        }),
    });
    if let Some(h) = slot.as_ref() {
        v["http_addr"] = opt_addr(h.http_addr);
        v["quic_addr"] = opt_addr(h.quic_addr);
        v["registry_addr"] = match h.registry.as_ref() {
            Some(r) => serde_json::json!(r.addr.to_string()),
            None => serde_json::Value::Null,
        };
        v["registry_members"] = serde_json::json!(
            h.registry
                .as_ref()
                .map(|r| r.live_members())
                .unwrap_or_default()
        );
    }
    set_out(v.to_string())
}

/// 幂等命中快照（已运行时 start 的回执形态）。
fn snapshot_if_running() -> Option<String> {
    let st = STATE.lock();
    let s = st.as_ref()?;
    let slot = s.relay.lock();
    slot.as_ref().map(relay_payload)
}

fn relay_payload(h: &relay_server::RelayServerHandle) -> String {
    let v = serde_json::json!({
        "ok": true,
        "running": h.running(),
        "http_addr": opt_addr(h.http_addr),
        "quic_addr": opt_addr(h.quic_addr),
        "registry_addr": match h.registry.as_ref() {
            Some(r) => serde_json::json!(r.addr.to_string()),
            None => serde_json::Value::Null,
        },
        "registry_members": serde_json::json!(h
            .registry
            .as_ref()
            .map(|r| r.live_members())
            .unwrap_or_default()),
    });
    v.to_string()
}

fn opt_addr(a: Option<std::net::SocketAddr>) -> serde_json::Value {
    match a {
        Some(a) => serde_json::json!(a.to_string()),
        None => serde_json::Value::Null,
    }
}

/// 停机收尾（gateway 进程终态调用；断通道不卸库——Library 进程生命周期）。
#[unsafe(no_mangle)]
pub extern "C" fn roaming_shutdown() {
    let taken = { STATE.lock().take() };
    if let Some(s) = taken {
        let mut relay = s.relay.lock().take();
        if let Some(h) = relay.as_mut() {
            plugin_log(ROAMING_LOG_INFO, "roaming", "shutdown（优雅停中继）");
            s.runtime.block_on(h.shutdown());
        }
        drop(relay);
        let mut registry = s.registry.lock().take();
        if let Some(h) = registry.as_mut() {
            s.runtime.block_on(h.shutdown());
        }
        drop(registry);
        let endpoint = s.endpoint.lock().take();
        if let Some(h) = endpoint.as_ref() {
            plugin_log(
                ROAMING_LOG_INFO,
                "roaming",
                "shutdown（停客户端连接面端点）",
            );
            s.runtime.block_on(h.shutdown());
        }
        drop(endpoint);
        // runtime 引用计数归零即停；后台残留任务由 shutdown_background 收。
        if let Ok(rt) = Arc::try_unwrap(s.runtime) {
            rt.shutdown_background();
        }
    }
}

// ---------------------------------------------------------------------------
// transport（2）—— M3 接线：宿主帧层经 loopback TCP 端口消费，帧协议照跑
// ---------------------------------------------------------------------------

fn state_endpoint() -> Result<Arc<endpoint::EndpointHandle>, *const c_char> {
    let st = STATE.lock();
    match st.as_ref().and_then(|s| s.endpoint.lock().clone()) {
        Some(h) => Ok(h),
        None => Err(set_out(err_json(
            "endpoint_not_running",
            "客户端连接面端点未启动（endpoint_start 先行）",
        ))),
    }
}

/// dial：`iroh:<hex>[@<ip:port>|@<relay_url>]` → 插件拨号 + 开一次性 loopback
/// TCP 监听，返回 `{"ok":true,"port":<u16>,"conn_id":<u64>}`——宿主
/// `TcpStream::connect(127.0.0.1:port)` 后帧协议照跑。宿主超时未拨入 = 插件
/// 侧连接强关 + 摘注册（半开兜底）。
///
/// # Safety
/// `addr` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_dial(addr: *const c_char) -> *const c_char {
    let addr = arg_str(addr);
    if addr.trim().is_empty() {
        return set_out(err_json("bad_args", "addr 为空"));
    }
    let runtime = {
        let st = STATE.lock();
        match st.as_ref() {
            Some(s) => s.runtime.clone(),
            None => {
                return set_out(err_json(
                    "not_initialized",
                    "roaming_init 未调用（宿主装配顺序错误）",
                ));
            }
        }
    };
    let handle = match state_endpoint() {
        Ok(h) => h,
        Err(out) => return out,
    };
    match runtime.block_on(handle.dial(&addr)) {
        Ok((port, conn_id)) => {
            set_out(serde_json::json!({ "ok": true, "port": port, "conn_id": conn_id }).to_string())
        }
        Err(e) => set_out(err_json("dial_failed", &e)),
    }
}

/// 按连接 id 显式关闭（半开兜底；关整条 QUIC 连接——流共享连接，插件侧
/// endpoint.rs 粒度注释同文）。不存在的 id = 幂等 ok（`closed:false`）。
///
/// # Safety
/// 无。
#[unsafe(no_mangle)]
pub extern "C" fn roaming_close_conn(conn_id: u64) -> *const c_char {
    let handle = match state_endpoint() {
        Ok(h) => h,
        Err(out) => return out,
    };
    let closed = handle.close_conn(conn_id);
    set_out(serde_json::json!({ "ok": true, "closed": closed }).to_string())
}

// ---------------------------------------------------------------------------
// identity（4）—— M2 接线：身份自产自销（插件是 roaming.key 唯一读写者；
// M3 起 endpoint 也在插件内，seed 不再出插件——identity_payload 一律
// with_secret=false，宿主只见 node_id）。
// ---------------------------------------------------------------------------

fn arg_str(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn state_ws_runtime() -> Result<(PathBuf, Arc<tokio::runtime::Runtime>), *const c_char> {
    let st = STATE.lock();
    match st.as_ref() {
        Some(s) => Ok((s.workspace.clone(), s.runtime.clone())),
        None => Err(set_out(err_json(
            "not_initialized",
            "roaming_init 未调用（宿主装配顺序错误）",
        ))),
    }
}

fn identity_payload(id: &Identity, created: bool) -> serde_json::Value {
    // M3：secret 不出插件（seed 只在插件内落盘/消费）；`created` 是契约
    // 承诺字段（宿主桥 IdentityInfo 反序列化依赖）。
    serde_json::json!({ "ok": true, "created": created, "iroh_id": id.node_id_hex() })
}

/// keygen：`{"force":bool}`（缺省 false）。force=false = load_or_create
/// （镜像旧宿主 CLI 语义：无文件生成 + 落盘，有文件原样加载）；force=true =
/// 无条件重生成覆盖。响应带 `created` / `iroh_id`（M3 起 secret 不出插件）。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_identity_keygen(arg: *const c_char) -> *const c_char {
    let force = serde_json::from_str::<serde_json::Value>(&arg_str(arg))
        .ok()
        .and_then(|v| v.get("force").and_then(|b| b.as_bool()))
        .unwrap_or(false);
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    let path = identity::roaming_key_path(&workspace);
    let id_created = if force {
        let id = Identity::generate();
        match identity::persist(&id, &path) {
            Ok(()) => (id, true),
            Err(e) => return set_out(err_json("persist_failed", &e)),
        }
    } else {
        match Identity::load_or_create(&workspace) {
            Ok(v) => v,
            Err(e) => return set_out(err_json("identity_load_failed", &e)),
        }
    };
    if id_created.1 {
        plugin_log(
            ROAMING_LOG_INFO,
            "identity",
            &format!("新身份已生成 {}", id_created.0.node_id_hex()),
        );
    }
    set_out(identity_payload(&id_created.0, id_created.1).to_string())
}

/// load：无入参。文件缺失/损坏 = `identity_load_failed`。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_identity_load(arg: *const c_char) -> *const c_char {
    let _ = arg_str(arg);
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    match Identity::load(&identity::roaming_key_path(&workspace)) {
        Ok(id) => set_out(identity_payload(&id, false).to_string()),
        Err(e) => set_out(err_json("identity_load_failed", &e)),
    }
}

/// get_iroh_id：无入参，只回身份不回 secret。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_get_iroh_id(arg: *const c_char) -> *const c_char {
    let _ = arg_str(arg);
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    match Identity::load(&identity::roaming_key_path(&workspace)) {
        Ok(id) => set_out(identity_payload(&id, false).to_string()),
        Err(e) => set_out(err_json("identity_load_failed", &e)),
    }
}

/// set：`{"secret_key_hex":"..."}`（hex 32B seed）。写盘后若名册循环在跑
/// 则停掉（身份已轮换——载荷还带旧 iroh_id，宿主需重新备料 registry_start）。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_identity_set(arg: *const c_char) -> *const c_char {
    let hex_str = serde_json::from_str::<serde_json::Value>(&arg_str(arg))
        .ok()
        .and_then(|v| {
            v.get("secret_key_hex")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let Some(hex_str) = hex_str else {
        return set_out(err_json("bad_args", "缺 secret_key_hex"));
    };
    let bytes = match hex::decode(hex_str.trim()) {
        Ok(b) => b,
        Err(e) => return set_out(err_json("bad_secret_hex", &format!("hex 非法: {e}"))),
    };
    let seed: [u8; 32] = match bytes.try_into() {
        Ok(s) => s,
        Err(v) => {
            return set_out(err_json(
                "bad_secret_hex",
                &format!("长度 {} 字节 ≠ 32（seed）", v.len()),
            ));
        }
    };
    let id = Identity::from_seed(&seed);
    let (workspace, runtime) = match state_ws_runtime() {
        Ok(v) => v,
        Err(out) => return out,
    };
    let path = identity::roaming_key_path(&workspace);
    if let Err(e) = identity::persist(&id, &path) {
        return set_out(err_json("persist_failed", &e));
    }
    // 身份轮换 → 名册循环自停（注册载荷还是旧 iroh_id；宿主重发
    // registry_start 携新载荷重启——热更语义）。
    let old = {
        let st = STATE.lock();
        st.as_ref().and_then(|s| s.registry.lock().take())
    };
    if let Some(mut h) = old {
        runtime.block_on(h.shutdown());
        plugin_log(
            ROAMING_LOG_INFO,
            "identity",
            "身份已变更，名册循环已停（宿主需重新 registry_start）",
        );
    }
    set_out(identity_payload(&id, false).to_string())
}

// ---------------------------------------------------------------------------
// trust（4）—— M3 接线：trust.toml 文件操作搬插件（accept 闸同文件同格式
// ——插件是裁决者，文件操作面自然同宿主；node_id 经 normalize 归一入簿）。
// ---------------------------------------------------------------------------

fn trust_book_payload(
    data: &endpoint::TrustBookData,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut v = serde_json::json!({
        "ok": true,
        "trusted": data.trusted,
        "revoked": data.revoked,
    });
    if let Some(obj) = v.as_object_mut()
        && let Some(extra_obj) = extra.as_object()
    {
        for (k, val) in extra_obj {
            obj.insert(k.clone(), val.clone());
        }
    }
    v
}

fn trust_node_id(arg: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(arg)
        .ok()
        .and_then(|v| {
            v.get("node_id")
                .or_else(|| v.get("iroh_id"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
        .map(|s| endpoint::normalize_id(&s))
}

/// trust：`{"node_id":"..."}` 入簿（幂等——已在 trusted 不重复）。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_trust_trust(arg: *const c_char) -> *const c_char {
    let Some(node_id) = trust_node_id(&arg_str(arg)) else {
        return set_out(err_json("bad_args", "缺 node_id"));
    };
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    let mut book = match endpoint::load_trust_book(&workspace) {
        Ok(b) => b,
        Err(e) => return set_out(err_json("trust_book_read_failed", &e)),
    };
    if !book
        .trusted
        .iter()
        .any(|s| endpoint::normalize_id(s) == node_id)
    {
        book.trusted.push(node_id.clone());
    }
    if let Err(e) = endpoint::save_trust_book(&workspace, &book) {
        return set_out(err_json("trust_book_write_failed", &e));
    }
    plugin_log(ROAMING_LOG_INFO, "trust", &format!("已信任 {node_id}"));
    set_out(trust_book_payload(&book, serde_json::json!({ "node_id": node_id })).to_string())
}

/// revoke：`{"node_id":"..."}` 入吊销段（一票否决优先于 trusted——accept 闸
/// 裁决次序）。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_trust_revoke(arg: *const c_char) -> *const c_char {
    let Some(node_id) = trust_node_id(&arg_str(arg)) else {
        return set_out(err_json("bad_args", "缺 node_id"));
    };
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    let mut book = match endpoint::load_trust_book(&workspace) {
        Ok(b) => b,
        Err(e) => return set_out(err_json("trust_book_read_failed", &e)),
    };
    if !book
        .revoked
        .iter()
        .any(|s| endpoint::normalize_id(s) == node_id)
    {
        book.revoked.push(node_id.clone());
    }
    if let Err(e) = endpoint::save_trust_book(&workspace, &book) {
        return set_out(err_json("trust_book_write_failed", &e));
    }
    plugin_log(ROAMING_LOG_INFO, "trust", &format!("已吊销 {node_id}"));
    set_out(trust_book_payload(&book, serde_json::json!({ "node_id": node_id })).to_string())
}

/// unrevoke：`{"node_id":"..."}` 出吊销段（幂等——不在段中即 no-op）。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_trust_unrevoke(arg: *const c_char) -> *const c_char {
    let Some(node_id) = trust_node_id(&arg_str(arg)) else {
        return set_out(err_json("bad_args", "缺 node_id"));
    };
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    let mut book = match endpoint::load_trust_book(&workspace) {
        Ok(b) => b,
        Err(e) => return set_out(err_json("trust_book_read_failed", &e)),
    };
    book.revoked
        .retain(|s| endpoint::normalize_id(s) != node_id);
    if let Err(e) = endpoint::save_trust_book(&workspace, &book) {
        return set_out(err_json("trust_book_write_failed", &e));
    }
    set_out(trust_book_payload(&book, serde_json::json!({ "node_id": node_id })).to_string())
}

/// list：读信任簿全量（文件缺失 = 空簿，不创建文件——读操作零副作用）。
///
/// # Safety
/// `arg` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_trust_list(arg: *const c_char) -> *const c_char {
    let _ = arg_str(arg);
    let workspace = match state_ws_runtime() {
        Ok((ws, _)) => ws,
        Err(out) => return out,
    };
    match endpoint::load_trust_book(&workspace) {
        Ok(book) => set_out(trust_book_payload(&book, serde_json::json!({})).to_string()),
        Err(e) => set_out(err_json("trust_book_read_failed", &e)),
    }
}

// ---------------------------------------------------------------------------
// registry（2）—— M2 接线：名册发现循环在插件内跑（拍板 7），成员经
// notify_members 推宿主漏斗。
// ---------------------------------------------------------------------------

/// start：入参 `{"registry_url":"...","token":"...","member":{...},
/// "interval_secs":30}`。身份一致性闸同步裁决（载荷 iroh_id 必须 == 插件
/// 盘上身份，err_code `identity_mismatch`）；幂等重入 = 停旧起新。
///
/// # Safety
/// `config` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_registry_start(config: *const c_char) -> *const c_char {
    let cfg_raw = arg_str(config);
    let cfg: RegistryLoopConfig = match serde_json::from_str(&cfg_raw) {
        Ok(c) => c,
        Err(e) => {
            return set_out(err_json(
                "bad_config",
                &format!("registry 配置解析失败: {e}"),
            ));
        }
    };
    if cfg.registry_url.trim().is_empty() {
        return set_out(err_json("bad_config", "registry_url 为空"));
    }
    if cfg.token.trim().is_empty() {
        return set_out(err_json("bad_config", "registry token 为空（fail-closed）"));
    }
    let (workspace, runtime) = match state_ws_runtime() {
        Ok(v) => v,
        Err(out) => return out,
    };
    // 身份一致性闸：载荷身份必须等于插件自己盘上身份——宿主组装载荷用了
    // 陈旧身份时 loud 拒（防「注册身份 ≠ 连接身份」静默分叉）。
    let own = match Identity::load(&identity::roaming_key_path(&workspace)) {
        Ok(i) => i,
        Err(e) => return set_out(err_json("identity_missing", &e)),
    };
    if cfg.member.iroh_id.trim().to_ascii_lowercase() != own.node_id_hex() {
        return set_out(err_json(
            "identity_mismatch",
            "注册载荷 iroh_id 与插件盘上身份不一致（重新备料后重发 registry_start）",
        ));
    }
    // 幂等重入 = 停旧起新（热更路径：身份/宣告字段变化 → 宿主重发 start）。
    let old = {
        let st = STATE.lock();
        st.as_ref().and_then(|s| s.registry.lock().take())
    };
    if let Some(mut h) = old {
        runtime.block_on(h.shutdown());
    }
    let handle = runtime.block_on(registry_client::spawn_registry_loop(cfg, |members| {
        if let Some(vt) = HOST.get()
            && let Some(f) = vt.notify_members
        {
            let payload = serde_json::json!({ "members": members }).to_string();
            if let Ok(c) = CString::new(payload) {
                f(c.as_ptr());
            }
        }
    }));
    let running = handle.running();
    {
        let st = STATE.lock();
        if let Some(s) = st.as_ref() {
            *s.registry.lock() = Some(handle);
        }
    }
    set_out(serde_json::json!({ "ok": true, "running": running }).to_string())
}

/// stop：停名册循环（幂等）。
#[unsafe(no_mangle)]
pub extern "C" fn roaming_registry_stop() -> *const c_char {
    let (runtime, old) = {
        let st = STATE.lock();
        match st.as_ref() {
            Some(s) => (s.runtime.clone(), s.registry.lock().take()),
            None => {
                return set_out(serde_json::json!({ "ok": true, "running": false }).to_string());
            }
        }
    };
    if let Some(mut h) = old {
        runtime.block_on(h.shutdown());
    }
    set_out(serde_json::json!({ "ok": true, "running": false }).to_string())
}

// ---------------------------------------------------------------------------
// endpoint（2）—— M3 接线：客户端连接面端点（dial/accept 共用承载）。
// ---------------------------------------------------------------------------

/// start：入参 `{"relay_url":"","relay_token":"","idle_timeout_secs":0}`
/// （relay_url 空 = n0 公共中继；非空 = Minimal 预设 + 自建中继 RelayMap +
/// 可选 auth token）。身份从插件盘上 roaming.key 取（缺 = `identity_missing`
/// ——身份面 keygen/load 先行）；幂等热更 = 停旧端点起新。返回
/// `{"ok":true,"node_id":hex}`。
///
/// # Safety
/// `config` 须为合法 NUL 结尾 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn roaming_endpoint_start(config: *const c_char) -> *const c_char {
    let cfg_raw = arg_str(config);
    let cfg: endpoint::EndpointConfig = match serde_json::from_str(&cfg_raw) {
        Ok(c) => c,
        Err(e) => {
            return set_out(err_json(
                "bad_config",
                &format!("endpoint 配置解析失败: {e}"),
            ));
        }
    };
    let (workspace, runtime) = match state_ws_runtime() {
        Ok(v) => v,
        Err(out) => return out,
    };
    let id = match Identity::load(&identity::roaming_key_path(&workspace)) {
        Ok(i) => i,
        Err(e) => return set_out(err_json("identity_missing", &e)),
    };
    // 幂等热更：停旧端点起新（relay 变更/身份轮换后宿主重发 start）。
    let old = {
        let st = STATE.lock();
        st.as_ref().and_then(|s| s.endpoint.lock().take())
    };
    if let Some(h) = old {
        plugin_log(ROAMING_LOG_INFO, "endpoint", "热更：停旧端点");
        runtime.block_on(h.shutdown());
    }
    let handle = match runtime.block_on(endpoint::spawn_endpoint(&cfg, &id.seed(), &workspace)) {
        Ok(h) => h,
        Err(e) => {
            plugin_log(ROAMING_LOG_ERROR, "endpoint", &format!("启动失败: {e}"));
            return set_out(err_json("spawn_failed", &e));
        }
    };
    let node_id = handle.node_id_hex();
    {
        let st = STATE.lock();
        if let Some(s) = st.as_ref() {
            *s.endpoint.lock() = Some(handle);
        }
    }
    plugin_log(
        ROAMING_LOG_INFO,
        "endpoint",
        &format!("端点已启动 {node_id}"),
    );
    set_out(serde_json::json!({ "ok": true, "node_id": node_id }).to_string())
}

/// stop：停端点（router 收流 + 注册连接强关 + endpoint.close；幂等）。
#[unsafe(no_mangle)]
pub extern "C" fn roaming_endpoint_stop() -> *const c_char {
    let (runtime, old) = {
        let st = STATE.lock();
        match st.as_ref() {
            Some(s) => (s.runtime.clone(), s.endpoint.lock().take()),
            None => {
                return set_out(serde_json::json!({ "ok": true, "running": false }).to_string());
            }
        }
    };
    if let Some(h) = old {
        plugin_log(
            ROAMING_LOG_INFO,
            "endpoint",
            "端点停机（运行时开关/停机链）",
        );
        runtime.block_on(h.shutdown());
    }
    set_out(serde_json::json!({ "ok": true, "running": false }).to_string())
}
