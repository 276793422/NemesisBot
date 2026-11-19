//! 太微子系统 FFI 桥接层（rlib 直连形态，`cfg(nb_taiwei)` 门控）。
//!
//! 2026-11-13 最终形态：nemesis-taiwei 以 rlib+ThinLTO 融合（不再是
//! cargo 依赖、不再是 opaque archive 直链）——`.cargo/config.toml` 静态
//! `--extern` 把 `slots/taiwei/impl/target/release/libnemesis_taiwei.rlib`
//! 挂进 extern 搜索面，build.rs 探测在场后 emit `cfg(nb_taiwei)`；宿主
//! `lto="thin"` 的 ThinLTO 管线吸收 taiwei bitcode（opaque archive 直链
//! 在 LTO 下结构性不可行——std 符号编译期已内化，链接期无定义可拉）。
//! 本模块是宿主侧唯一 FFI 面：
//! - ABI 握手（每次入口调用前对质 `nb_tw_abi_version`，不符 = 库与桥接层
//!   构建错配，loud 拒绝）。**该调用同时是 crate 激活点**：`use` 路径的
//!   Rust 层引用让 rustc 把 taiwei rlib 载入 LTO 管线——裸 extern "C"
//!   声明不会激活 crate（rlib 不激活 = bitcode 不进 LTO = 符号无定义）；
//! - clap 子命令枚举（与旧 `nemesis_taiwei::host::TaiweiAction` 同构，UX
//!   逐字段保持——解析后序列化为 action JSON 过 FFI，值集校验单点在
//!   taiwei 侧 `ffi::parse_event_kind` 等）；
//! - gateway 接线（spawn_startup / spawn_exit / wait_exit / privacy_md）。
//!
//! 库缺席形态（开源构建）：本模块整体不编译，`taiwei` 子命令不存在、
//! gateway 接线点全部 cfg 消失——与直链前的 OSS 语义一致。
//!
//! 错误面纪律：FFI 动作失败时 err_buf 文本回传（taiwei 侧产出的中文人读
//! 消息），宿主原样打印 + 退码 1；emit 保持静默成功语义（禁用/离线不是
//! 错误，taiwei 侧内部消化）。

use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};

use clap::Subcommand;

// crate 激活引用（Rust 层路径调用进 abi_check；符号级细节见模块文档）。
use nemesis_taiwei::nb_tw_abi_version;

/// 与 taiwei-impl `ffi::ABI_VERSION` 对质（两侧常量同源维护；运行期握手
/// 兜底防陈旧库残留——错配 = 硬错而非静默畸形）。
const NB_TW_ABI_VERSION: u32 = 1;

/// err_buf 容量（人读错误消息上限；截断可接受）。
const ERR_BUF_LEN: usize = 4096;

unsafe extern "C" {
    fn nb_tw_privacy_md() -> *const c_char;
    fn nb_tw_action(action_json: *const c_char, err_buf: *mut c_char, err_len: usize) -> i32;
    fn nb_tw_spawn_startup(
        home: *const c_char,
        workspace: *const c_char,
        version: *const c_char,
    ) -> i32;
    fn nb_tw_spawn_exit(
        home: *const c_char,
        workspace: *const c_char,
        version: *const c_char,
        duration_secs: u64,
    ) -> i32;
    fn nb_tw_exit_wait(wait_ms: u32) -> i32;
}

