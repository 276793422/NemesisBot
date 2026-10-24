//! Roaming 插件 C-ABI 契约（v2）。
//!
//! 双向复制纪律（vm_display.rs 同款）：本文件是宿主侧真相源，`plugins/plugin-roaming/`
//! 持有逐字节副本（src/roaming.rs），drift 测试钉死相等。修改本文件 = 同时修改两处
//! + bump `HOST_ROAMING_ABI_VERSION`。
//!
//! # 版本史
//!
//! - v1（QUIC 插件化 M1/M2）：生命周期/identity/trust/registry 17 导出 +
//!   `notify_incoming(conn_id, port)` / `notify_members`；
//! - v2（M3）：`notify_incoming` 增 `remote_hex`（宿主受理器日志/审计带对端身份，
//!   插件 accept 时已握有远端 EndpointId，顺手携带零成本）；追加
//!   `roaming_endpoint_start`/`roaming_endpoint_stop`（客户端连接面端点生命周期——
//!   dial/accept 两臂的 iroh 承载搬插件，宿主只控启停与拨号）。
//!
//! 新式交互插件样板（plugin-vm 是老式功能插件样板）：
//! - **双向回调**：vtable 除查询面外含 plugin → host 反向通知（notify_incoming/notify_members）；
//! - **常驻服务**：插件自持 tokio runtime + iroh endpoint/中继常驻监听；
//! - **分组函数表**：plugin → host 导出按子对象分组（生命周期/transport/identity/trust/registry），
//!   「未来可能会增加更多子对象。未来需要客户端先加功能，后续才能提供给外部使用」——
//!   新功能先在插件内部长成内部模块（不占 ABI），稳定后开成 `roaming_<子对象>_<操作>`
//!   导出并演进版本闸。开出去的函数即冻结承诺。
//!
//! # 线程纪律
//!
//! 所有导出函数（除 `roaming_abi_version`/`roaming_shutdown`）内部投插件自持 runtime
//! 阻塞等结果——宿主调用侧必须 `spawn_blocking`，不卡 async executor 线程（拍板 10）。
//!
//! # 返回 JSON 约定
//!
//! 除 `roaming_abi_version`（→ u32）与 `roaming_shutdown`（→ void）外，全部导出返回
//! `*const c_char`，指向插件内静态 JSON 缓冲（per-call 覆盖，单线程串行纪律）：
//! - 成功：`{"ok":true, ...载荷}`（start 携带 http_addr/quic_addr/registry_addr；
//!   dial 携带 port/conn_id；status 携带运行态 + registry_members 数组）；
//! - 失败：`{"ok":false,"err_code":"...","err_msg":"..."}`。
//!
//! # 版本闸
//!
//! vtable 首字段 `abi_version: u32` 双向闸（vm-display 同款：宿主读首字段不认识即拒载，
//! 插件同样校验宿主版本）。函数指针全部 `Option` 包裹（Some = 宿主实现 / None = 不支持），
//! 向前兼容 = 末尾追加 + 版本 +1。

use std::os::raw::{c_char, c_int};

/// 宿主侧 ABI 版本（插件侧同款常量双向闸）。
pub const HOST_ROAMING_ABI_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// host → plugin vtable（宿主实现，init 时传入插件）
// ---------------------------------------------------------------------------

/// 日志级别（vtable `log` 第一参数；与 tracing 级别对齐）。
pub const ROAMING_LOG_TRACE: i32 = 0;
pub const ROAMING_LOG_DEBUG: i32 = 1;
pub const ROAMING_LOG_INFO: i32 = 2;
pub const ROAMING_LOG_WARN: i32 = 3;
pub const ROAMING_LOG_ERROR: i32 = 4;

/// 日志回调：插件日志转发回宿主 tracing（拍板 5：转发不落盘，插件不落自己的日志文件）。
/// tag 为短标签（如 "relay" / "rendezvous"），宿主负责并入消息体。
pub type RoamingLogFn = extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char);

/// 工作目录回调：把 workspace 根绝对路径写入 buf（UTF-8 + NUL）。
/// 返回值 = **写入字节数（>0 成功）**；-1 = 缓冲不足（截断）或宿主不支持
/// （vm-display `write_cstr` 同款长度语义——宿主单测按此钉死，勿改回
/// 「0=成功」布尔语义）。插件自管文件（roaming.key / trust.toml / relay
/// 证书）路径与格式不变——迁移零成本。
pub type RoamingWorkspaceDirFn = extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32;

/// 安全审计回调：拨号/入站/信任簿变更等事件进宿主审计链。
/// event = 事件类别短串，detail = JSON 明细。
pub type RoamingAuditFn = extern "C" fn(event: *const c_char, detail: *const c_char);

