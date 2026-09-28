//! P24（2026-09-25 能力扩展 WS1）：Windows 用户态 ACL 轻量沙盒档（实验性）。
//!
//! ## 围栏机制（半档隔离的诚实边界）
//!
//! 零安装、零 UAC。核心是**强制完整性标签（Mandatory Integrity Label）的
//! No-Write-Up 策略**，与 landlock 一样是「进程侧」围栏：
//!
//! 1. 工作区（writable_roots）打 **Low 完整性标签**（带 OICI 继承——此后在
//!    工作区内新建的文件/子目录继承 Low）；
//! 2. executor 进程把**自身令牌降到 Low IL**（降级无特权要求，升回不行——
//!    对 per-call 即退的 executor 子进程不可逆无妨）；
//! 3. No-Write-Up 生效：Low 令牌写不了 Medium+ 对象 → 工作区外几乎全部
//!    默认 Medium 的文件系统（含 `%TEMP%`、用户目录、系统路径）写操作被
//!    内核拒绝。**agent 进程树对工作区外的敏感路径写操作被 ACL 拒绝**。
//!
//! ## 三件套落地程度（任务要求的分层交付）
//!
//! - **完整性标签（必做，已做）**：`set_integrity_label` / `get_integrity_label`
//!   / `remove_integrity_label`（对象级 API，目录与文件同源）+
//!   `lower_current_process_integrity`（令牌侧）+ `label_tree`（存量文件
//!   一次性递归重标——新建对象靠 OICI 继承，存量 Medium 文件必须显式重标，
//!   Low 令牌才写得了）。
//! - **DACL 定向拒绝（已接线，独立档，D1-D3 2026-09-27）**：定向档已由
//!   workspace-dacl 路线落地——`derive_workspace_sid`（workspace SID 确定性
//!   派生，sid.rs）+ `ensure_grant_ace_tree`（幂等 standing GRANT ACE 树，
//!   本文件）+ 受限令牌（`create_write_restricted_token`：白名单
//!   restricting SID ∪ logon SID ∪ ws_sid + `WRITE_RESTRICTED`，token.rs）
//!   经 `executor.acl.dacl`（opt-in，默认 false）在 executor 通道装配点接线
//!   ——spawn 走受限令牌，工作区外写被内核双合取检查拒绝。**完整性标签档
//!   自身仍不消费 deny ACE**：`add_deny_write_ace` / `revoke_ace` 原语独立
//!   存在（对 Everyone 的 deny 会连用户自己一起拦，定向语义归受限令牌档）。
//! - **capability SID（可选，未做）**：capability SID（S-1-15-3-…）只在
//!   AppContainer/LPAC 语境有意义；AppContainer 是完整得多的隔离形态
//!   （但破坏面也大得多：任意构建工具在 AppContainer 里大量失能），不在
//!   本档范围。诚实记录为未做。
//!
//! ## `apply_to_self` 装配顺序（失败可降级、不假装隔离成功）
//!
//! 1. 快照 writable_roots 当前标签；
//! 2. 逐根打 Low 标签 + [`label_tree`] 递归重标**存量文件**（预算
//!    [`LABEL_TREE_BUDGET`]；超预算/IO 失败 → 已标部分保持生效，缺口如实
//!    入 gaps 继续装配——围栏本体不因此缺席）——根标签设置失败 →
//!    **Err 且不动令牌**（先降令牌后失败会让 executor 连工作区都写不了，
//!    坏过没盒）；
//! 3. 工作区内建低完整性临时目录并重定向 `TMP`/`TEMP`（编译类工具的临时
//!    文件落点）——失败 → **gap 继续**（围栏照装，代价如实入列）；
//! 4. 自身令牌降 Low——失败 → **回滚步骤 2 的标签快照**（尽力而为，仅根级
//!    ——label_tree 的逐文件重标无差量记录不回滚，回滚失败明细并入 Err
//!    文本）→ Err；
//! 5. 返回 [`Enforcement::Partial`]（gaps 必非空：ACL 档禁不了网）。
//!
//! ## 诚实边界（gaps 之外的已知代价）
//!
//! - **不禁网**：No-Write-Up 只管写，不管 socket——禁网需求走 Sandboxie 档
//!   （`AllowNetworkAccess=n`）或 bwrap `--unshare-net`。
//! - **未启用 No-Read-Up**：Low 令牌仍可**读**全盘（读围栏不做——工具链
//!   需要读编译器/依赖，全盘禁读会让 executor 不可用）。
//! - **存量 Medium 文件**：装配点已接线 [`label_tree`] 一次性递归重标
//!   （预算 [`LABEL_TREE_BUDGET`]，超限诚实报 Err → 转为 gaps 明细继续）。
//!   未覆盖到的存量文件（超预算/IO 错误）仍是 Medium IL，Low 令牌写不了。
//! - **COM/RPC 断链**：Low IL 进程与 Medium COM 服务器的交互受限，个别
//!   工具可能失能——实验性档位的已知代价。
//! - **整树打标的成本**：[`label_tree`] 有 `max_files` 预算，超限诚实报
//!   Err（已标部分保持生效），不静默截断。

use std::ffi::c_void;
use std::path::Path;

