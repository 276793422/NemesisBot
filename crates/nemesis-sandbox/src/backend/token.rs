//! DACL 定向档 D3：write-restricted 受限令牌 + CreateProcessAsUserW spawn。
//!
//! 设计文档：`docs/PLAN/2026-09-27_windows-acl-targeted-deny-design.md` §2/§3.3。
//! Windows + `acl` feature 编译（与 acl_impl 同门控）。
//!
//! ## 为什么不是 `SandboxBackend` trait 的第三形态
//!
//! 受限令牌**必须由父进程铸造并施加给子进程**（`CreateProcessAsUserW`）——
//! 进程不能把自己的令牌换成受限版，所以 trait 的两个既有形态都不适用：
//! `apply_to_self`（子进程自装，landlock 式）语义上不可能；
//! `wrap_command`（包装器 spawn，bwrap/Start.exe 式）没有令牌入参。本模块
//! 因此是**独立 spawn 原语库**：gateway 装配点铸造令牌（一次），以闭包事务
//! 形式注入 `ExecutorChannel`（nemesis-agent 刻意不依赖 nemesis-sandbox，
//! 决策/能力以闭包传入——与 `sandbox_probe`/`userland_fallback` 同款模式）。
//!
//! ## 令牌语义（write-restricted，2026-09-27 实证定稿）
//!
//! `CreateRestrictedToken`（无需特权）+ flags `WRITE_RESTRICTED |
//! DISABLE_MAX_PRIVILEGE` + restricting SIDs = `[白名单组面（现仅 Everyone）
//! …, logon sid, workspace_sid]`。内核对 restricted 令牌做**双合取**访问
//! 检查（MSDN《Restricted Tokens》）：普通 SID 列表与 restricting 列表都
//! 要放行，访问才成立；`WRITE_RESTRICTED` flag（0x8，Vista+）让
//! restricting 检查**只在评估写类访问时**进行——读/执行只走普通检查。
//!
//! - **组面白名单**（本档围栏的形状）：restricting 只含 Everyone
//!   （S-1-1-0）+ logon sid——第二检查的组面开口最小化。演进实证：
//!   * restricting 严格窄于第一检查（首版 `[ws_sid, S-1-5-12]`）→ 子进程
//!     初始化必死（cmd.exe/whoami/自身 全部 0xC0000142 秒死）；
//!   * 全组镜像 → Administrators（S-1-5-32-544）与 S-1-5-113 随组面入列，
//!     admin 账号上机器全盘的 Administrators:(F) ACE 穿透第二检查，围栏
//!     全空（fence 子进程树外写成功，exit 23）；
//!   * 九组白名单（首版生产）→ 生产任务 B 实锤围栏失守：Authenticated
//!     Users 在 restricting 列表，而 **C:\ 根默认 DACL 给 Authenticated
//!     Users Modify（OI/CI 继承，Windows 标准形态）**→ 所有用户自建目录
//!     树（含本仓库 target/）在 restricting 侧放行，cmd 重定向写
//!     `target\dacl-uat-outside.txt` 成功；
//!   * 组面二分矩阵（token_tests `restricted_token_minimal_restricting_
//!     set_probe`）定稿：**Everyone 单组即满足初始化**（空集/au-only/
//!     users-only 全死 0xC0000142，everyone-only 活）；AU 纯开口无贡献。
//!     生产白名单 = `["S-1-1-0"]`。
//! - **`S-1-5-12` 明确不用**：它是内核 legacy write-restricted 标记（先于
//!   flag 机制的 svchost 时代产物）——只要出现在 restricting 列表，无论
//!   flag 组合如何子进程初始化必死（诊断矩阵 15 行实证）。设计 §2 原假设
//!   「混入 S-1-5-12 即 write-restricted 型」已被推翻：写只查语义来自
//!   flag 本身。
//! - workspace_sid：D1 从工作区路径确定性派生；树上预置了它的 GRANT ACE
//!   （D2 standing ACE）——写工作区内第二检查经它放行；它是 restricting
//!   列表里**唯一的扩面**。
//! - **用户自身 SID 与高特权组不进 restricting 列表**（收窄面）：用户/admin
//!   专属 DACL 上的写类访问第一检查过、第二检查无人放行 → 拒绝。
//! - `DISABLE_MAX_PRIVILEGE`：丢全部特权，**SeChangeNotifyPrivilege 除外**
//!   （MSDN 明文保留——bypass traverse checking 不受影响）；agent 子进程
//!   不需要特权（防特权路径绕过 ACL 面）。
//! - 子进程的后代**全继承** restricting SIDs（内核保证，不可剥离）——
//!   exec/async_shell 工具链全罩。
//!
//! ## spawn 事务（同步，调用方放 `spawn_blocking`）
//!
//! [`stdio_txn_raw`] 一次完成：CreateProcessAsUserW（三通匿名管道 stdio，
//! 父端清继承）→ 写请求行 → 关 stdin（EOF 信号）→ 收 stdout 响应行（读
//! 线程与主线程 `WaitForSingleObject` 超时窗并行）→ 超时 `TerminateProcess`
//! → 收尸带退出码。响应行是 `Option`——libtest 子进程等非 executor 形态
//! 的调用方可以只取退出码（executor 闭包层负责把 `None` 变 Err）。
//!
//! ## 诚实边界
//!
//! - **白名单组面的代价**：restricting 含 Everyone（S-1-1-0），Everyone
//!   授予的写位在围栏外仍可写——现代 Windows 文件系统 DACL 极少给 Everyone
//!   写类授予（%PUBLIC% 子树等个别共享位），开口面已是最小组面形态；
//!   fence e2e 断言用户/admin 专属 DACL 面与 Authenticated Users 面被拒
//!   （后者是 C:\ 根默认 Modify 的最大开口位，2026-09-28 收窄）；
//! - hardlink 缺口（工作区内对外部文件造 hardlink = 写外部数据，同一权限
//!   位无法区分）——设计 §4.2，v1 记录为已知边界；
//! - 不防网络 / 读面 / 进程注入——与完整性标签档同一结构性边界；
//! - **console 分配面（2026-09-27 全链实证定稿）**：restricted 令牌子进程
//!   对 **新** console 分配（\Device\ConDrv 写类访问）必死
//!   0xC0000142——默认 flags（console-less 父下新分配带窗口）与
//!   `CREATE_NO_WINDOW`（隐藏分配）都死，**无 SID 可救**（曾试 S-1-2-1，
//!   无效已删）。三条活路：① `DETACHED_PROCESS`（无 console，executor
//!   stdio 全管道，生产形态）；② console **继承**（有 console 的父 + 默认
//!   flags，不产生新分配）；③ `AttachConsole(ATTACH_PARENT_PROCESS)`
//!   **放行**（打开既有 condrv 非写类）——顶部进程持 console、受限树附着
//!   即可让第三方链式工具链（cargo→rustc 类默认 flags spawn）存活。
//!   诚实边界：console-less 树内第三方工具以默认 flags spawn console 子进程
//!   仍会死（我们控制不了的深层 spawn），受限模式下 exec 工具链必须全程
//!   DETACHED 或依赖顶部附着。
//! - 本档不申请 strict 合格（P24 契约：strict 只认 Sandboxie）。

