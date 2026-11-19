//! Mail 插件 C-ABI 契约（v1）。
//!
//! 双向复制纪律（roaming.rs 同款）：本文件是宿主侧真相源，`plugins/plugin-mail/`
//! 持有逐字节副本（src/mail.rs），drift 测试钉死相等。修改本文件 = 同时修改两处
//! + bump `HOST_MAIL_ABI_VERSION`。
//!
//! # 版本史
//!
//! - v1（邮件基础设施子系统，2026-11-18）：生命周期 3 + 服务端 3 + 客户端 1 +
//!   存储 6 + 地址 3 = **16 导出**；host vtable = log / get_home_dir / audit_event /
//!   notify_mail_in / notify_mail_sent 五回调。
//!
//! 新式交互插件（roaming 第二例）：双向回调 + 常驻服务（SMTP/POP3 服务端自持
//! tokio runtime）+ 分组函数表——新功能先在插件内部长成内部模块（不占 ABI），
//! 稳定后开成 `mail_<子对象>_<操作>` 导出并演进版本闸。开出去的函数即冻结承诺。
//!
//! # 线程纪律
//!
//! 所有导出函数（除 `mail_abi_version`/`mail_shutdown`）内部投插件自持 runtime
//! 阻塞等结果——宿主调用侧必须 `spawn_blocking`，不卡 async executor 线程。
//!
//! # 返回 JSON 约定
//!
//! 除 `mail_abi_version`（u32）与 `mail_shutdown`（void）外，全部导出返回
//! `*const c_char`，指向插件内静态 JSON 缓冲（per-call 覆盖，单线程串行纪律）：
//! - 成功：`{"ok":true,...载荷}`（send 携带 message_id；list 携带 items 数组；
//!   stats 携带计数；status 携带 smtp/pop3 各自运行态；addr_list 携带 addresses 数组）；
//! - 失败：`{"ok":false,"err_code":"...","err_msg":"..."}`。
//!
//! # 版本闸
//!
//! vtable 首字段 `abi_version: u32` 双向闸（roaming 同款：宿主读首字段不认识即拒载，
//! 插件同样校验宿主版本）。函数指针全部 `Option` 包裹（Some = 宿主实现 / None = 不支持），
//! 向前兼容 = 末尾追加 + 版本 +1。
//!
//! # 结构性隔离（能力矩阵：agent 面可发、可收、可读、不可删）
//!
//! 邮件库落 `<home>/mail/`（宿主 get_home_dir 回 home 根，插件拼 `mail/` 子目录）
//! ——主 workspace 之外，LLM 文件工具（restrict_to_workspace 围栏）结构性够不着，
//! 防删信掩盖。`mail_delete` 只经本 ABI 暴露给宿主治理面（WSAPI/CLI），**不在
//! agent 工具面供给**（B4）；地址注册表同属宿主治理面，agent 面不含。

use std::os::raw::{c_char, c_int};

/// 宿主侧 ABI 版本（插件侧同款常量双向闸）。
pub const HOST_MAIL_ABI_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// host → plugin vtable（宿主实现，init 时传入插件）
// ---------------------------------------------------------------------------

/// 日志级别（vtable `log` 第一参数；与 tracing 级别对齐）。
pub const MAIL_LOG_TRACE: i32 = 0;
pub const MAIL_LOG_DEBUG: i32 = 1;
pub const MAIL_LOG_INFO: i32 = 2;
pub const MAIL_LOG_WARN: i32 = 3;
pub const MAIL_LOG_ERROR: i32 = 4;

/// 日志回调：插件日志转发回宿主 tracing（转发不落盘，插件不落自己的日志文件）。
/// tag 为短标签（如 "smtp" / "pop3" / "store"），宿主负责并入消息体。
pub type MailLogFn = extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char);