/// 入站连接交接回调：插件 accept iroh 连接后开 loopback 端口，通知宿主受理
/// （宿主 `TcpStream::connect(port)` 后帧协议照跑；流粒度 = 一条 bi-stream 一条 TCP）。
/// v2：追加 `remote_hex`（对端 EndpointId 64 位小写 hex；宿主受理器日志/审计
/// 携带对端身份——插件 accept 时已握有远端 id，顺手携带零成本）。
pub type RoamingNotifyIncomingFn =
    extern "C" fn(conn_id: u64, port: u16, remote_hex: *const c_char);

/// 名册成员回调：插件内 register/members 循环拉到成员 → 推宿主漏斗入册
/// （集群状态是宿主的）。json = 成员数组载荷。
pub type RoamingNotifyMembersFn = extern "C" fn(json: *const c_char);

/// host → plugin 回调表。init 时由宿主构造传入；首字段为版本闸。
#[repr(C)]
pub struct HostRoamingVTable {
    /// vtable 版本（== `HOST_ROAMING_ABI_VERSION`），双向闸锚点。
    pub abi_version: u32,
    pub log: Option<RoamingLogFn>,
    pub get_workspace_dir: Option<RoamingWorkspaceDirFn>,
    pub audit_event: Option<RoamingAuditFn>,
    pub notify_incoming: Option<RoamingNotifyIncomingFn>,
    pub notify_members: Option<RoamingNotifyMembersFn>,
}

// ---------------------------------------------------------------------------
// plugin → host 导出（分组函数表，符号统一 roaming_ 前缀）
// ---------------------------------------------------------------------------

// --- 生命周期（6） ---

pub const ROAMING_ABI_VERSION_SYMBOL: &str = "roaming_abi_version";
pub const ROAMING_INIT_SYMBOL: &str = "roaming_init";
pub const ROAMING_START_SYMBOL: &str = "roaming_start";
pub const ROAMING_STOP_SYMBOL: &str = "roaming_stop";
pub const ROAMING_STATUS_SYMBOL: &str = "roaming_status";
pub const ROAMING_SHUTDOWN_SYMBOL: &str = "roaming_shutdown";

/// 读 ABI 版本（宿主装载闸第一道）。
pub type RoamingAbiVersionFn = extern "C" fn() -> u32;

/// 初始化：传宿主回调表 + 配置 JSON（`{"relay": {...RelaySettings 规范化形态...}}`，
/// relay 段 = 宿主 config.quic.json relay 节规范化后的 serde 形态；tls 字段为
/// "self-signed"|"manual"|"none" 字符串）。返回 0 = 成功；非 0 = 拒载/配置不可用。
/// 重复调用 = 刷新配置（幂等放行）。
pub type RoamingInitFn =
    unsafe extern "C" fn(vtable: *const HostRoamingVTable, config_json: *const c_char) -> c_int;

/// 启动常驻服务（中继/endpoint/名册循环，按配置与后续 M2/M3 接线逐面生效）。
/// 返回静态 JSON：`{"ok":true,...}` 或 `{"ok":false,"err_code","err_msg"}`。
pub type RoamingStrFn = unsafe extern "C" fn() -> *const c_char;

/// 优雅停机（gateway 进程停机链专用；**estop 不触达传输面**——中继/endpoint 是
/// 传输设施不是 agent 活动面）。断通道不卸库（Library 进程生命周期，防 UB）。
pub type RoamingShutdownFn = extern "C" fn();

// --- transport（集群数据路径用，2；M3 接线生效） ---

pub const ROAMING_DIAL_SYMBOL: &str = "roaming_dial";
pub const ROAMING_CLOSE_CONN_SYMBOL: &str = "roaming_close_conn";

/// 拨号：addr 形如 `iroh:<node_id>`。成功后插件在本机开一次性 loopback TCP
/// 监听，返回 `{"ok":true,"port":<u16>,"conn_id":<u64>}`——宿主
/// `TcpStream::connect(127.0.0.1:port)` 后帧协议照跑。
pub type RoamingDialFn = unsafe extern "C" fn(addr: *const c_char) -> *const c_char;

/// 按连接 id 显式关闭（半开兜底）。
pub type RoamingCloseConnFn = unsafe extern "C" fn(conn_id: u64) -> *const c_char;

// --- identity（4；M2 接线） ---

pub const ROAMING_IDENTITY_KEYGEN_SYMBOL: &str = "roaming_identity_keygen";
pub const ROAMING_IDENTITY_LOAD_SYMBOL: &str = "roaming_identity_load";
pub const ROAMING_GET_IROH_ID_SYMBOL: &str = "roaming_get_iroh_id";
pub const ROAMING_SET_IDENTITY_SYMBOL: &str = "roaming_identity_set";