// windows-sys 0.59 模块路径备忘（与 0.5x 早期差异大，改这里先对照 registry
// 源码）：ACL 系列 API 在 `Security::Authorization`；令牌 API 分裂在
// `Security`（Get/SetTokenInformation）与 `System::Threading`
// （OpenProcessToken/GetCurrentProcess）；LocalFree/CloseHandle 在
// `Foundation`（不在 System::Memory）。
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, DENY_ACCESS,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
    TRUSTEE_W,
};
// FILE_ACCESS_RIGHTS 系列常量在 Storage::FileSystem（0.59 模块划分）。
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACCESS_DENIED_ACE, ACE_HEADER, ACL, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetSecurityDescriptorSacl, GetSidSubAuthority,
    GetSidSubAuthorityCount, GetTokenInformation, INHERIT_ONLY_ACE, LABEL_SECURITY_INFORMATION,
    NO_PROPAGATE_INHERIT_ACE, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID, SID_AND_ATTRIBUTES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, SYSTEM_MANDATORY_LABEL_ACE, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
};
// FILE_DELETE_CHILD 仅状态面 deny 观测消费；DELETE 无生产消费者——基线 mask
// 保含 DELETE 且不做「掐 DELETE」分拣（2026-09-28 证伪，见 GRANT_MASK 文档）。
use windows_sys::Win32::Storage::FileSystem::{FILE_DELETE_CHILD, WRITE_DAC, WRITE_OWNER};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// winnt.h 的 `SYSTEM_MANDATORY_LABEL_ACE_TYPE`（0x11）——windows-sys 把它放
/// 在 System::SystemServices（额外 feature），本地常量免拖依赖面。
const ML_ACE_TYPE: u8 = 0x11;

use super::{Availability, BackendForm, Enforcement, SandboxBackend, SandboxConf};

// ---------------------------------------------------------------------------
// 完整性级别
// ---------------------------------------------------------------------------

/// 支持的强制完整性级别（围栏只关心 Low / Medium 两档；S-1-16-<rid>）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityLevel {
    /// S-1-16-4096——工作区打这档（Low 令牌可写）。
    Low,
    /// S-1-16-8192——进程令牌默认档（查询/还原用）。
    Medium,
}

impl IntegrityLevel {
    /// SID 的最后一节 subauthority（RID）。
    pub fn rid(self) -> u32 {
        match self {
            IntegrityLevel::Low => 4096,
            IntegrityLevel::Medium => 8192,
        }
    }

    /// 完整 SID 字符串（ConvertStringSidToSidW 消费）。
    pub fn sid_string(self) -> &'static str {
        match self {
            IntegrityLevel::Low => "S-1-16-4096",
            IntegrityLevel::Medium => "S-1-16-8192",
        }
    }

    /// RID → 级别（未知 RID = None——查询侧如实还原用）。
    pub fn from_rid(rid: u32) -> Option<IntegrityLevel> {
        match rid {
            4096 => Some(IntegrityLevel::Low),
            8192 => Some(IntegrityLevel::Medium),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// FFI 基础件
// ---------------------------------------------------------------------------

/// UTF-16 + NUL（Win32 宽字符路径/SDDL/SID 字符串入参）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// DWORD 风格错误码 → 人类可读文本（借 io::Error 的系统消息表）。
fn win32_err(op: &str, code: u32) -> String {
    format!("{op}: {}", std::io::Error::from_raw_os_error(code as i32))
}

/// BOOL 风格失败（错误码在 GetLastError）。
fn last_err(op: &str) -> String {
    win32_err(op, unsafe { GetLastError() })
}

/// 读一个**已打开**令牌的完整性级别（RID）。供
/// [`current_process_integrity`] 与 [`lower_current_process_integrity`]
/// 共用（单一真相源，避免两份 FFI 探针漂移）。
fn token_integrity_level(token: HANDLE) -> Result<u32, String> {
    unsafe {
        // 先探长度（返回 0 + ERROR_INSUFFICIENT_BUFFER 是预期路径）。
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            return Err(last_err("GetTokenInformation(size)"));
        }
        // TOKEN_MANDATORY_LABEL 首字段是指针——用 u64 数组保证对齐。
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        ) == 0
        {
            return Err(last_err("GetTokenInformation"));
        }
        let label = buf.as_ptr() as *const TOKEN_MANDATORY_LABEL;
        let sid = (*label).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid) as u32;
        if count == 0 {
            return Err("令牌完整性 SID 无 subauthority（内部错误）".to_string());
        }
        Ok(*GetSidSubAuthority(sid, count - 1))
    }
}

/// 当前进程令牌的完整性级别（RID）。探测与围栏断言共用。
pub fn current_process_integrity() -> Result<u32, String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last_err("OpenProcessToken"));
        }
        let r = token_integrity_level(token);
        CloseHandle(token);
        r
    }
}

/// 把当前进程令牌降到 `level`（只降不升：目标高于现状 → 诚实拒绝——静默
/// 放行 = 假装隔离）。降级无特权要求（升回才要 SeRelabelPrivilege），但
/// **不可逆**：只该在 executor 子进程这类即弃进程里调，绝不进 gateway/
/// 测试进程（同进程升不回去）。
pub fn lower_current_process_integrity(level: IntegrityLevel) -> Result<(), String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_DEFAULT | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return Err(last_err("OpenProcessToken"));
        }
        let r = (|| {
            if let Ok(cur) = token_integrity_level(token)
                && cur < level.rid()
            {
                return Err(format!(
                    "拒绝升标签：当前 IL {cur} 已低于目标 {}（{cur} < {}）",
                    level.rid(),
                    level.rid()
                ));
            }
            let sid_w = wide(level.sid_string());
            let mut sid: PSID = std::ptr::null_mut();
            if ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid) == 0 {
                return Err(last_err("ConvertStringSidToSidW"));
            }
            // SetTokenInformation 的缓冲 = TOKEN_MANDATORY_LABEL + 1 个 RID。
            // repr(C) 包装结构保证指针对齐，比裸字节缓冲干净。
            #[repr(C)]
            struct LabelBuf {
                label: TOKEN_MANDATORY_LABEL,
                rid: u32,
            }
            let mut buf = LabelBuf {
                label: TOKEN_MANDATORY_LABEL {
                    Label: SID_AND_ATTRIBUTES {
                        Sid: sid,
                        Attributes: 0,
                    },
                },
                rid: level.rid(),
            };
            let ok = SetTokenInformation(
                token,
                TokenIntegrityLevel,
                &mut buf as *mut LabelBuf as *mut c_void,
                std::mem::size_of::<LabelBuf>() as u32,
            );
            LocalFree(sid as _);
            if ok == 0 {
                Err(last_err("SetTokenInformation(TokenIntegrityLevel)"))
            } else {
                Ok(())
            }
        })();
        CloseHandle(token);
        r
    }
}