use std::ffi::c_void;
use std::path::Path;

// windows-sys 0.59 模块路径备忘（同 acl_impl 顶部注释，源码已逐一对照）：
// CreateRestrictedToken/DISABLE_MAX_PRIVILEGE/TOKEN_*/PSID 在 `Security`；
// OpenProcessToken/GetCurrentProcess/CreateProcessAsUserW/Wait* 在
// `System::Threading`；CreatePipe 在 `System::Pipes`；环境串 API 在
// `System::Environment`（GetEnvironmentStringsW 返回 PWSTR，调用方 free）；
// WAIT_OBJECT_0/WAIT_TIMEOUT/HANDLE_FLAG_INHERIT 都是 Foundation 真导出常量。
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, GetTokenInformation, PSID, SECURITY_ATTRIBUTES,
    SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS,
    TOKEN_QUERY, TokenGroups, WRITE_RESTRICTED,
};
use windows_sys::Win32::System::Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess,
    OpenProcessToken, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};

/// 完整性标签组属性位（windows-sys 0.59 里挂在 `Win32_System_SystemServices`
/// feature 后面——为单个常量单开 feature 不值，本地定义，值与
/// `SystemServices::SE_GROUP_INTEGRITY = 32i32` 对照一致）。
const SE_GROUP_INTEGRITY: u32 = 0x20;