/// home 根目录回调：把 home 根绝对路径写入 buf（UTF-8 + NUL）。邮件库 =
/// `<home>/mail/{<地址>/{new,cur,tmp},...}` + 注册表等插件自管文件，路径全部
/// 由插件自拼（结构性隔离的路径锚点）。返回值 = **写入字节数（>0 成功）**；
/// -1 = 缓冲不足（截断）或宿主不支持（roaming `write_cstr` 同款长度语义，
/// 勿改回「0=成功」布尔语义）。
pub type MailHomeDirFn = extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32;

/// 安全审计回调：收信/发信/删信/地址注册变更等事件进宿主审计链。
/// event = 事件类别短串，detail = JSON 明细。
pub type MailAuditFn = extern "C" fn(event: *const c_char, detail: *const c_char);

/// 收信通知回调：SMTP 服务端落信后通知宿主（宿主消费点：mail 通道入站回灌、
/// mailbox.updated WS push）。meta_json = 收信元数据载荷。
pub type MailNotifyInFn = extern "C" fn(meta_json: *const c_char);

/// 发信结果回调：客户端发信完成后通知宿主（宿主消费点：发信审计/回执）。
/// result_json = 发信结果载荷。
pub type MailNotifySentFn = extern "C" fn(result_json: *const c_char);

/// host → plugin 回调表。init 时由宿主构造传入；首字段为版本闸。
#[repr(C)]
pub struct HostMailVTable {
    /// vtable 版本（== `HOST_MAIL_ABI_VERSION`），双向闸锚点。
    pub abi_version: u32,
    pub log: Option<MailLogFn>,
    pub get_home_dir: Option<MailHomeDirFn>,
    pub audit_event: Option<MailAuditFn>,
    pub notify_mail_in: Option<MailNotifyInFn>,
    pub notify_mail_sent: Option<MailNotifySentFn>,
}

// ---------------------------------------------------------------------------
// plugin → host 导出（分组函数表，符号统一 mail_ 前缀，共 16）
// ---------------------------------------------------------------------------

// --- 生命周期（3） ---

pub const MAIL_ABI_VERSION_SYMBOL: &str = "mail_abi_version";
pub const MAIL_INIT_SYMBOL: &str = "mail_init";
pub const MAIL_SHUTDOWN_SYMBOL: &str = "mail_shutdown";

/// 读 ABI 版本（宿主装载闸第一道）。
pub type MailAbiVersionFn = extern "C" fn() -> u32;

/// 初始化：传宿主回调表 + 配置 JSON（`config.mail.json` 规范化后的 serde
/// 形态：enabled 总闸 + smtp/pop3/client 各段 + local_domains 等）。返回
/// 0 = 成功；非 0 = 拒载/配置不可用（0/-1/-2/-3/-4/-5 约定见插件侧）。
/// 重复调用 = 刷新配置（幂等放行）。
pub type MailInitFn =
    unsafe extern "C" fn(vtable: *const HostMailVTable, config_json: *const c_char) -> c_int;

/// 优雅停机（gateway 进程停机链专用；**estop 不触达传输面**——SMTP/POP3
/// 服务端是邮件基础设施不是 agent 活动面）。停服务不卸库（Library 进程
/// 生命周期，防 UB）。
pub type MailShutdownFn = extern "C" fn();

// --- 服务端（3） ---

pub const MAIL_SERVER_START_SYMBOL: &str = "mail_server_start";
pub const MAIL_SERVER_STOP_SYMBOL: &str = "mail_server_stop";
pub const MAIL_SERVER_STATUS_SYMBOL: &str = "mail_server_status";

/// 启动服务端：按 config 各自的 `smtp.enabled` / `pop3.enabled` 起停
/// （入参为覆盖配置 JSON，空对象 = 按 init 配置；per-call 覆盖支持单面
/// 重启）。返回 `{"ok":true,"smtp":{...},"pop3":{...}}` 或失败 JSON。
pub type MailJsonFn = unsafe extern "C" fn(arg: *const c_char) -> *const c_char;

/// 停服务端（两面全停；未运行 = 幂等 ok）。无独立类型——与生命周期
/// `MailStrFn` 同形（无入参 → 静态 JSON）。
pub type MailStrFn = unsafe extern "C" fn() -> *const c_char;