// ---------------------------------------------------------------------------
// 完整性标签（对象侧）：set / get / remove / label_tree
// ---------------------------------------------------------------------------

/// 给文件/目录打强制完整性标签（No-Write-Up + OICI 继承）。
///
/// OICI = 新建子对象继承标签——工作区打标后，之后由任意进程创建的文件
/// 都是 Low（这是「父进程先标目录、子进程再写文件」时序能工作的关键）。
/// 存量子对象不受影响（ACE 继承只对新对象生效），需要 [`label_tree`]。
///
/// 向下重标（如 Medium→Low）无需特权；**向上**重标超过自身令牌 IL 需要
/// SeRelabelPrivilege——本 API 定位是向下打标，向上打不进去会诚实报错。
pub fn set_integrity_label(path: &Path, level: IntegrityLevel) -> Result<(), String> {
    // SDDL：ML ACE = 强制标签；OICI = 对象/容器继承；NW = No-Write-Up 策略；
    // LW/ME = Low/Medium 完整性 SID 的 SDDL 别名。
    let sddl = match level {
        IntegrityLevel::Low => "S:(ML;OICI;NW;;;LW)",
        IntegrityLevel::Medium => "S:(ML;OICI;NW;;;ME)",
    };
    let sddl_w = wide(sddl);
    let path_w = wide(&path.as_os_str().to_string_lossy());
    unsafe {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut psd,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(last_err(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            ));
        }
        // 标签住在 SACL——从转换出的 SD 里摘出 SACL 再整体交给
        // SetNamedSecurityInfoW（它同步拷贝，调用返回后即可释放 SD）。
        let mut present: i32 = 0;
        let mut sacl: *mut ACL = std::ptr::null_mut();
        let mut defaulted: i32 = 0;
        let r = if GetSecurityDescriptorSacl(psd, &mut present, &mut sacl, &mut defaulted) == 0 {
            Err(last_err("GetSecurityDescriptorSacl"))
        } else if present == 0 || sacl.is_null() {
            Err("SDDL 转换结果缺少 SACL（内部错误）".to_string())
        } else {
            let hr = SetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                sacl,
            );
            if hr == 0 {
                Ok(())
            } else {
                Err(win32_err("SetNamedSecurityInfoW(LABEL)", hr))
            }
        };
        LocalFree(psd as _);
        r
    }
}

/// 读对象当前的强制完整性级别（RID）。`Ok(None)` = 无显式标签（默认按
/// Medium 处理）。只在 SACL 里找第一条 ML ACE（微软语义：对象至多一条）。
pub fn get_integrity_label(path: &Path) -> Result<Option<u32>, String> {
    let path_w = wide(&path.as_os_str().to_string_lossy());
    unsafe {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let mut sacl: *mut ACL = std::ptr::null_mut();
        let hr = GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sacl,
            &mut psd,
        );
        if hr != 0 {
            return Err(win32_err("GetNamedSecurityInfoW(LABEL)", hr));
        }
        let mut level: Option<u32> = None;
        if !sacl.is_null() {
            for i in 0..(*sacl).AceCount {
                let mut pace: *mut c_void = std::ptr::null_mut();
                if GetAce(sacl, i as u32, &mut pace) == 0 || pace.is_null() {
                    continue;
                }
                let ace = pace as *const SYSTEM_MANDATORY_LABEL_ACE;
                if (*ace).Header.AceType == ML_ACE_TYPE {
                    // SID 内联在 ACE 尾部（SidStart 只标起点）；取最后一个
                    // subauthority = RID。
                    let sid = std::ptr::addr_of!((*ace).SidStart) as PSID;
                    let count = *GetSidSubAuthorityCount(sid) as u32;
                    if count > 0 {
                        level = Some(*GetSidSubAuthority(sid, count - 1));
                    }
                    break;
                }
            }
        }
        LocalFree(psd as _);
        Ok(level)
    }
}

/// 移除显式完整性标签（pSacl = NULL + LABEL_SECURITY_INFORMATION → 回落
/// 默认 Medium）。回滚路径用。
pub fn remove_integrity_label(path: &Path) -> Result<(), String> {
    let path_w = wide(&path.as_os_str().to_string_lossy());
    let hr = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if hr == 0 {
        Ok(())
    } else {
        Err(win32_err("SetNamedSecurityInfoW(LABEL remove)", hr))
    }
}