use super::Availability;

// ---------------------------------------------------------------------------
// 受限令牌
// ---------------------------------------------------------------------------

/// write-restricted 受限令牌（`CreateRestrictedToken` 产物；Drop 关句柄）。
/// 一次铸造、跨调用复用（同 workspace 派生 SID 不变——standing ACE 与令牌
/// restricting SID 恒对齐），gateway 装配点持有 `Arc`。
pub struct WriteRestrictedToken {
    /// 原始令牌句柄（crate 内只读——token_tests 查证 restricting SIDs 用）。
    pub(crate) handle: HANDLE,
}

impl Drop for WriteRestrictedToken {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

// 句柄只读共享（FFI 句柄本身是进程级内核对象，跨线程使用安全）。
unsafe impl Send for WriteRestrictedToken {}
unsafe impl Sync for WriteRestrictedToken {}

/// 组面白名单：restricting 列表的组面**只含这些 SID**（字符串直接解析进列
/// 表，不从基础令牌组里筛选）——高特权组（Administrators / S-1-5-113 /
/// Domain Admins 等）混入会让第二检查继承 admin 的全盘写面，围栏全空
/// （本机 admin 账号实证 exit 23）。白名单之外只放行 logon sid（本会话
/// 对象面）。
///
/// **为什么只有 Everyone（S-1-1-0）——2026-09-28 围栏失守排查实证**：
/// 生产任务 B 抓出「受限 exec 的 cmd 重定向写 C:\AI\NemesisBot_Rust\target\
/// 成功」——根因是 Authenticated Users（S-1-5-11）在 restricting 列表，
/// 而 **C:\ 根默认 DACL 给 Authenticated Users Modify（OI/CI 继承，Windows
/// 标准形态）**→ 所有用户自建目录树在 restricting 侧放行，围栏对整个
/// 用户数据区失效。组面二分矩阵（`restricted_token_minimal_restricting_
/// set_probe`，真进程）实证：
/// - 空集 / au-only / users-only → 子进程初始化死 0xC0000142；
/// - **everyone-only → 存活**（Everyone 单组即满足初始化的公共对象授予
///   需求）；白名单去 AU → 存活且 fence 收紧（%TEMP% 与 target/ 双拒）+
///   AttachConsole 正常。
///
/// 结论：AU 是纯开口无贡献；其余组对初始化同样非必需（单组实验）且各自
/// 在公共 DACL 上有授予位（Users → ProgramData append 等）都是开口面。
/// 生产白名单收窄为 Everyone-only = 围栏开口面最小化。
const RESTRICTING_SID_WHITELIST: &[&str] = &["S-1-1-0"]; // Everyone

/// logon 会话组属性位（winnt.h `SE_GROUP_LOGON_ID`；windows-sys 0.59 挂在
/// SystemServices 的 i32 常量，本地按 u32 定义）。
const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

/// 令牌铸造主体：组面枚举（传入的组面 SID + logon sid）+ 追加 ws_sid +
/// CreateRestrictedToken。拆成自由函数免去闭包嵌套捕获；组面 PSID 解析成功
/// 由调用方保证（失败不会进到这里）。
///
/// # SAFETY
/// `base` 必须是 TOKEN_QUERY 可查的有效令牌句柄；`extra_groups`/`ws_sid`
/// 必须是 ConvertStringSidToSidW 产物且在本次调用期间有效（SID 由内核复制，
/// 返回后即可释放）。
unsafe fn mint_restricted(
    base: HANDLE,
    extra_groups: &[PSID],
    ws_sid: PSID,
    out: &mut HANDLE,
) -> Result<(), String> {
    // edition 2024 `unsafe_op_in_unsafe_fn`：unsafe fn 体内的不安全操作仍需
    // 显式 unsafe 块（本函数体整体就是一段 FFI 事务，统一包住）。
    unsafe {
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(base, TokenGroups, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            return Err("TokenGroups 查询失败（返回长度 0）".to_string());
        }
        // u64 背书的缓冲：TOKEN_GROUPS 含 PSID 指针字段（对齐 8）——Vec<u8>
        // （align 1）指针转型是对齐面 UB；u64 槽位字节长 ceil(needed/8)*8
        // ≥ needed，GetTokenInformation 仍按 needed 写。
        let mut buf = vec![0u64; needed.div_ceil(size_of::<u64>() as u32) as usize];
        if GetTokenInformation(
            base,
            TokenGroups,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        ) == 0
        {
            return Err(last_err("GetTokenInformation(TokenGroups)"));
        }
        let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        let gbase = groups.Groups.as_ptr();
        let mut sids: Vec<SID_AND_ATTRIBUTES> = Vec::new();
        for i in 0..groups.GroupCount as usize {
            let e = *gbase.add(i);
            // logon sid 恒在（本会话对象面）；完整性标签不参与（强制完整性
            // 策略载体，不是授予面）。组面由调用方给定（生产白名单过滤在
            // create 层做）。
            if e.Attributes & SE_GROUP_INTEGRITY != 0 {
                continue;
            }
            if e.Attributes & SE_GROUP_LOGON_ID != 0 {
                sids.push(SID_AND_ATTRIBUTES {
                    Sid: e.Sid,
                    Attributes: 0,
                });
            }
        }
        // 组面 + workspace_sid 追加在 logon 之后。
        for g in extra_groups {
            sids.push(SID_AND_ATTRIBUTES {
                Sid: *g,
                Attributes: 0,
            });
        }
        sids.push(SID_AND_ATTRIBUTES {
            Sid: ws_sid,
            Attributes: 0,
        });
        let ok = CreateRestrictedToken(
            base,
            DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            sids.len() as u32,
            sids.as_ptr(),
            out,
        );
        if ok == 0 {
            return Err(last_err("CreateRestrictedToken"));
        }
        Ok(())
    }
}

/// 受限子进程尽力**附着**父进程 console（DACL 定向档生产链，D4 接线）。
/// 附着是打开既有 condrv 对象（非写类访问）——白名单 restricting 下放行
/// （2026-09-27 实证：附着成功且附着后默认 flags 孙进程存活）；成功后本
/// 进程树内默认 flags spawn 继承该 console，不再触**新分配**面 → 第三方
/// 链式工具链（cargo→rustc 类）可用。父进程 console-less（服务化启动）
/// 时失败返回 false——调用方按「全链 DETACHED」降级（exec 工具链的深层
/// 第三方 spawn 是诚实边界，见模块文档「console 分配面」）。
pub fn attach_parent_console() -> bool {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}

/// 铸造 write-restricted 受限令牌（无需特权；失败即 Err，不降级）。
///
/// flags = `WRITE_RESTRICTED | DISABLE_MAX_PRIVILEGE` + restricting SIDs =
/// `[logon sid, 白名单组面…, workspace_sid]`——语义见模块文档
/// （白名单把第二检查约束成「写时当普通用户」；ws_sid 的 standing ACE 是
/// 唯一放行扩面；用户自身 SID 与高特权组排除 = 收窄面）。调用方组合
/// `Arc<WriteRestrictedToken>` 供事务复用。
pub fn create_write_restricted_token(workspace_sid: &str) -> Result<WriteRestrictedToken, String> {
    mint_with_group_strings(workspace_sid, RESTRICTING_SID_WHITELIST)
}

/// 组面显式给定形态（测试对照实验专用）：`groups` 为追加进 restricting 列表
/// 的组 SID 字符串（logon sid 与 ws_sid 恒在）。生产 `create_write_
/// restricted_token` 走白名单；最小集实验传 `&[]` 验证「无组面」下子进程
/// 初始化 / fence 语义（2026-09-28 围栏失守排查）。
#[cfg(test)]
pub(crate) fn create_write_restricted_token_with_groups(
    workspace_sid: &str,
    groups: &[&str],
) -> Result<WriteRestrictedToken, String> {
    mint_with_group_strings(workspace_sid, groups)
}

/// mint 共通路径：OpenProcessToken + SID 解析（ws + 组面）+ mint_restricted
/// + LocalFree 清理。任一步失败即 Err，不降级。
fn mint_with_group_strings(
    workspace_sid: &str,
    group_strs: &[&str],
) -> Result<WriteRestrictedToken, String> {
    let ws_w = wide(workspace_sid);
    unsafe {
        let mut base: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut base,
        ) == 0
        {
            return Err(last_err("OpenProcessToken"));
        }
        let mut restricted: HANDLE = std::ptr::null_mut();
        let r = (|| {
            let mut ws_sid: PSID = std::ptr::null_mut();
            if ConvertStringSidToSidW(ws_w.as_ptr(), &mut ws_sid) == 0 {
                return Err(last_err("ConvertStringSidToSidW(workspace_sid)"));
            }
            // 组面 SID 一次解析（EqualSid 语义经字符串比较——一次性铸造
            // 不在乎这点开销，还免了 SID 二进制比较的长度分支）。
            let mut group_sids: Vec<PSID> = Vec::with_capacity(group_strs.len());
            let mut parse_err: Option<String> = None;
            for s in group_strs {
                let w = wide(s);
                let mut p: PSID = std::ptr::null_mut();
                if ConvertStringSidToSidW(w.as_ptr(), &mut p) == 0 {
                    parse_err = Some(last_err(&format!("ConvertStringSidToSidW({s})")));
                    break;
                }
                group_sids.push(p);
            }
            let r = match parse_err {
                Some(e) => Err(e),
                None => mint_restricted(base, &group_sids, ws_sid, &mut restricted),
            };
            for p in group_sids {
                LocalFree(p);
            }
            LocalFree(ws_sid);
            r
        })();
        CloseHandle(base);
        match r {
            Ok(()) => Ok(WriteRestrictedToken { handle: restricted }),
            Err(e) => {
                if !restricted.is_null() {
                    CloseHandle(restricted);
                }
                Err(e)
            }
        }
    }
}