// --- 客户端（1） ---

pub const MAIL_SEND_SYMBOL: &str = "mail_send";

/// 发信（三 C 字符串参）。投递走 config.client 段的 SMTP relay（465
/// implicit TLS）；发完经 notify_mail_sent 回宿主。返回
/// `{"ok":true,"message_id":...}` 或失败 JSON。
pub type MailSendFn = unsafe extern "C" fn(
    to: *const c_char,
    subject: *const c_char,
    body: *const c_char,
) -> *const c_char;

// --- 存储（6） ---

pub const MAIL_LIST_SYMBOL: &str = "mail_list";
pub const MAIL_READ_SYMBOL: &str = "mail_read";
pub const MAIL_MARK_READ_SYMBOL: &str = "mail_mark_read";
pub const MAIL_DELETE_SYMBOL: &str = "mail_delete";
pub const MAIL_STATS_SYMBOL: &str = "mail_stats";
pub const MAIL_RECORD_SYMBOL: &str = "mail_record";

// 列信：query JSON `{"mailbox":"<地址,可选,缺=默认主库>","unread_only":bool,
// "limit":u32,"offset":u32}`。返回 items 数组（id/from/subject/date/seen）。
// （类型 = `MailJsonFn`，上面服务端组已声明。）

// 读信全文：入参邮件 id。返回 `{"ok":true,"meta":{...},"body":"..."}`。
// （类型 = `MailJsonFn`。）

// 标记已读：入参邮件 id。（类型 = `MailJsonFn`。）

// 删信：入参邮件 id。**仅宿主治理面（WSAPI/CLI）可达**——agent 工具面
// 不供给（能力矩阵「不可删」）。（类型 = `MailJsonFn`。）

// 库统计：各注册地址分库计数（总数/未读/容量字节）。无入参。
// （类型 = `MailStrFn`。）

// 收信记录登记：宿主把外部来源（如 email 通道 IMAP 轮询到的信）的元数据
// JSON 写入本插件库统一纳管（去重键 = message_id）。入参 meta JSON。
// （类型 = `MailJsonFn`。）

// --- 地址（3） ---

pub const MAIL_ADDR_ADD_SYMBOL: &str = "mail_addr_add";
pub const MAIL_ADDR_REMOVE_SYMBOL: &str = "mail_addr_remove";
pub const MAIL_ADDR_LIST_SYMBOL: &str = "mail_addr_list";

// 注册收信地址：入参 `{"address":"zoo@xxx.com"}`（local part 命名自由，
// 域须 ∈ local_domains；重复注册幂等）。注册后 RCPT 两级判定第二级命中、
// POP3 USER 可达本库。（类型 = `MailJsonFn`。）

// 注销地址：入参地址串。只停收不删库（历史信保留、可重新注册）。
// （类型 = `MailJsonFn`。）

// 列注册地址：无入参。返回 `{"ok":true,"addresses":[...],"default":...}`。
// （类型 = `MailStrFn`。）

// ---------------------------------------------------------------------------
// 宿主侧句柄
// ---------------------------------------------------------------------------

/// 已解析导出函数指针集（dlopen 后组装；`None` = 符号缺失，装载期即拒）。
///
/// 纯 fn 指针载体——nemesis-plugin 契约 crate 零依赖（不引 libloading）；
/// Library 由 loader `Box::leak`（进程生命周期），fn 指针因此恒有效。
/// 所有方法为阻塞调用，宿主侧必须 `spawn_blocking`。
#[derive(Clone, Copy)]
pub struct MailPluginHandle {
    pub init: MailInitFn,
    pub shutdown: MailShutdownFn,
    pub server_start: MailJsonFn,
    pub server_stop: MailStrFn,
    pub server_status: MailStrFn,
    pub send: MailSendFn,
    pub list: MailJsonFn,
    pub read: MailJsonFn,
    pub mark_read: MailJsonFn,
    pub delete: MailJsonFn,
    pub stats: MailStrFn,
    pub record: MailJsonFn,
    pub addr_add: MailJsonFn,
    pub addr_remove: MailJsonFn,
    pub addr_list: MailStrFn,
}