/// 递归重标整棵树（存量文件的补救路径）。返回成功重标的对象数（含根）。
/// `max_files` 预算防大工作区卡死装配——超限 **Err**（已标部分保持生效，
/// 明细诚实入错），不静默截断。符号链接不跟随（symlink_metadata 判定），
/// 防出树重标。
pub fn label_tree(root: &Path, level: IntegrityLevel, max_files: usize) -> Result<usize, String> {
    set_integrity_label(root, level)?;
    let mut count: usize = 1;
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("label_tree: 读目录 {} 失败: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("label_tree: 目录项读取失败: {e}"))?;
            let p = entry.path();
            let meta = std::fs::symlink_metadata(&p)
                .map_err(|e| format!("label_tree: 元数据 {} 失败: {e}", p.display()))?;
            if meta.is_dir() {
                stack.push(p.clone());
            }
            set_integrity_label(&p, level)?;
            count += 1;
            if count > max_files {
                return Err(format!(
                    "label_tree: 预算耗尽（已重标 {count} > 上限 {max_files}）——已标部分保持生效，\
                     请调大 max_files 或收窄工作区后重跑"
                ));
            }
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// DACL 原语：deny-write / revoke
// ---------------------------------------------------------------------------

/// 追加一条「拒绝写」DACL ACE（继承到子对象/子目录）。`sid` 是任意可转换
/// 的 SID 字符串（如 Everyone = `S-1-1-0`）。
///
/// ⚠ 定位是**原语**：deny ACE 对所有含该 SID 的令牌生效——包括用户自己
/// 的其他进程。定向「只拒 agent」需要 agent 独占 SID（用户态铸造不了，
/// 见模块文档三件套落地程度），调用方自行权衡目标 SID 的波及面。
pub fn add_deny_write_ace(path: &Path, sid: &str) -> Result<(), String> {
    change_dacl(path, sid, DENY_ACCESS)
}

/// 撤销该 SID 在 DACL 上的全部 ACE（deny 与 allow 一起撤——测试/回滚用）。
///
/// **不走 `SetEntriesInAclW(REVOKE_ACCESS)`**：实测（2026-09-26，Win11
/// 26200）REVOKE 条目对 DACL 现存的 deny/allow ACE 全部不命中——new DACL
/// 与旧 DACL 逐条相同、一条不删（SetEntriesInAclW 照常返回成功）。改为
/// **确定性重建**：读旧 DACL → 逐 ACE 转 EXPLICIT_ACCESS、用 [`EqualSid`]
/// 跳过目标 SID 的项 → `SetEntriesInAclW` 以空底表重建 → 写回。
///
/// 保真细节：允许 ACE 的 mask（具体权）与继承位（OI/CI/NP/IO）原样保留；
/// INHERITED_ACE 位丢弃——`SetNamedSecurityInfoW` 写非保护 DACL 时系统会
/// 自动从父目录重继承。⚠ 滤完为空 = 空 DACL（除属主隐含权外无人可入，
/// 含调用者自己）——原语语义本就如此，调用方自行权衡。
pub fn revoke_ace(path: &Path, sid: &str) -> Result<(), String> {
    let path_w = wide(&path.as_os_str().to_string_lossy());
    let sid_w = wide(sid);
    unsafe {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let hr = GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old_dacl,
            std::ptr::null_mut(),
            &mut psd,
        );
        if hr != 0 {
            return Err(win32_err("GetNamedSecurityInfoW(DACL)", hr));
        }
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let mut sid_ptr: PSID = std::ptr::null_mut();
        let r = (|| {
            if ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid_ptr) == 0 {
                return Err(last_err("ConvertStringSidToSidW"));
            }
            const INHERIT_MASK: u32 = OBJECT_INHERIT_ACE
                | CONTAINER_INHERIT_ACE
                | NO_PROPAGATE_INHERIT_ACE
                | INHERIT_ONLY_ACE;
            let mut eas: Vec<EXPLICIT_ACCESS_W> = Vec::new();
            if !old_dacl.is_null() {
                let count = (*old_dacl).AceCount;
                for i in 0..count {
                    let mut pace: *mut c_void = std::ptr::null_mut();
                    if GetAce(old_dacl, i as u32, &mut pace) == 0 || pace.is_null() {
                        return Err(last_err("GetAce"));
                    }
                    let hdr = pace as *const ACE_HEADER;
                    let (mode, mask, ace_sid) = match (*hdr).AceType {
                        0 => {
                            let a = pace as *const ACCESS_ALLOWED_ACE;
                            (
                                GRANT_ACCESS,
                                (*a).Mask,
                                &(*a).SidStart as *const u32 as PSID,
                            )
                        }
                        1 => {
                            let a = pace as *const ACCESS_DENIED_ACE;
                            (DENY_ACCESS, (*a).Mask, &(*a).SidStart as *const u32 as PSID)
                        }
                        t => {
                            // 不认识的 ACE 类型（如 system-audit 误入 DACL）：
                            // 拒绝静默丢弃，诚实失败（丢了可能导致权限漂移）。
                            return Err(format!(
                                "revoke_ace: 第 {i} 条 ACE 类型 {t} 未支持，拒绝静默丢弃"
                            ));
                        }
                    };
                    if EqualSid(ace_sid, sid_ptr) != 0 {
                        continue; // 目标 SID 的 ACE（deny/allow 皆）→ 滤掉
                    }
                    eas.push(EXPLICIT_ACCESS_W {
                        grfAccessPermissions: mask,
                        grfAccessMode: mode,
                        grfInheritance: (*hdr).AceFlags as u32 & INHERIT_MASK,
                        Trustee: TRUSTEE_W {
                            pMultipleTrustee: std::ptr::null_mut(),
                            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                            TrusteeForm: TRUSTEE_IS_SID,
                            TrusteeType: TRUSTEE_IS_UNKNOWN,
                            ptstrName: ace_sid as *mut u16,
                        },
                    });
                }
            }
            // 空底表重建：SetEntriesInAclW 只按传入的 eas 生成新 ACL（不合并
            // old_dacl——后者是待过滤的源头，上面已逐条拣选）。
            let hr2 = SetEntriesInAclW(
                eas.len() as u32,
                eas.as_ptr(),
                std::ptr::null(),
                &mut new_dacl,
            );
            if hr2 != 0 {
                return Err(win32_err("SetEntriesInAclW", hr2));
            }
            let hr3 = SetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null(),
            );
            if hr3 != 0 {
                return Err(win32_err("SetNamedSecurityInfoW(DACL)", hr3));
            }
            Ok(())
        })();
        if !new_dacl.is_null() {
            LocalFree(new_dacl as _);
        }
        if !sid_ptr.is_null() {
            LocalFree(sid_ptr as _);
        }
        if !psd.is_null() {
            LocalFree(psd as _);
        }
        r
    }
}