/// PSID → 字符串 SID（测试断言/诊断 dump 专用：introspection、fence-dump、
/// 用户 SID 排除断言。生产链不消费——组面白名单经字符串直接解析进
/// restricting 列表，不做 PSID 反查）。
#[cfg(test)]
pub(crate) fn sid_to_string(sid: PSID) -> Option<String> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    unsafe {
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut p) == 0 {
            return None;
        }
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        LocalFree(p.cast());
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// spawn 事务
// ---------------------------------------------------------------------------

/// spawn 事务结果（响应行可选——非 executor 形态的调用方可以只取退出码）。
#[derive(Debug)]
pub struct TxnOutcome {
    /// stdout 首行（EOF 前收到才有）。executor 协议里是 `ExecutorResponse`
    /// JSON 行；测试场景多为 None（libtest 子进程不写协议行）。
    pub response: Option<String>,
    /// 进程退出码（收尸成功才 Some）。
    pub exit_code: Option<u32>,
    /// stderr 尾部（诊断用，前 4KB 内）。
    pub stderr_tail: String,
}

/// 同步 spawn 事务（见模块文档）。调用方负责放进 `spawn_blocking`（事务内
/// 有阻塞等待）。`timeout` 的语义：`WaitForSingleObject` 超时窗内子进程没退
/// 出 → `TerminateProcess` 强杀 + Err——精确到本进程，不误伤同 workspace 的
/// 其他调用。
///
/// `creation_flags`：追加的进程创建标志（`CREATE_UNICODE_ENVIRONMENT` 由本
/// 函数自行叠加）。受限令牌下**新** console 分配必死 0xC0000142（模块文档
/// 「console 分配面」），executor 场景传 `DETACHED_PROCESS`（stdio 全管道
/// 无需 console）；libtest 哨兵形态同用 DETACHED。
///
/// `args`：executor 子进程无参数（全 env 驱动）；测试传 libtest 过滤器。
/// `env_extra`：在继承环境之上覆盖/追加（gateway 闭包组装
/// `NEMESISBOT_ROLE`/`NEMESISBOT_EXECUTOR_WORKSPACE`/
/// `NEMESISBOT_SANDBOX_BACKEND=workspace-dacl` 等）。工作目录继承父进程
/// （与 stdio 通道一致）。
pub fn stdio_txn_raw(
    token: &WriteRestrictedToken,
    exe: &Path,
    args: &[String],
    env_extra: &[(String, String)],
    request_line: &str,
    creation_flags: u32,
    timeout: std::time::Duration,
) -> Result<TxnOutcome, String> {
    // 三对管道：stdin(读端←子 / 写端←父)、stdout、stderr（SA 可继承——
    // 子侧端要随 bInheritHandles 过去）。
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (stdin_r, stdin_w) = create_inherit_pipe(&sa)?;
    let (stdout_r, stdout_w) = create_inherit_pipe(&sa)?;
    let (stderr_r, stderr_w) = create_inherit_pipe(&sa)?;
    // 父侧端清继承（防子进程的子孙拿到父端句柄——经典坑，漏了会让孙进程
    // 拖住 EOF、读线程收不到收尾）。
    for h in [stdin_w, stdout_r, stderr_r] {
        if unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) } == 0 {
            let e = last_err("SetHandleInformation");
            for h in [stdin_r, stdin_w, stdout_r, stdout_w, stderr_r, stderr_w] {
                unsafe { CloseHandle(h) };
            }
            return Err(e);
        }
    }

    // 环境块：继承环境 + 覆盖/追加 extra，UTF-16 双 NUL 结尾。
    let env_block = build_unicode_env_block(env_extra)?;

    // 命令行：quoted exe + args（PWSTR 缓冲，CreateProcessAsUserW 可能写回）。
    let mut cmdline = format!("\"{}\"", exe.display());
    for a in args {
        cmdline.push(' ');
        cmdline.push('"');
        cmdline.push_str(a);
        cmdline.push('"');
    }
    let mut cmdline_w: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    let exe_w = wide(&exe.to_string_lossy());

    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = stdin_r;
    si.hStdOutput = stdout_w;
    si.hStdError = stderr_w;

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessAsUserW(
            token.handle,
            exe_w.as_ptr(),
            cmdline_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // bInheritHandles：stdio 三通就靠它
            creation_flags | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *const c_void,
            std::ptr::null(), // cwd 继承父进程（与 stdio 通道一致）
            &si,
            &mut pi,
        )
    };
    // 子侧句柄已随继承交给子进程（成败都要关父侧的子端副本）。
    unsafe {
        CloseHandle(stdin_r);
        CloseHandle(stdout_w);
        CloseHandle(stderr_w);
    }
    if ok == 0 {
        let e = last_err("CreateProcessAsUserW");
        unsafe {
            CloseHandle(stdin_w);
            CloseHandle(stdout_r);
            CloseHandle(stderr_r);
        }
        return Err(format!(
            "CreateProcessAsUserW 失败: {e}——受限令牌 spawn 被拒，请走配置的降级链（warn 回退纯完整性档 / acl.strict=true 时拒绝执行）"
        ));
    }
    unsafe {
        CloseHandle(pi.hThread); // 只留 hProcess 收尸/杀用
    }

    // ---- 事务主体 ----
    // 1) 写请求行 + flush；File 绑定保活到写完再 drop（关 stdin = EOF 信号，
    //    语义同 spawn_and_call_stdio；句柄所有权唯一，绝不二次包装同一句柄）。
    let (write_res, stdin_writer) = {
        use std::io::Write;
        let mut f = to_file(stdin_w);
        let r = f
            .write_all(request_line.as_bytes())
            .and_then(|()| f.flush());
        (r, f)
    };
    drop(stdin_writer);
    if let Err(e) = write_res {
        // 读线程尚未启动：父侧 stdout/stderr 读端还是裸句柄，错误路径必须
        // 收掉（漏了 = 每次「写请求失败」泄 2 个句柄；子进程秒死断管是该
        // 路径的现实触发形态，如初始化 0xC0000142）。
        unsafe {
            CloseHandle(stdout_r);
            CloseHandle(stderr_r);
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hProcess);
        }
        return Err(format!("写 executor 请求失败: {e}"));
    }

    // 2) stdout/stderr 读线程（同步读；子进程退出 → 管道 EOF → 自然收束）。
    //    stdout 首行经 channel 回主线程（recv 有界等待，不等孙进程拖死）。
    //    句柄先在主线程包成 owned File（HANDLE 裸指针不 Send；File Send）再
    //    move 进线程。
    let (line_tx, line_rx) = std::sync::mpsc::channel();
    let stdout_file = to_file(stdout_r);
    let stderr_file = to_file(stderr_r);
    let stdout_reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(stdout_file);
        let mut line = String::new();
        let mut got: Option<String> = None;
        // 必须排水到 EOF（只留首行）：提前收手会关闭读端，子进程后续输出
        // 撞破管道（ERROR_NO_DATA 232）以 101 退出——真实 exit code 被污染。
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if got.is_none() {
                        got = Some(line.trim_end_matches(['\n', '\r']).to_string());
                    }
                }
            }
        }
        let _ = line_tx.send(got);
    });
    // stderr 排水线程：内容写共享缓冲 + 完成信号。take(4096) 限总量，但
    // read_to_end 的 EOF 可能被继承写端的孙进程拖住——无界 `join` 会在进程
    // 已退、超时窗已过之后仍挂死事务，故主线程只做有界等待后取已积累内容
    // （stderr_tail 是诊断面，截断可接受）；超时臂线程 detach，持 File 的
    // 它 EOF/上限时自收句柄。
    let stderr_buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
    let (stderr_done_tx, stderr_done_rx) = std::sync::mpsc::channel::<()>();
    let stderr_reader = {
        let stderr_buf = stderr_buf.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut s = Vec::new();
            let _ = stderr_file.take(4096).read_to_end(&mut s);
            if let Ok(mut g) = stderr_buf.lock() {
                *g = s;
            }
            let _ = stderr_done_tx.send(());
        })
    };

    // 3) 主线程：等进程退出（带超时窗）→ 收响应行。
    let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    let wait = unsafe { WaitForSingleObject(pi.hProcess, timeout_ms) };
    if wait == WAIT_TIMEOUT {
        unsafe {
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hProcess);
        }
        // 读线程随管道 EOF 收束，detach 即可（不阻塞错误路径）。
        return Err(format!(
            "executor timed out after {timeout:?}（受限令牌子进程已被 TerminateProcess）"
        ));
    }
    if wait != WAIT_OBJECT_0 {
        unsafe { CloseHandle(pi.hProcess) };
        return Err(format!("WaitForSingleObject 异常返回 0x{wait:X}"));
    }

    // 子进程已退 → 它持有的管道写端全部关闭 → 读线程即达 EOF。有界等待：
    // 极端迟到（比如孙进程继承了写端还活着）也不挂死事务。
    let response = line_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or(None);
    let _ = stdout_reader; // 已达 EOF 或 detach；不 join（同上理由）
    // stderr 有界收尾（见上方排水线程注释）：完成信号最多等 5s，超时取
    // 已积累部分（Mutex 仍在被写就取到哪算哪）。
    let _ = stderr_done_rx.recv_timeout(std::time::Duration::from_secs(5));
    let stderr_tail = stderr_buf
        .lock()
        .map(|g| String::from_utf8_lossy(&g).into_owned())
        .unwrap_or_default();
    let _ = stderr_reader; // 完成臂已结束；超时臂 detach（自持句柄自收）

    // 4) 收尸：退出码。
    let mut code: u32 = 0;
    let have_code = unsafe { GetExitCodeProcess(pi.hProcess, &mut code) } != 0;
    unsafe { CloseHandle(pi.hProcess) };
    Ok(TxnOutcome {
        response,
        exit_code: have_code.then_some(code),
        stderr_tail,
    })
}

