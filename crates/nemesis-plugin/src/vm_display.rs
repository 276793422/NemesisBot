//! plugin-vm 显示通道 vtable（P5-R2 ④）——host 导出给 plugin-vm 的
//! vm-display 窄接口。
//!
//! **本文件是唯一权威定义**。plugins/plugin-vm cdylib（非 workspace
//! member）通过复制此文件保持同步（host_services.rs 同款纪律）。
//!
//! 与通用 [`HostServices`] 的关系：同一个 C-ABI 家族（HostServices vtable
//! 保持冻结只维护），vm-display 是**解冻首例**——窄接口（只服务显示通
//! 道）、独立版本号、**双向版本闸首立**（host 校验 plugin 上报的
//! `plugin_abi_version`、plugin 校验 host 下发的 `abi_version`，任一侧
//! 不相等 = 拒绝装载/初始化失败。HostServices 时代只做单向前向兼容，
//! 从未做过双向闸——plugin-vm 是一方插件，靠自身 v4 签名机制保证安全，
//! 通路不必封死，但 ABI 相等性必须双向确认）。
//!
//! 体积纪律：类型本体零依赖（只 `std::os::raw`），不拖任何运行时进宿主。

use std::os::raw::c_char;

/// vm-display vtable ABI 版本。结构体布局 / 函数语义 / wire 协议变更必须
/// +1；双向闸两侧都锚定本常量。
pub const HOST_VM_DISPLAY_ABI_VERSION: u32 = 1;

/// `display_port_query` 返回值约定：`>0` = 该 agent 的 QEMU VNC 回环端口；
/// 负数错误码：
/// - [`PORT_QUERY_ERR_UNKNOWN`]（-1）：未知 agent id；
/// - [`PORT_QUERY_ERR_NOT_RUNNING`]（-2）：agent 存在但 VM 未运行；
/// - [`PORT_QUERY_ERR_NO_DISPLAY`]（-3）：未开显示（config display=none）；
/// - [`PORT_QUERY_ERR_INTERNAL`]（-4）：宿主内部错误（supervisor 缺席等）。
pub const PORT_QUERY_ERR_UNKNOWN: i32 = -1;
pub const PORT_QUERY_ERR_NOT_RUNNING: i32 = -2;
pub const PORT_QUERY_ERR_NO_DISPLAY: i32 = -3;
pub const PORT_QUERY_ERR_INTERNAL: i32 = -4;

/// `plugin_vm_display_init` 返回值约定：`0` = 成功（out 已填）；负数错误码：
/// -1 参数无效；-2 host ABI 不匹配（双向闸 plugin 侧拒绝）；-3 内部错误
/// （监听端口分配失败 / runtime 启动失败等）。
pub const INIT_ERR_INVALID_ARG: i32 = -1;
pub const INIT_ERR_ABI_MISMATCH: i32 = -2;
pub const INIT_ERR_INTERNAL: i32 = -3;

/// host 导出给 plugin-vm 的 vtable。所有函数指针用 `Option` 包裹
/// （HostServices 同款：`Some` = host 实现，`None` = 不支持，plugin 不应
/// 调用）。向前兼容：新函数追加在末尾，旧字段位置不变（配合版本 +1）。
#[repr(C)]
pub struct HostVmDisplayVTable {
    /// ABI 版本（双向闸 host 侧半边；plugin init 时校验
    /// `== HOST_VM_DISPLAY_ABI_VERSION`，不等 = 返回
    /// [`INIT_ERR_ABI_MISMATCH`]）。
    pub abi_version: u32,

    /// 日志：经宿主 tracing 输出。level: 0=trace, 1=debug, 2=info, 3=warn,
    /// 4=error。tag = 组件名（如 "plugin-vm"）。
    pub log: Option<extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char)>,

    /// workspace 根目录路径写入 buf（UTF-8，`\0` 结尾），返回写入字节数
    /// （不含 `\0`），负数 = 错误。
    pub get_workspace_dir: Option<extern "C" fn(buf: *mut c_char, buf_len: usize) -> i32>,

    /// 查询 agent 的 QEMU VNC 回环端口（[`PORT_QUERY_ERR_*`] 错误码见上）。
    /// 插件按会话的 agent_id 调用——本 shim 宿主不持 VM 状态，端口解析
    /// 责任在插件侧。
    pub display_port_query: Option<extern "C" fn(agent_id: *const c_char) -> i32>,

    /// 审计事件上报：宿主落 tracing（security feature 在位时再落审计链）。
    /// event = 事件名（如 "session_open" / "session_close"），detail =
    /// 自由文本（JSON 串）。
    pub audit_event: Option<extern "C" fn(event: *const c_char, detail: *const c_char)>,
}

/// plugin-vm init 产物（宿主分配栈上结构，plugin 填写）。
#[repr(C)]
pub struct PluginVmInfo {
    /// 插件侧 ABI 版本（双向闸 plugin 侧半边；host 校验
    /// `== HOST_VM_DISPLAY_ABI_VERSION` 才 set_endpoint）。
    pub plugin_abi_version: u32,
    /// 插件回环监听端口（插件自选空闲端口后上报；网关 shim 作为 TCP
    /// 客户端连 127.0.0.1 —— 插件是 listener，网关永不监听）。
    pub listen_port: u16,
}

/// C ABI 约定（`plugin_vm.dll` / `libplugin_vm.so` / `libplugin_vm.dylib`
/// 必须导出）：
///
/// ```c
/// int plugin_vm_display_init(const HostVmDisplayVTable *host, PluginVmInfo *out);
/// void plugin_vm_display_free(void);
/// ```
///
/// - `plugin_vm_display_init`：校验 host ABI → 启动内部 tokio runtime +
///   回环 listener → 填 `out`。成功前不持有任何宿主资源。
/// - `plugin_vm_display_free`：停 listener + shutdown runtime。幂等；
///   进程退出时宿主卸载库前调用。
pub const INIT_SYMBOL: &str = "plugin_vm_display_init";
pub const FREE_SYMBOL: &str = "plugin_vm_display_free";