/// DACL 修改单漏斗：GetNamedSecurityInfoW 读旧 DACL → SetEntriesInAclW 合并
/// EXPLICIT_ACCESS → SetNamedSecurityInfoW 写回。所有中间句柄 LocalFree，
/// 失败路径不泄漏。
fn change_dacl(path: &Path, sid: &str, mode: i32) -> Result<(), String> {
    let path_w = wide(&path.as_os_str().to_string_lossy());
    let sid_w = wide(sid);
    unsafe {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let hr = GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old_dacl,
            std::ptr::null_mut(),
            &mut psd,
        );
        if hr != 0 {
            return Err(win32_err("GetNamedSecurityInfoW(DACL)", hr));
        }
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let mut sid_ptr: PSID = std::ptr::null_mut();
        let r = (|| {
            if ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid_ptr) == 0 {
                return Err(last_err("ConvertStringSidToSidW"));
            }
            let ea = EXPLICIT_ACCESS_W {
                // GENERIC_WRITE 覆盖 FILE_ADD_FILE / 写数据 / 改属性——目录
                // 与文件两用。DELETE 等其他右侧按需再扩展。
                grfAccessPermissions: GENERIC_WRITE,
                grfAccessMode: mode,
                grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: sid_ptr as *mut u16,
                },
            };
            let hr2 = SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl);
            if hr2 != 0 {
                return Err(win32_err("SetEntriesInAclW", hr2));
            }
            let hr3 = SetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null(),
            );
            if hr3 != 0 {
                return Err(win32_err("SetNamedSecurityInfoW(DACL)", hr3));
            }
            Ok(())
        })();
        if !new_dacl.is_null() {
            LocalFree(new_dacl as _);
        }
        if !sid_ptr.is_null() {
            LocalFree(sid_ptr as _);
        }
        if !psd.is_null() {
            LocalFree(psd as _);
        }
        r
    }
}

// ---------------------------------------------------------------------------
// SandboxBackend 接线
// ---------------------------------------------------------------------------

/// 装配点 `label_tree` 一次性重标存量文件的默认预算（对象数，含目录）。
/// 超预算 = 重标未完成 → existing-files gap 如实入列（见 apply_to_self）。
pub const LABEL_TREE_BUDGET: usize = 20_000;

/// label_tree 未完成时 existing-files gap 的明细文本（纯函数，单测钉形态）。
pub(super) fn existing_files_gap(reason: &str) -> String {
    format!(
        "existing-files: 存量文件递归重标未完成（{reason}）——未重标到的存量文件仍是 \
         Medium IL，Low 令牌写不了；收窄工作区或调大预算后重跑装配可补齐"
    )
}

/// P24 Windows 用户态 ACL 轻量档（SelfApply 形态——与 landlock 同款：
/// executor 子进程启动时对自身装配，后代全继承）。
///
/// **实验性档位**：半档隔离（禁不了网、读不设防；存量文件打标已接线，
/// 见 [`LABEL_TREE_BUDGET`]）。选型入口见 [`super::select_windows_backend`]
/// 决策表；config 键 = `executor.backend`。
pub struct AclBackend;

impl AclBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AclBackend {
    fn default() -> Self {
        Self
    }
}

impl SandboxBackend for AclBackend {
    fn name(&self) -> &str {
        "acl"
    }

    fn form(&self) -> BackendForm {
        BackendForm::SelfApply
    }

    /// 本机可用性：令牌可开 + 完整性 SID 可转换（Vista+ 恒真——本档不依赖
    /// 任何第三方安装物）。失败的机器诚实报 Unavailable；可用时报 Partial
    /// （实验性 + 半档缺口进探测面，sandbox 状态页能看到）。
    fn availability(&self) -> Availability {
        if let Err(e) = current_process_integrity() {
            return Availability::Unavailable(format!("当前进程令牌不可用: {e}"));
        }
        let sid_w = wide(IntegrityLevel::Low.sid_string());
        let mut sid: PSID = std::ptr::null_mut();
        unsafe {
            if ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid) == 0 {
                return Availability::Unavailable(last_err("ConvertStringSidToSidW"));
            }
            LocalFree(sid as _);
        }
        Availability::Partial(vec![
            "experimental: ACL 档为半档隔离（禁不了网；存量文件打标已接线，超预算/失败进 gaps）"
                .to_string(),
        ])
    }

    /// 装配顺序见模块文档「apply_to_self 装配顺序」。恒 Partial（禁不了网
    /// 是本档的结构性缺口）。
    fn apply_to_self(&self, conf: &SandboxConf) -> Result<Enforcement, String> {
        // 1) 快照——失败路径要能把标签滚回去。
        let mut snapshot: Vec<(std::path::PathBuf, Option<u32>)> = Vec::new();
        for root in &conf.writable_roots {
            let prev = get_integrity_label(root)?;
            snapshot.push((root.clone(), prev));
        }
        // 2) 工作区打 Low 标签 + label_tree 递归重标存量文件（打标后新建
        //    对象靠 OICI 继承，存量 Medium 文件必须显式重标 Low 令牌才写得
        //    了）。根标签设置失败 → Err 且不动令牌（围栏本体缺席，见模块
        //    文档顺序说明）；递归重标未完成（超预算/子项 IO 错误）→ 已标
        //    部分保持生效，缺口如实入 gaps **继续装配**——围栏不因此缺席。
        let mut gaps: Vec<String> = Vec::new();
        for root in &conf.writable_roots {
            set_integrity_label(root, IntegrityLevel::Low).map_err(|e| {
                format!(
                    "工作区完整性标签设置失败（{}）：{e} ——拒绝降级令牌\
                     （先降令牌会让执行体连工作区都写不了，坏过没盒）",
                    root.display()
                )
            })?;
            // label_tree 会重设根标签（幂等）再递归——预算耗尽/子项失败
            // 返回 Err，但已标部分保持生效。
            match label_tree(root, IntegrityLevel::Low, LABEL_TREE_BUDGET) {
                Ok(count) => eprintln!(
                    "[acl-backend] 存量文件重标完成：{} 个对象（{}）",
                    count,
                    root.display()
                ),
                Err(e) => gaps.push(existing_files_gap(&e)),
            }
        }
        // 3) 低完整性临时目录 + TMP/TEMP 重定向（编译类工具的临时文件落点
        //    ——否则 Low 令牌写不了 Medium 的 %TEMP%，构建全炸）。失败 →
        //    gap 继续（围栏照装，代价如实入列）。
        if let Some(first_root) = conf.writable_roots.first() {
            let tmp = first_root.join(".sandbox-lowil-tmp");
            let prepared = std::fs::create_dir_all(&tmp)
                .map_err(|e| format!("create_dir_all: {e}"))
                .and_then(|_| set_integrity_label(&tmp, IntegrityLevel::Low))
                .map(|_| tmp.clone());
            match prepared {
                Ok(tmp) => {
                    // SAFETY（edition 2024 set_var unsafe）：装配点在 executor
                    // 子进程 main 早期、单线程、尚无并发 env 读者；改动只影响
                    // 本进程及其后代 spawn 的继承 env。
                    unsafe {
                        std::env::set_var("TMP", &tmp);
                        std::env::set_var("TEMP", &tmp);
                    }
                }
                Err(e) => gaps.push(format!(
                    "tmp: 低完整性临时目录就位失败（{e}）——%TEMP% 仍为 Medium IL，\
                     编译类工具写临时文件会被拒"
                )),
            }
        }
        // 4) 自身令牌降 Low——失败 → 尽力回滚标签快照 → Err（不假装成功）。
        if let Err(e) = lower_current_process_integrity(IntegrityLevel::Low) {
            let mut rb_fail: Vec<String> = Vec::new();
            for (root, prev) in &snapshot {
                // Some(rid) = 旧标签精确还原；None/未知 RID = 移除显式标签
                //（回落默认 Medium）。
                let r = match prev.and_then(IntegrityLevel::from_rid) {
                    Some(lv) => set_integrity_label(root, lv),
                    None => remove_integrity_label(root),
                };
                if let Err(re) = r {
                    rb_fail.push(format!("{}: {re}", root.display()));
                }
            }
            let rb_note = if rb_fail.is_empty() {
                "标签已回滚".to_string()
            } else {
                format!("标签回滚部分失败: {}", rb_fail.join("; "))
            };
            return Err(format!("令牌完整性降级失败：{e} ——{rb_note}；围栏未装上"));
        }
        // 5) 恒 Partial——结构性缺口如实入列（调用方 warn + 继续）。
        gaps.push(
            "network: ACL 档禁不了网（No-Write-Up 只管写；禁网需求走 Sandboxie 档\
             或 bwrap --unshare-net）"
                .to_string(),
        );
        gaps.push(
            "read: 未启用 No-Read-Up——Low 令牌仍可读全盘（工具链需要读编译器/依赖，\
             全盘禁读会让执行体不可用）"
                .to_string(),
        );
        gaps.push(
            "dacl: 本档（完整性标签）自身不含 per-agent 身份定向拒绝——定向能力\
             由 workspace-dacl 受限令牌档提供（executor.acl.dacl，opt-in；\
             deny ACE 原语 add_deny_write_ace 仍在，波及面由调用方权衡）"
                .to_string(),
        );
        Ok(Enforcement::Partial(gaps))
    }
}