impl MailPluginHandle {
    fn call_str(self, f: MailStrFn, name: &str) -> Result<String, String> {
        let raw = unsafe { f() };
        json_from_raw(raw, name)
    }

    fn call_json(self, f: MailJsonFn, arg: &str, name: &str) -> Result<String, String> {
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
    pub fn init(self, vtable: *const HostMailVTable, config_json: &str) -> Result<(), String> {
        let c =
            std::ffi::CString::new(config_json).map_err(|_| "config_json 含 NUL".to_string())?;
        let rc = unsafe { (self.init)(vtable, c.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("mail_init 返回 {rc}"))
        }
    }

    /// 启动服务端（config 覆盖 JSON，空对象 = 按 init 配置）。
    pub fn server_start(self, override_json: &str) -> Result<String, String> {
        self.call_json(self.server_start, override_json, "mail_server_start")
    }

    /// 停服务端（两面全停；幂等）。
    pub fn server_stop(self) -> Result<String, String> {
        self.call_str(self.server_stop, "mail_server_stop")
    }

    /// 服务端运行态（smtp/pop3 各自状态 + 计数）。
    pub fn server_status(self) -> Result<String, String> {
        self.call_str(self.server_status, "mail_server_status")
    }

    /// 发信（三参；投递走 config.client relay，发完 notify_mail_sent）。
    pub fn send(self, to: &str, subject: &str, body: &str) -> Result<String, String> {
        let (c_to, c_subject, c_body) = (
            std::ffi::CString::new(to).map_err(|_| "mail_send: to 含 NUL".to_string())?,
            std::ffi::CString::new(subject).map_err(|_| "mail_send: subject 含 NUL".to_string())?,
            std::ffi::CString::new(body).map_err(|_| "mail_send: body 含 NUL".to_string())?,
        );
        let raw = unsafe { (self.send)(c_to.as_ptr(), c_subject.as_ptr(), c_body.as_ptr()) };
        json_from_raw(raw, "mail_send")
    }

    /// 列信（query JSON：mailbox/unread_only/limit/offset）。
    pub fn list(self, query_json: &str) -> Result<String, String> {
        self.call_json(self.list, query_json, "mail_list")
    }

    /// 读信全文（入参邮件 id）。
    pub fn read(self, id: &str) -> Result<String, String> {
        self.call_json(self.read, id, "mail_read")
    }

    /// 标记已读（入参邮件 id）。
    pub fn mark_read(self, id: &str) -> Result<String, String> {
        self.call_json(self.mark_read, id, "mail_mark_read")
    }

    /// 删信（入参邮件 id；**仅宿主治理面可达**）。
    pub fn delete(self, id: &str) -> Result<String, String> {
        self.call_json(self.delete, id, "mail_delete")
    }

    /// 库统计（各注册地址分库计数）。
    pub fn stats(self) -> Result<String, String> {
        self.call_str(self.stats, "mail_stats")
    }

    /// 收信记录登记（外部来源元数据 JSON 统一纳管，message_id 去重）。
    pub fn record(self, meta_json: &str) -> Result<String, String> {
        self.call_json(self.record, meta_json, "mail_record")
    }

    /// 注册收信地址（`{"address":"..."}`；幂等）。
    pub fn addr_add(self, address: &str) -> Result<String, String> {
        let arg = format!(r#"{{"address":{}}}"#, json_arg(address));
        self.call_json(self.addr_add, &arg, "mail_addr_add")
    }

    /// 注销地址（只停收不删库）。
    pub fn addr_remove(self, address: &str) -> Result<String, String> {
        self.call_json(self.addr_remove, address, "mail_addr_remove")
    }

    /// 列注册地址。
    pub fn addr_list(self) -> Result<String, String> {
        self.call_str(self.addr_list, "mail_addr_list")
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