// ---------------------------------------------------------------------------
// clap 子命令面（与旧 nemesis_taiwei::host::TaiweiAction 同构）
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum TaiweiAction {
    /// 上报一条事件（安装器/卸载链通道；静默语义，失败非 0 退码）。
    Emit {
        /// 事件类型: install | startup | exit | uninstall
        ///（位置参数——安装器 spawn 形态 `taiwei emit install --install-id X`
        /// 的既有契约，installer/windows+linux ops.rs 同构消费）
        kind: String,
        /// install 专属：安装事件幂等 ID
        #[arg(long)]
        install_id: Option<String>,
        /// exit 专属：进程运行时长（秒）
        #[arg(long)]
        duration_secs: Option<u64>,
        /// exit 专属：退出原因（自由文本，透传）
        #[arg(long)]
        exit_reason: Option<String>,
        /// uninstall 专属: not_useful | issue | reinstall | other
        #[arg(long)]
        reason: Option<String>,
        /// 覆盖上报端点（缺省 = 内建默认/配置/env 解析链）
        #[arg(long)]
        endpoint: Option<String>,
        /// 指定工作区（缺省 = home 下标准 workspace）
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// 打印透明度状态（遥测开关/机器标识/待补队列）。
    Status {
        /// 指定工作区
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// 开启使用统计上报。
    Enable,
    /// 关闭使用统计上报。
    Disable,
    /// 开启精确定位（startup 事件附一次 OS 级定位，绝不轮询）。
    #[command(name = "enable-precise")]
    PreciseEnable,
    /// 关闭精确定位。
    #[command(name = "disable-precise")]
    PreciseDisable,
    /// 机器级设备标识管理（安装器提权语境）。
    #[command(subcommand)]
    MachineId(MachineIdCommand),
}

#[derive(Debug, Subcommand)]
pub enum MachineIdCommand {
    /// 读取/创建机器级设备标识并打印（幂等——升级/重跑安装器绝不重置身份）。
    Ensure,
    /// 清除机器级设备标识（机器存储 + VM 缓存，可选连带 workspace 兜底）。
    Remove {
        /// 附带清理的 workspace（删除 config/ 下标识兜底文件）
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
}

// ---------------------------------------------------------------------------
// CLI 动作（nb_tw_action 门面）
// ---------------------------------------------------------------------------

/// 执行一条 taiwei CLI 动作。Err(String) = 失败（宿主 main 打印 + 退码 1）。
/// `home`/`default_ws` 由宿主解析后显式传入（--local 语义在宿主侧收敛，
/// FFI 侧不做任何路径推断）。
pub fn run_cli_action(action: TaiweiAction, home: &Path, default_ws: &Path) -> Result<(), String> {
    abi_check();
    let json = build_action_json(&action, home, default_ws);
    let c_json = CString::new(json).map_err(|_| "action JSON 含 NUL")?;
    let mut err_buf = vec![0 as c_char; ERR_BUF_LEN];
    // SAFETY: c_json 合法 NUL 结尾 C 字符串；err_buf 由本函数分配（4KB），
    // nb_tw_action 只写不越界（截断 + NUL 终止是 taiwei 侧契约）
    let code = unsafe { nb_tw_action(c_json.as_ptr(), err_buf.as_mut_ptr(), ERR_BUF_LEN) };
    if code == 0 {
        return Ok(());
    }
    let msg = unsafe { CStr::from_ptr(err_buf.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string();
    if msg.is_empty() {
        Err(format!("taiwei 动作失败（code={code}）"))
    } else {
        Err(msg)
    }
}

/// 构建 action JSON（形态与 taiwei-impl `ffi::FfiAction` 反序列化端逐字段
/// 对齐；两侧漂移由 taiwei ffi tests + 本模块 tests 双侧钉住）。
fn build_action_json(action: &TaiweiAction, home: &Path, default_ws: &Path) -> String {
    use serde_json::json;
    let _ = home; // 保留参数位（workspace 缺省语义 = default_ws，见下）
    match action {
        TaiweiAction::Emit {
            kind,
            install_id,
            duration_secs,
            exit_reason,
            reason,
            endpoint,
            workspace,
        } => json!({
            "action": "emit",
            "kind": kind,
            "install_id": install_id,
            "duration_secs": duration_secs,
            "exit_reason": exit_reason,
            "reason": reason,
            "endpoint": endpoint,
            "workspace": workspace_or_default(workspace, default_ws),
        })
        .to_string(),
        TaiweiAction::Status { workspace } => json!({
            "action": "status",
            "workspace": workspace_or_default(workspace, default_ws),
        })
        .to_string(),
        TaiweiAction::Enable => json!({"action": "enable"}).to_string(),
        TaiweiAction::Disable => json!({"action": "disable"}).to_string(),
        TaiweiAction::PreciseEnable => json!({"action": "enable-precise"}).to_string(),
        TaiweiAction::PreciseDisable => json!({"action": "disable-precise"}).to_string(),
        TaiweiAction::MachineId(MachineIdCommand::Ensure) => {
            json!({"action": "machine-id", "sub": "ensure"}).to_string()
        }
        TaiweiAction::MachineId(MachineIdCommand::Remove { workspace }) => {
            // remove 的 workspace 是「可选连带清理」语义——不填缺省
            //（None = 只清机器存储与缓存，不碰 workspace 兜底文件）
            json!({
                "action": "machine-id",
                "sub": "remove",
                "workspace": workspace.as_ref().map(|p| p.to_string_lossy()),
            })
            .to_string()
        }
    }
}

/// status/emit 的 workspace 解析：CLI 显式指定 > 宿主解析的标准 workspace
///（桥接侧恒传显式路径——FFI 侧的 home 推断兜底永远不该被触发）。
fn workspace_or_default(workspace: &Option<PathBuf>, default_ws: &Path) -> String {
    match workspace {
        Some(p) => p.to_string_lossy().into_owned(),
        None => default_ws.to_string_lossy().into_owned(),
    }
}

// ---------------------------------------------------------------------------
// gateway 接线（spawn_startup / spawn_exit / wait_exit / privacy_md）
// ---------------------------------------------------------------------------

/// gateway 装配完成后的启动遥测：spawn taiwei 侧后台线程（自建 runtime），
/// 立即返回不阻塞就绪。失败 eprintln 留痕不 panic（遥测永不拖垮 gateway）。
pub fn spawn_startup(home: &Path, workspace: &Path, version: &'static str) {
    abi_check();
    let c_home = path_cstring(home);
    let c_ws = path_cstring(workspace);
    let c_ver = CString::new(version).unwrap_or_default();
    // SAFETY: 三个 CStr 在调用期间存活；taiwei 侧在 spawn 线程前已拷贝成
    // 自有数据（catch_unwind 闭包体内同步转换）
    let rc = unsafe { nb_tw_spawn_startup(c_home.as_ptr(), c_ws.as_ptr(), c_ver.as_ptr()) };
    if rc != 0 {
        eprintln!("taiwei: startup 遥测 spawn 失败（rc={rc}）");
    }
}

/// gateway 停机遥测：spawn taiwei 侧后台线程执行 exit 全链；完成信号由
/// [`wait_exit`] 在 teardown 收尾处有界等待。
pub fn spawn_exit(home: &Path, workspace: &Path, version: &'static str, duration_secs: u64) {
    abi_check();
    let c_home = path_cstring(home);
    let c_ws = path_cstring(workspace);
    let c_ver = CString::new(version).unwrap_or_default();
    // SAFETY: 同 spawn_startup
    let rc = unsafe {
        nb_tw_spawn_exit(
            c_home.as_ptr(),
            c_ws.as_ptr(),
            c_ver.as_ptr(),
            duration_secs,
        )
    };
    if rc != 0 {
        eprintln!("taiwei: exit 遥测 spawn 失败（rc={rc}）");
    }
}

/// teardown 收尾：有界等待 exit 遥测落盘。true = 完成；false = 超时
///（硬杀场景 exit 丢失的诚实边界——与旧 tokio timeout(3s) 语义对齐）。
pub fn wait_exit(wait_ms: u32) -> bool {
    abi_check();
    // SAFETY: 纯数值参数
    unsafe { nb_tw_exit_wait(wait_ms) == 0 }
}

/// 隐私声明全文（静态库内嵌资产 `PRIVACY.md`）。返回空串 = taiwei 库侧
/// 异常（panic 被 catch），调用方按「缺资产」降级。
pub fn privacy_md() -> String {
    abi_check();
    // SAFETY: 返回指针指向 taiwei 库静态区（永不释放、NUL 结尾），只读拷贝
    let p = unsafe { nb_tw_privacy_md() };
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

/// ABI 握手（每次入口调用前；assert 失败 = 库与桥接层构建错配，消息给出
/// 修复路径）。经 Rust 层路径调用 `nemesis_taiwei::nb_tw_abi_version`——
/// 这是 taiwei crate 进宿主 ThinLTO 管线的激活点（rlib 不激活则 bitcode
/// 不进管线，其余五个裸 extern "C" 符号将无定义）。
fn abi_check() {
    // taiwei 侧 `pub extern "C" fn`（safe）——Rust 路径直调
    let got = nb_tw_abi_version();
    assert_eq!(
        got, NB_TW_ABI_VERSION,
        "taiwei rlib ABI 版本错配（库={got}，桥={NB_TW_ABI_VERSION}）——重编 slots/taiwei/impl"
    );
}

fn path_cstring(p: &Path) -> CString {
    CString::new(p.as_os_str().to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests;