// ---------------------------------------------------------------------------
// DACL 定向档 D2（2026-09-27）：workspace SID standing GRANT ACE 树
// ---------------------------------------------------------------------------

/// winnt.h 的 `FILE_GENERIC_ALL`（0x1F01FF）——windows-sys 0.59 未导出该组合
/// 常量（FILE_ACCESS_RIGHTS 单项在 Storage::FileSystem，组合值缺位），本地
/// 定义（同 ML_ACE_TYPE 先例：本地常量免拖依赖面）。
const FILE_GENERIC_ALL: u32 = 0x001F_01FF;

/// standing GRANT ACE 的权限位：`FILE_GENERIC_ALL & !(WRITE_DAC | WRITE_OWNER)`
/// ——保工作区**基线全功能**（含 DELETE——且真进程实证 DELETE 本就不在
/// write-restricted 的写类评估集合，见下），掐掉的只有**越狱面**：改 ACL /
/// 改属主——WRITE_DAC 在手可给树外路径翻 DACL，翻转 restricting 侧的合取
/// 评估放行树外写，这是唯一必须掐的理由。
///
/// **为什么没有「掐 DELETE」的加固档**（2026-09-28 真进程证伪， Fence
/// 证据见 ACL 定向档报告 §3.9）：write-restricted 的 restricting 侧评估
/// **只覆盖写类访问权，DELETE 不在其中**——子进程对树外对象（无任何
/// ws_sid GRANT）请求 DELETE 照样成功（token fence 实测），所以 restricting
/// 侧 mask 掐 DELETE / deny `FILE_DELETE_CHILD`（trustee=ws_sid 只影响
/// restricting 侧）对删除/改名**零拦截力**，普通令牌侧用户的 DELETE 直通；
/// 同时 rename 的 DELETE 通路也从不经 restricting 评估——v1 掐 DELETE 形态
/// 下树内 rename 实测照常成功（该形态与本文件证伪前的加固档 mask 等价）。
/// 结论：「内核强制树内不许删」在用户态 write-restricted 机制内不存在
/// 可行通路（trustee=Everyone 的 deny 会伤及用户自己的进程，违反定向承诺；
/// minifilter/Sandboxie 级别才做得到），树内删除/改名治理归 8 层策略层与
/// 审批——见 ACL 定向档报告 §5 已知边界。
pub const GRANT_MASK: u32 = FILE_GENERIC_ALL & !(WRITE_DAC | WRITE_OWNER);