/// 身份自产自销（roaming.key 落 workspace/cluster/，经 get_workspace_dir 自取路径）。
/// keygen 入参：`{"force":bool}`；identity_load / get_iroh_id 无入参；
/// set 入参：`{"secret_key_hex":"..."}`（node.update_identity 热更新同步）。
pub type RoamingJsonFn = unsafe extern "C" fn(arg: *const c_char) -> *const c_char;

// --- trust（4；M3 起插件内实作裁决与文件操作） ---

pub const ROAMING_TRUST_TRUST_SYMBOL: &str = "roaming_trust_trust";
pub const ROAMING_TRUST_REVOKE_SYMBOL: &str = "roaming_trust_revoke";
pub const ROAMING_TRUST_UNREVOKE_SYMBOL: &str = "roaming_trust_unrevoke";
pub const ROAMING_TRUST_LIST_SYMBOL: &str = "roaming_trust_list";

// --- registry（2；M2 接线） ---

pub const ROAMING_REGISTRY_START_SYMBOL: &str = "roaming_registry_start";
pub const ROAMING_REGISTRY_STOP_SYMBOL: &str = "roaming_registry_stop";

/// 名册循环：register/members HTTP 搬插件内跑，成员经 vtable notify_members 回调出。
/// start 入参：注册载荷 JSON（成员字段 + registry_url）；stop 无入参。
pub type RoamingRegistryStartFn = unsafe extern "C" fn(config: *const c_char) -> *const c_char;

// --- endpoint（2；M3 接线，v2 追加） ---

pub const ROAMING_ENDPOINT_START_SYMBOL: &str = "roaming_endpoint_start";
pub const ROAMING_ENDPOINT_STOP_SYMBOL: &str = "roaming_endpoint_stop";

/// 客户端连接面端点拉起（v2）：宿主传 `{"relay_url":"","relay_token":"",
/// "idle_timeout_secs":<u64>}`（relay_url 空 = n0 公共中继；非空 = Minimal
/// 预设 + 自建中继 RelayMap + 可选 auth token）。端点是 dial 臂（宿主
/// `roaming_dial` 消费）与 accept 臂（插件 serve → notify_incoming）的共用
/// 承载；重复 start = 停旧起新（幂等热更）。返回 `{"ok":true,"node_id":hex}`。
pub type RoamingEndpointStartFn = unsafe extern "C" fn(config: *const c_char) -> *const c_char;

// 客户端连接面端点停机（v2）：关 router + endpoint.close()，清连接注册表。
// 未运行 = 幂等 ok。stop 无独立类型——与生命周期 `RoamingStrFn` 同形
// （无入参 → 静态 JSON）。

// ---------------------------------------------------------------------------
// 宿主侧句柄
// ---------------------------------------------------------------------------

/// 已解析导出函数指针集（dlopen 后组装；`None` = 符号缺失，装载期即拒）。
///
/// 纯 fn 指针载体——nemesis-plugin 契约 crate 零依赖（不引 libloading）；
/// Library 由 loader `Box::leak`（进程生命周期），fn 指针因此恒有效。
/// 所有方法为阻塞调用，宿主侧必须 `spawn_blocking`（拍板 10）。
#[derive(Clone, Copy)]
pub struct RoamingPluginHandle {
    pub init: RoamingInitFn,
    pub start: RoamingStrFn,
    pub stop: RoamingStrFn,
    pub status: RoamingStrFn,
    pub shutdown: RoamingShutdownFn,
    pub dial: RoamingDialFn,
    pub close_conn: RoamingCloseConnFn,
    pub identity_keygen: RoamingJsonFn,
    pub identity_load: RoamingJsonFn,
    pub get_iroh_id: RoamingJsonFn,
    pub identity_set: RoamingJsonFn,
    pub trust_trust: RoamingJsonFn,
    pub trust_revoke: RoamingJsonFn,
    pub trust_unrevoke: RoamingJsonFn,
    pub trust_list: RoamingJsonFn,
    pub registry_start: RoamingRegistryStartFn,
    pub registry_stop: RoamingStrFn,
    /// v2 追加（末尾追加纪律——v1 布局前缀字节不变）。
    pub endpoint_start: RoamingEndpointStartFn,
    pub endpoint_stop: RoamingStrFn,
}

impl RoamingPluginHandle {
    fn call_str(self, f: RoamingStrFn, name: &str) -> Result<String, String> {
        let raw = unsafe { f() };
        json_from_raw(raw, name)
    }