// ---------------------------------------------------------------------------
// FFI 辅助
// ---------------------------------------------------------------------------

/// UTF-16 + NUL（宽字符入参）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// BOOL 风格失败（错误码在 GetLastError）。
fn last_err(op: &str) -> String {
    let code = unsafe { GetLastError() };
    format!("{op}: {}", std::io::Error::from_raw_os_error(code as i32))
}

/// 一对可继承匿名管道。
fn create_inherit_pipe(sa: &SECURITY_ATTRIBUTES) -> Result<(HANDLE, HANDLE), String> {
    let (mut r, mut w) = (std::ptr::null_mut(), std::ptr::null_mut());
    if unsafe { CreatePipe(&mut r, &mut w, sa, 0) } == 0 {
        return Err(last_err("CreatePipe"));
    }
    Ok((r, w))
}

/// raw 句柄包成 owned `std::fs::File`（Drop 自动 CloseHandle；读写同用）。
fn to_file(h: HANDLE) -> std::fs::File {
    use std::os::windows::io::FromRawHandle;
    // SAFETY：h 是 CreatePipe 返回的有效裸句柄，所有权在此移交 File——每个
    // 句柄值只移交一次（写路径关 stdin 是对同一 File 绑定的 drop，不二次
    // 包装同一句柄）。
    unsafe { std::fs::File::from_raw_handle(h as _) }
}