/// 递归给整棵树打 workspace SID 的 standing GRANT ACE（DACL 定向档 D2）。
/// 返回处理的对象数（含根；达标跳过的也计入——语义同 [`label_tree`]）。
///
/// 每个对象的目标 DACL 形态：
/// - GRANT workspace SID，mask = [`GRANT_MASK`]，目录带 `OI|CI` 继承（新建
///   子对象自动获得；存量走本遍历逐个补打——ACE 继承只对新对象生效）；
/// - 不打 deny ACE（「掐 DELETE + 目录 DENY FILE_DELETE_CHILD」的加固形态
///   已被真进程证伪为零拦截力——见 [`GRANT_MASK`] 文档；旧形态残留的
///   deny/含-DELETE-grant 由幂等分拣当作 stale 滤除重建）。
///
/// **幂等**：读旧 DACL 逐 ACE 分拣——目标 SID 的 grant 覆盖 [`GRANT_MASK`]
/// = 已达标，不写回（standing ACE 跨会话复用的关键路径：第二次遍历只有读
/// IO）。目标 SID 的**任何不达标形态**（deny 项、含-DELETE 的超集 grant
/// ——只可能来自外部篡改或旧加固档残留）标 stale 强制重建，走 revoke_ace
/// 验证过的**确定性重建**（其他 SID 项原样保留 + 目标形态重写，空底表
/// `SetEntriesInAclW` → 非保护 `SetNamedSecurityInfoW`——父目录可继承 ACE
/// 由系统自动重流入）。
///
/// **junction/symlink 跳过**：`symlink_metadata` 判定，链接项不打不跟随
/// （写入面 restricted 检查按最终解析路径评估，链接自身有没有 ACE 无关
/// 语义；万一 DACL 打点跟随目标还会把 ACE 泄到树外——保守跳过）。根为
/// symlink → Err（工作区根必须是真实目录）。
///
/// `max_files` 预算同 [`label_tree`]：超限 **Err**（已打部分保持生效）。
pub fn ensure_grant_ace_tree(
    root: &Path,
    workspace_sid: &str,
    max_files: usize,
) -> Result<usize, String> {
    let sid_w = wide(workspace_sid);
    unsafe {
        let mut sid_ptr: PSID = std::ptr::null_mut();
        if ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid_ptr) == 0 {
            return Err(last_err("ConvertStringSidToSidW"));
        }
        let r = ensure_grant_ace_tree_inner(root, sid_ptr, max_files);
        LocalFree(sid_ptr as _);
        r
    }
}

fn ensure_grant_ace_tree_inner(
    root: &Path,
    sid_ptr: PSID,
    max_files: usize,
) -> Result<usize, String> {
    let meta = std::fs::symlink_metadata(root)
        .map_err(|e| format!("ensure_grant_ace_tree: 元数据 {} 失败: {e}", root.display()))?;
    if meta.is_symlink() {
        return Err(
            "ensure_grant_ace_tree: 根是 symlink/junction——工作区根必须是真实目录".to_string(),
        );
    }
    if !meta.is_dir() {
        return Err(format!(
            "ensure_grant_ace_tree: 根 {} 不是目录",
            root.display()
        ));
    }
    ensure_grant_ace_single(root, sid_ptr, true)?;
    let mut count: usize = 1;
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("ensure_grant_ace_tree: 读目录 {} 失败: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("ensure_grant_ace_tree: 目录项读取失败: {e}"))?;
            let p = entry.path();
            let meta = std::fs::symlink_metadata(&p)
                .map_err(|e| format!("ensure_grant_ace_tree: 元数据 {} 失败: {e}", p.display()))?;
            // 链接项跳过（不跟随理由见函数文档）——不压栈不打不计数。
            if meta.is_symlink() {
                continue;
            }
            ensure_grant_ace_single(&p, sid_ptr, meta.is_dir())?;
            if meta.is_dir() {
                stack.push(p.clone());
            }
            count += 1;
            if count > max_files {
                return Err(format!(
                    "ensure_grant_ace_tree: 预算耗尽（已处理 {count} > 上限 {max_files}）\
                     ——已打部分保持生效，请调大 max_files 或收窄工作区后重跑"
                ));
            }
        }
    }
    Ok(count)
}

/// 单对象 standing ACE 幂等落位（`is_dir` 决定继承位）。已达标 = 不写回
/// （false）；否则确定性重建写回（目标 SID 的旧形态残留——deny 项、含
/// DELETE 的超集 grant——一并滤除）。句柄全部 LocalFree。
fn ensure_grant_ace_single(path: &Path, ws_sid: PSID, is_dir: bool) -> Result<(), String> {
    let path_w = wide(&path.as_os_str().to_string_lossy());
    unsafe {
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let hr = GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old_dacl,
            std::ptr::null_mut(),
            &mut psd,
        );
        if hr != 0 {
            return Err(win32_err("GetNamedSecurityInfoW(DACL)", hr));
        }
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let r = (|| {
            const INHERIT_MASK: u32 = OBJECT_INHERIT_ACE
                | CONTAINER_INHERIT_ACE
                | NO_PROPAGATE_INHERIT_ACE
                | INHERIT_ONLY_ACE;
            let mut eas: Vec<EXPLICIT_ACCESS_W> = Vec::new();
            let mut grant_ok = false;
            // 任一目标 SID 的**不达标形态**现存项 → 强制重建（satisfied 短路
            // 禁用）——deny 项（证伪的加固档残留）、含 DELETE 的超集 grant
            // （防未来误掐）都只可能来自外部篡改或旧档残留，一律滤除收敛到
            // 单形态（grant 覆盖 GRANT_MASK、无 deny）。
            let mut stale_ws_ace = false;
            if !old_dacl.is_null() {
                for i in 0..(*old_dacl).AceCount {
                    let mut pace: *mut c_void = std::ptr::null_mut();
                    if GetAce(old_dacl, i as u32, &mut pace) == 0 || pace.is_null() {
                        return Err(last_err("GetAce"));
                    }
                    let hdr = pace as *const ACE_HEADER;
                    let (mode, mask, ace_sid) = match (*hdr).AceType {
                        0 => {
                            let a = pace as *const ACCESS_ALLOWED_ACE;
                            (
                                GRANT_ACCESS,
                                (*a).Mask,
                                &(*a).SidStart as *const u32 as PSID,
                            )
                        }
                        1 => {
                            let a = pace as *const ACCESS_DENIED_ACE;
                            (DENY_ACCESS, (*a).Mask, &(*a).SidStart as *const u32 as PSID)
                        }
                        t => {
                            // 同 revoke_ace：未知 ACE 类型拒绝静默丢弃。
                            return Err(format!(
                                "ensure_grant_ace: 第 {i} 条 ACE 类型 {t} 未支持，拒绝静默丢弃"
                            ));
                        }
                    };
                    if EqualSid(ace_sid, ws_sid) != 0 {
                        // 目标 SID 的现存项：grant 覆盖 GRANT_MASK = 达标形态
                        // （重建时以目标形态重写）；其余形态标 stale（滤除）。
                        match mode {
                            GRANT_ACCESS => {
                                if mask & GRANT_MASK == GRANT_MASK {
                                    grant_ok = true;
                                } else {
                                    stale_ws_ace = true;
                                }
                            }
                            _ => stale_ws_ace = true,
                        }
                        continue;
                    }
                    // 其他 SID 的项原样保留（继承位保真，INHERITED_ACE 位丢弃
                    // 由非保护写回触发系统重继承——revoke_ace 已验证的模式）。
                    eas.push(EXPLICIT_ACCESS_W {
                        grfAccessPermissions: mask,
                        grfAccessMode: mode,
                        grfInheritance: (*hdr).AceFlags as u32 & INHERIT_MASK,
                        Trustee: TRUSTEE_W {
                            pMultipleTrustee: std::ptr::null_mut(),
                            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                            TrusteeForm: TRUSTEE_IS_SID,
                            TrusteeType: TRUSTEE_IS_UNKNOWN,
                            ptstrName: ace_sid as *mut u16,
                        },
                    });
                }
            }
            let satisfied = grant_ok && !stale_ws_ace;
            if satisfied {
                return Ok(()); // 已达标——standing ACE 复用路径，不写回。
            }
            // 目标形态追加：GRANT（目录 OI|CI）。被滤除的旧形态项在此一并
            // 重写/清除。
            eas.push(EXPLICIT_ACCESS_W {
                grfAccessPermissions: GRANT_MASK,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: if is_dir {
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
                } else {
                    0
                },
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: ws_sid as *mut u16,
                },
            });
            let hr2 = SetEntriesInAclW(
                eas.len() as u32,
                eas.as_ptr(),
                std::ptr::null(), // 空底表：eas 已是全量分拣结果。
                &mut new_dacl,
            );
            if hr2 != 0 {
                return Err(win32_err("SetEntriesInAclW", hr2));
            }
            let hr3 = SetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null(), // 非保护 DACL：父目录继承自动重流入。
            );
            if hr3 != 0 {
                return Err(win32_err("SetNamedSecurityInfoW(DACL)", hr3));
            }
            Ok(())
        })();
        if !new_dacl.is_null() {
            LocalFree(new_dacl as _);
        }
        if !psd.is_null() {
            LocalFree(psd as _);
        }
        r
    }
}