    fn call_json(self, f: RoamingJsonFn, arg: &str, name: &str) -> Result<String, String> {
        let c = std::ffi::CString::new(arg).map_err(|_| format!("{name}: arg 内含 NUL"))?;
        let raw = unsafe { f(c.as_ptr()) };
        json_from_raw(raw, name)
    }

    /// 刷新配置（init 幂等重入）。返回 `{"ok":true}`。
    ///
    /// `vtable` 裸指针由调用方保证进程生命周期有效（宿主 `static HOST_VTABLE`
    /// ——契约同文）；clippy 无法穿透 FFI 回调看见「插件侧只读拷贝不存指针」
    /// 的契约语义，allow + 注释即契约文书化。
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn init(self, vtable: *const HostRoamingVTable, config_json: &str) -> Result<(), String> {
        let c =
            std::ffi::CString::new(config_json).map_err(|_| "config_json 含 NUL".to_string())?;
        let rc = unsafe { (self.init)(vtable, c.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("roaming_init 返回 {rc}"))
        }
    }

    pub fn start(self) -> Result<String, String> {
        self.call_str(self.start, "roaming_start")
    }

    pub fn stop(self) -> Result<String, String> {
        self.call_str(self.stop, "roaming_stop")
    }

    /// 运行态 JSON（含 registry_members 数组）。
    pub fn status(self) -> Result<String, String> {
        self.call_str(self.status, "roaming_status")
    }

    /// M3 接线：拨号 → `{"ok":true,"port","conn_id"}`。
    pub fn dial(self, addr: &str) -> Result<String, String> {
        self.call_json(self.dial, addr, "roaming_dial")
    }

    /// M3 接线：按连接 id 显式关闭（半开兜底）。
    pub fn close_conn(self, conn_id: u64) -> Result<String, String> {
        let raw = unsafe { (self.close_conn)(conn_id) };
        json_from_raw(raw, "roaming_close_conn")
    }

    pub fn identity_keygen(self, force: bool) -> Result<String, String> {
        self.call_json(
            self.identity_keygen,
            &format!(r#"{{"force":{force}}}"#),
            "roaming_identity_keygen",
        )
    }

    pub fn identity_load(self) -> Result<String, String> {
        self.call_json(self.identity_load, "{}", "roaming_identity_load")
    }

    pub fn get_iroh_id(self) -> Result<String, String> {
        self.call_json(self.get_iroh_id, "{}", "roaming_get_iroh_id")
    }

    pub fn identity_set(self, secret_key_hex: &str) -> Result<String, String> {
        let arg = format!(r#"{{"secret_key_hex":"{secret_key_hex}"}}"#);
        self.call_json(self.identity_set, &arg, "roaming_identity_set")
    }

    pub fn trust_trust(self, node_id: &str) -> Result<String, String> {
        self.call_json(self.trust_trust, &json_arg(node_id), "roaming_trust_trust")
    }

    pub fn trust_revoke(self, node_id: &str) -> Result<String, String> {
        self.call_json(
            self.trust_revoke,
            &json_arg(node_id),
            "roaming_trust_revoke",
        )
    }

    pub fn trust_unrevoke(self, node_id: &str) -> Result<String, String> {
        self.call_json(
            self.trust_unrevoke,
            &json_arg(node_id),
            "roaming_trust_unrevoke",
        )
    }

    pub fn trust_list(self) -> Result<String, String> {
        self.call_json(self.trust_list, "{}", "roaming_trust_list")
    }

    pub fn registry_start(self, config_json: &str) -> Result<String, String> {
        self.call_json(self.registry_start, config_json, "roaming_registry_start")
    }

    pub fn registry_stop(self) -> Result<String, String> {
        self.call_str(self.registry_stop, "roaming_registry_stop")
    }

    /// v2：客户端连接面端点拉起（幂等热更；宿主 setup_roaming 消费）。
    pub fn endpoint_start(self, config_json: &str) -> Result<String, String> {
        self.call_json(self.endpoint_start, config_json, "roaming_endpoint_start")
    }

    /// v2：客户端连接面端点停机（幂等；宿主 stop 链消费）。
    pub fn endpoint_stop(self) -> Result<String, String> {
        self.call_str(self.endpoint_stop, "roaming_endpoint_stop")
    }
}

fn json_arg(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// 从插件返回的裸指针收集 JSON 字符串（插件侧静态缓冲 per-call 覆盖——立即拷出）。
fn json_from_raw(raw: *const c_char, name: &str) -> Result<String, String> {
    if raw.is_null() {
        return Err(format!("{name}: 返回空指针"));
    }
    unsafe {
        std::ffi::CStr::from_ptr(raw)
            .to_str()
            .map(|s| s.to_string())
            .map_err(|_| format!("{name}: 返回非 UTF-8"))
    }
}