/// 继承环境 + 覆盖/追加 extra → UTF-16 环境块（NAME=VALUE\0...\0\0）。
fn build_unicode_env_block(extra: &[(String, String)]) -> Result<Vec<u16>, String> {
    let mut vars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    unsafe {
        let block = GetEnvironmentStringsW();
        if block.is_null() {
            return Err(last_err("GetEnvironmentStringsW"));
        }
        let mut p = block;
        loop {
            // 逐条扫描到双 NUL。
            let mut len = 0usize;
            while *p.add(len) != 0 {
                len += 1;
            }
            if len == 0 {
                break; // 结尾双 NUL
            }
            let entry: Vec<u16> = std::slice::from_raw_parts(p, len).to_vec();
            let s = String::from_utf16_lossy(&entry);
            if let Some((k, v)) = s.split_once('=')
                && !k.is_empty()
            {
                vars.insert(k.to_string(), v.to_string());
            }
            p = p.add(len + 1);
        }
        FreeEnvironmentStringsW(block);
    }
    for (k, v) in extra {
        vars.insert(k.clone(), v.clone());
    }
    // CreateProcess 契约：环境块按名字母序（大小写不敏感）排序（MSDN
    // lpEnvironment 注记）——HashMap 迭代序随机，必须显式排序。
    let mut vars: Vec<(String, String)> = vars.into_iter().collect();
    vars.sort_by_key(|a| a.0.to_uppercase());
    let mut block: Vec<u16> = Vec::with_capacity(4096);
    for (k, v) in &vars {
        let kv = format!("{k}={v}");
        block.extend(kv.encode_utf16());
        block.push(0);
    }
    block.push(0); // 双 NUL 结尾
    Ok(block)
}

// ---------------------------------------------------------------------------
// availability 探针（状态面 / 选型用——D4 接线）
// ---------------------------------------------------------------------------

/// DACL 定向档本机可用性（令牌可开 + 两个 restricting SID 可解析）。
/// 全部是 Vista+ 恒真 API——失败 = 机器态异常，诚实报 Unavailable。
pub fn dacl_availability() -> Availability {
    if let Err(e) = create_write_restricted_token("S-1-5-21-1-1-1") {
        return Availability::Unavailable(format!("受限令牌铸造失败: {e}"));
    }
    Availability::Full
}