/// DACL 定向档 D4 状态面（2026-09-27）：只读查根对象的 standing ACE 达标态
/// ——**不打标、不写回、零副作用**（与 [`ensure_grant_ace_tree`] 的幂等写
/// 路径区分：状态查询绝不能触发打标）。返回
/// `(grant 达标, deny 现存, 是否目录)`；grant 达标按 [`GRANT_MASK`] 覆盖判
/// （与铺设幂等判定同判据）；deny 面是**纯观测**——生产从不打 deny，现存
/// 即外部篡改/旧残留痕迹，如实反映不处置。链接根 → Err（与
/// [`ensure_grant_ace_tree`] 同判据）；对象不存在 / 读 DACL 失败 → Err。
pub fn root_standing_ace_state(
    root: &Path,
    workspace_sid: &str,
) -> Result<(bool, bool, bool), String> {
    let meta = std::fs::symlink_metadata(root).map_err(|e| {
        format!(
            "root_standing_ace_state: 元数据 {} 失败: {e}",
            root.display()
        )
    })?;
    if meta.is_symlink() {
        return Err("root_standing_ace_state: 根是 symlink/junction".to_string());
    }
    let is_dir = meta.is_dir();
    let sid_w = wide(workspace_sid);
    let path_w = wide(&root.as_os_str().to_string_lossy());
    unsafe {
        let mut sid_ptr: PSID = std::ptr::null_mut();
        if ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid_ptr) == 0 {
            return Err(last_err("ConvertStringSidToSidW"));
        }
        let mut psd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let hr = GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut psd,
        );
        let r = (|| -> Result<(bool, bool), String> {
            if hr != 0 {
                return Err(win32_err("GetNamedSecurityInfoW(DACL)", hr));
            }
            let mut grant_ok = false;
            let mut deny_ok = false;
            if !dacl.is_null() {
                for i in 0..(*dacl).AceCount {
                    let mut pace: *mut c_void = std::ptr::null_mut();
                    if GetAce(dacl, i as u32, &mut pace) == 0 || pace.is_null() {
                        return Err(last_err("GetAce"));
                    }
                    let hdr = pace as *const ACE_HEADER;
                    let (mode, mask, ace_sid) = match (*hdr).AceType {
                        0 => {
                            let a = pace as *const ACCESS_ALLOWED_ACE;
                            (
                                GRANT_ACCESS,
                                (*a).Mask,
                                &(*a).SidStart as *const u32 as PSID,
                            )
                        }
                        1 => {
                            let a = pace as *const ACCESS_DENIED_ACE;
                            (DENY_ACCESS, (*a).Mask, &(*a).SidStart as *const u32 as PSID)
                        }
                        // 状态面只读：审计类 ACE（ML/system audit 等）与本探针
                        // 无关语义，跳过（写路径里是拒绝静默丢弃——语义不同，
                        // 写路径改 ACL 会丢信息，读路径不会）。
                        _ => continue,
                    };
                    if EqualSid(ace_sid, sid_ptr) != 0 {
                        match mode {
                            GRANT_ACCESS => grant_ok |= mask & GRANT_MASK == GRANT_MASK,
                            _ => deny_ok |= mask & FILE_DELETE_CHILD != 0,
                        }
                    }
                }
            }
            Ok((grant_ok, deny_ok))
        })();
        if !psd.is_null() {
            LocalFree(psd as _);
        }
        LocalFree(sid_ptr as _);
        let (grant_ok, deny_ok) = r?;
        Ok((grant_ok, deny_ok, is_dir))
    }
}
