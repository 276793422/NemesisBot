//! DACL 定向档 D3 测试：write-restricted 令牌 + spawn 事务 + 写围栏 E2E。
//!
//! 真进程测试的子进程 = **本测试二进制自身**（`current_exe` + libtest 过滤器
//! args，[`dacl_child_entry`] 是哨兵入口，按 env `NB_DACL_CHILD` 分臂）——
//! 这是受控子进程的惯用形态：不依赖外部二进制，围栏语义用真实文件系统验证。
//!
//! 整文件与 token.rs 同门控（Windows + `acl` feature）——令牌铸造与 spawn
//! 在其他平台没有意义。

use std::ffi::c_void;

use super::Availability;
use super::acl_impl::ensure_grant_ace_tree;
use super::sid::derive_workspace_sid;
use super::token::{
    TxnOutcome, WriteRestrictedToken, create_write_restricted_token,
    create_write_restricted_token_with_groups, dacl_availability, sid_to_string, stdio_txn_raw,
};

/// 子进程哨兵 env（模式臂）。
const CHILD_ENV: &str = "NB_DACL_CHILD";
/// 子进程哨兵 env（工作区根 / 围栏外目录 / env 注入探针值）。
const CHILD_ENV_WS: &str = "NB_DACL_WS";
const CHILD_ENV_OUT: &str = "NB_DACL_OUT";
const CHILD_ENV_MARKER: &str = "NB_DACL_MARKER";
/// execsim 臂 env：spawn 的 creation flags（十进制 u32）与是否 current_dir(ws)。
const CHILD_ENV_EXEC_FLAGS: &str = "NB_DACL_EXEC_FLAGS";
const CHILD_ENV_EXEC_CWD: &str = "NB_DACL_EXEC_CWD";
const CHILD_ENV_EXEC_FULLPATH: &str = "NB_DACL_EXEC_FULLPATH";
const CHILD_ENV_EXEC_PIPED: &str = "NB_DACL_EXEC_PIPED";
/// fence 对照臂开关（=1 时 execsim 先跑 cmd 树外重定向 + std::fs::write 对照）。
const CHILD_ENV_EXEC_OUTSIDE: &str = "NB_DACL_EXEC_OUTSIDE";

/// 子进程退出码约定（>0 都带 eprintln 诊断，走 --nocapture 进 stderr_tail）。
const EXIT_BAD_MODE: i32 = 9;
const EXIT_MARKER_MISS: i32 = 7;
const EXIT_INSIDE_DENIED: i32 = 21;
const EXIT_OUTSIDE_READ_DENIED: i32 = 22;
const EXIT_FENCE_BROKEN: i32 = 23;
const EXIT_GRANDCHILD_FAIL: i32 = 24;
const EXIT_ATTACH_FAIL: i32 = 25;
const EXIT_EXECSIM_FAIL: i32 = 26;
/// F9 证伪（2026-09-28）rename/delete 断言面：27=树内改名被拒（F9 所称
/// 「掐 DELETE 断 rename」与实证相反——本码防有人再掐回去假回归）29=树内
/// →树外改名成功（逃逸）30=树内删除被拒。**已知边界**：DELETE 不在
/// write-restricted 写类评估集合 → 树内删除/改名**拦不住**（trustee=ws_sid
/// 的 deny ACE 只影响 restricting 侧），「加固档」形态被真进程证伪为零拦截
/// 力——治理归 8 层策略层，见 ACL 定向档报告 §3.9/§5。
const EXIT_RENAME_DENIED: i32 = 27;
const EXIT_RENAME_ESCAPE: i32 = 29;
const EXIT_DELETE_DENIED: i32 = 30;

// ---------------------------------------------------------------------------
// 子进程哨兵入口
// ---------------------------------------------------------------------------

/// 子进程哨兵（父测试经受限令牌 spawn 本二进制 + `--exact` 过滤器驱动）。
/// 常规套件运行时 env 缺省 → 空跑直接通过，零副作用。
#[test]
fn dacl_child_entry() {
    let mode = std::env::var(CHILD_ENV).unwrap_or_default();
    match mode.as_str() {
        // 常规套件空跑 / exit_ok：正常 return，libtest 报 ok → 退出码 0。
        "" | "exit_ok" => {}
        "env_probe" => {
            if std::env::var(CHILD_ENV_MARKER).as_deref() != Ok("v4-wr-token") {
                eprintln!(
                    "[env_probe] env 注入未生效（{CHILD_ENV_MARKER} 缺失或不等于 v4-wr-token）"
                );
                std::process::exit(EXIT_MARKER_MISS);
            }
        }
        "hang" => {
            // 超时杀进程测试用：睡够长让父进程的 timeout 窗先到。
            std::thread::sleep(std::time::Duration::from_secs(300));
        }
        "fence" => {
            let ws =
                std::path::PathBuf::from(std::env::var(CHILD_ENV_WS).unwrap_or_else(|_| "".into()));
            let out_dir = std::path::PathBuf::from(
                std::env::var(CHILD_ENV_OUT).unwrap_or_else(|_| "".into()),
            );
            fence_dump_token(&out_dir);
            // a) 工作区内写：standing GRANT ACE 放行（restricted 检查两边合取
            //    都过）。
            let inside = ws.join("inside.txt");
            if let Err(e) = std::fs::write(&inside, b"hi") {
                eprintln!("[fence] 工作区内写入意外被拒: {e}");
                std::process::exit(EXIT_INSIDE_DENIED);
            }
            // b) 工作区外读：write-restricted 语义（WRITE_RESTRICTED flag
            //    让 restricting 检查只查写类访问）的直接实证——读必须放行
            //    （被拒 = 语义与设计不符，22 号退出码让父测试如实红出来）。
            let outside = out_dir.join("outside.txt");
            if let Err(e) = std::fs::read_to_string(&outside) {
                eprintln!("[fence] 工作区外读取意外被拒（write-restricted 读放行语义被推翻）: {e}");
                std::process::exit(EXIT_OUTSIDE_READ_DENIED);
            }
            // c) 工作区外写：核心围栏——restricting 检查两边合取里 workspace
            //    SID 在树外无 ACE → 必须被拒。
            match std::fs::write(out_dir.join("violated.txt"), b"x") {
                Ok(()) => {
                    eprintln!("[fence] 围栏失守：工作区外写入成功！");
                    std::process::exit(EXIT_FENCE_BROKEN);
                }
                Err(e) => eprintln!("[fence] 工作区外写入被拒（符合预期）: {e}"),
            }
            // d) 工作区外删除：观测点（不设失败条件）——DELETE 是否落入
            //    WRITE_RESTRICTED 的「写类」判定集合，MSDN 未逐位枚举；实测
            //    结果打进 stderr 供报告/D4 逃逸边界文档记录真实语义。
            //    **2026-09-28 实测定论：DELETE 不在写类集合**（树外无 ACE 仍
            //    删成功）——这是「掐 DELETE 加固」零拦截力的机制根因。
            match std::fs::remove_file(&outside) {
                Ok(()) => {
                    eprintln!("[fence] 工作区外删除成功（DELETE 不在写类判定集合——已知边界）")
                }
                Err(e) => eprintln!("[fence] 工作区外删除被拒（DELETE 在写类判定集合）: {e}"),
            }
            // e) 树内改名（F9 证伪锚，2026-09-28）：Windows rename 需要源文件
            //    DELETE 权——**mask 保含 DELETE 的基线下必须通**（git
            //    lock→rename / cargo 原子替换 / mv 的内核语义锚）。被拒 = 有
            //    人把 DELETE 掐回去了（假回归，27 号退出码红出来）。
            let renamed = ws.join("inside_renamed.txt");
            match std::fs::rename(&inside, &renamed) {
                Ok(()) => eprintln!("[fence] 树内改名放行（基线能力锚——F9 证伪）"),
                Err(e) => {
                    eprintln!("[fence] 树内改名被拒（F9 假回归——基线 DELETE 被掐）: {e}");
                    std::process::exit(EXIT_RENAME_DENIED);
                }
            }
            // f) 树内→树外改名：源在树内 + 目标父目录在树外 → 必须被拒
            //    （rename 还需要目标父目录 FILE_ADD_FILE——树外无 ACE——
            //    DELETE 放行不许把改名通道开到树外，围栏完整性锚）。
            match std::fs::rename(&renamed, out_dir.join("escaped.txt")) {
                Ok(()) => {
                    eprintln!("[fence] 围栏失守：树内→树外改名成功！");
                    std::process::exit(EXIT_RENAME_ESCAPE);
                }
                Err(e) => eprintln!("[fence] 树外改名被拒（符合预期）: {e}"),
            }
            // g) 树内删除（rm 基线能力锚）：基线 mask 保含 DELETE 必须通。
            match std::fs::remove_file(&renamed) {
                Ok(()) => eprintln!("[fence] 树内删除放行（基线能力锚）"),
                Err(e) => {
                    eprintln!("[fence] 树内删除被拒（F9 假回归）: {e}");
                    std::process::exit(EXIT_DELETE_DENIED);
                }
            }
            // 残留清理尽力而为（g) 已删掉 renamed；inside 已被 rename 走）。
            let _ = std::fs::remove_file(&inside);
        }
        // AttachConsole 臂：受限子进程（DETACHED，console-less）**附着**到
        // 已存在的 console（父=有 console 的测试进程）。附着是打开既有
        // condrv 对象（非写类）而非新建——2026-09-27 实证白名单下放行，且
        // 附着后默认 flags 孙进程继承该 console 存活（与 S-1-2-1 无关，对照
        // 实验已定论）→ 生产形态「顶部进程持 console + 受限树附着」可解锁
        // 第三方工具链（cargo→rustc 类）默认 flags 链式 spawn。父测试进程
        // 无 console 的环境（headless CI）attach 诚实失败退 25。
        "attachconsole" => {
            use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
            let rc = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
            if rc == 0 {
                let err = std::io::Error::last_os_error();
                eprintln!("[attachconsole] AttachConsole 失败: {err}");
                std::process::exit(EXIT_ATTACH_FAIL);
            }
            eprintln!("[attachconsole] AttachConsole 成功");
            // 附着成功后，默认 flags 孙进程继承该 console（无新分配）→ 应存活。
            match std::process::Command::new("C:\\Windows\\System32\\cmd.exe")
                .args(["/c", "exit 0"])
                .status()
            {
                Ok(s) if s.success() => {
                    eprintln!("[attachconsole] 附着后默认孙进程存活 exit=0")
                }
                other => {
                    eprintln!("[attachconsole] 附着后默认孙进程仍死: {other:?}");
                    std::process::exit(EXIT_ATTACH_FAIL);
                }
            }
        }
        // execsim 臂（D4 真进程二分，2026-09-27）：受限子进程内复现 exec 工具
        // 的 spawn 形态——`cmd /C` + 可选 current_dir(workspace) + creation
        // flags 变体，stdio piped（loop_tools make_piped_shell_command 同款）。
        // spawn 失败 → 26 退码带诊断；成功打出 status/stdout 供父侧断言。
        "execsim" => {
            use std::os::windows::process::CommandExt;
            let ws =
                std::path::PathBuf::from(std::env::var(CHILD_ENV_WS).unwrap_or_else(|_| "".into()));
            let flags = std::env::var(CHILD_ENV_EXEC_FLAGS)
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0x0000_0008); // 缺省 = DETACHED（生产形态）
            let with_cwd = std::env::var(CHILD_ENV_EXEC_CWD).as_deref() == Ok("1");
            let full_path = std::env::var(CHILD_ENV_EXEC_FULLPATH).as_deref() == Ok("1");
            let piped = std::env::var(CHILD_ENV_EXEC_PIPED).unwrap_or_else(|_| "1".into());
            let program = if full_path {
                "C:\\Windows\\System32\\cmd.exe"
            } else {
                "cmd"
            };
            let mut c = std::process::Command::new(program);
            // fence 对照臂（outside=1）：同进程内先跑 cmd 重定向写树外（复刻
            // 生产 exec 工具形态）——status/落盘/cmd_stderr 全打印进本进程
            // stderr（→ 父侧 stderr_tail）供对照观测；落盘面硬断言在父侧
            // （observation 测试按白名单形态逐项断言）。
            if std::env::var(CHILD_ENV_EXEC_OUTSIDE).as_deref() == Ok("1") {
                let outside_dir = std::path::PathBuf::from(
                    std::env::var(CHILD_ENV_OUT).unwrap_or_else(|_| "".into()),
                );
                let target = outside_dir.join("violated_by_cmd.txt");
                // cmd 自身 stdout/stderr 落工作区观测文件（树外建不了文件——
                // 受限侧 File::create(outside) 本身就会被拒），跑完读回打进
                // 本进程 stderr（→ 父侧 stderr_tail）。
                let obs_out = ws.join("obs_cmd_out.tmp");
                let obs_err = ws.join("obs_cmd_err.tmp");
                let mut cc = std::process::Command::new(program);
                cc.args(["/C", &format!("echo cmd_wrote > {}", target.display())])
                    .creation_flags(0x0000_0008); // DETACHED（生产 exec 同款）
                if let Ok(f) = std::fs::File::create(&obs_out) {
                    cc.stdout(std::process::Stdio::from(f));
                }
                if let Ok(f) = std::fs::File::create(&obs_err) {
                    cc.stderr(std::process::Stdio::from(f));
                }
                match cc.status() {
                    Ok(s) => {
                        let landed = target.exists();
                        let err_txt = std::fs::read_to_string(&obs_err).unwrap_or_default();
                        eprintln!(
                            "[outside] cmd 重定向树外写 target={}：status={s} 落盘={landed} cmd_stderr={}",
                            target.display(),
                            err_txt.trim()
                        );
                    }
                    other => {
                        eprintln!("[outside] cmd 重定向树外写 spawn 异常: {other:?}");
                        std::process::exit(EXIT_EXECSIM_FAIL);
                    }
                }
            }
            c.args(["/C", "echo execsim_ok"]);
            // piped 三态：1=std piped（命名管道创建，受限下死）/ 0=inherit /
            // file=workspace 内文件重定向（D4 修复形态——GRANT ACE 放行文件
            // 创建，句柄继承不重评）。
            if piped == "1" {
                c.stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
            } else if piped == "file" {
                let o = ws.join("execsim_out.tmp");
                let e = ws.join("execsim_err.tmp");
                let of = std::fs::File::create(&o).expect("create out tmp");
                let ef = std::fs::File::create(&e).expect("create err tmp");
                c.stdout(std::process::Stdio::from(of))
                    .stderr(std::process::Stdio::from(ef));
            }
            if with_cwd {
                c.current_dir(&ws);
            }
            c.creation_flags(flags);
            // ⚠ `output()` 隐式强制 stdout/stderr=piped（Stdio 设置会被它覆盖）
            // ——inherit 臂必须走 `status()`（继承 stdio）才算真 inherit。
            if piped == "0" {
                match c.status() {
                    Ok(s) => eprintln!(
                        "[execsim] spawn ok flags={flags:#x} cwd={with_cwd} full={full_path} piped={piped} status={s:?}（inherit）"
                    ),
                    Err(e) => {
                        eprintln!(
                            "[execsim] spawn FAILED flags={flags:#x} cwd={with_cwd} full={full_path} piped={piped}: {e}"
                        );
                        std::process::exit(EXIT_EXECSIM_FAIL);
                    }
                }
            } else {
                match c.output() {
                    Ok(o) => {
                        let out = if piped == "file" {
                            String::from_utf8_lossy(
                                &std::fs::read(ws.join("execsim_out.tmp")).unwrap_or_default(),
                            )
                            .trim()
                            .to_string()
                        } else {
                            String::from_utf8_lossy(&o.stdout).trim().to_string()
                        };
                        eprintln!(
                            "[execsim] spawn ok flags={flags:#x} cwd={with_cwd} full={full_path} piped={piped} status={:?} stdout={out}",
                            o.status
                        );
                        // 输出文件读回后尽力删除（基线 mask 保含 DELETE，
                        // 受限下自删通常成功；失败=残留非失败条件）。
                        let _ = std::fs::remove_file(ws.join("execsim_out.tmp"));
                        let _ = std::fs::remove_file(ws.join("execsim_err.tmp"));
                    }
                    Err(e) => {
                        eprintln!(
                            "[execsim] spawn FAILED flags={flags:#x} cwd={with_cwd} full={full_path} piped={piped}: {e}"
                        );
                        std::process::exit(EXIT_EXECSIM_FAIL);
                    }
                }
            }
        }
        other => {
            eprintln!("[dacl_child_entry] 未知 mode: {other}");
            std::process::exit(EXIT_BAD_MODE);
        }
    }
}

/// 受限子进程内 spawn 控制台孙进程（exec 工具链形态代理：真实 executor 会
/// 经 exec 工具跑 shell 命令，孙进程默认带 CREATE_NO_WINDOW=隐藏 console）。
/// 孙进程活不活得成 = 白名单围栏与真实 exec 工作负载的兼容性裁决。
#[test]
fn dacl_grandchild_entry() {
    let mode = std::env::var(CHILD_ENV).unwrap_or_default();
    if mode != "grandchild" {
        return; // 常规套件空跑
    }
    // 臂 a（观测点，不判失败）：默认 flags。console 子系统孙进程在
    // console-less 父进程下必须**新分配** console（窗口形态）→ 对 condrv
    // 写类访问 → 白名单 restricting 必死 0xC0000142（2026-09-27 实证；
    // 此前 diag 里 default 存活是**继承假象**——diag 父测试进程自带 console，
    // 孙进程继承无需分配）。这正是「受限树内一切受控 spawn 必须 DETACHED /
    // 继承路径」诚实边界的实证锚点：第三方工具链（cargo→rustc 类）以默认
    // flags 链式 spawn console 子进程会断链。
    match std::process::Command::new("C:\\Windows\\System32\\cmd.exe")
        .args(["/c", "exit 0"])
        .status()
    {
        Ok(s) if s.success() => eprintln!("[grandchild] 默认 flags：孙进程存活 exit=0"),
        other => eprintln!(
            "[grandchild] 默认 flags 孙进程死（预期观测：新 console 分配必死）: {other:?}"
        ),
    }
    // 臂 c：DETACHED_PROCESS（无 console 形态——executor 父子链的生产形态）。
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    match std::process::Command::new("C:\\Windows\\System32\\cmd.exe")
        .args(["/c", "exit 0"])
        .creation_flags(DETACHED_PROCESS)
        .status()
    {
        Ok(s) if s.success() => eprintln!("[grandchild] DETACHED_PROCESS：孙进程存活 exit=0"),
        other => {
            eprintln!("[grandchild] DETACHED_PROCESS 孙进程异常: {other:?}");
            std::process::exit(EXIT_GRANDCHILD_FAIL);
        }
    }
    // 臂 b（观测点，不判失败）：CREATE_NO_WINDOW（nemesisbot exec 工具现行
    // 形态；隐藏 console 也是**新分配** → 实测死。受限模式下 exec 工具必须
    // 避开此形态，D4 接线时改）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match std::process::Command::new("C:\\Windows\\System32\\cmd.exe")
        .args(["/c", "exit 0"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
    {
        Ok(s) if s.success() => eprintln!("[grandchild] CREATE_NO_WINDOW：孙进程存活 exit=0"),
        other => eprintln!("[grandchild] CREATE_NO_WINDOW 孙进程死（预期观测）: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 父侧测试
// ---------------------------------------------------------------------------

/// 子进程自证：本进程主令牌的 IsTokenRestricted + 全部 restricting SIDs
/// （dump 进 stderr，随 stderr_tail 回到父测试断言信息里）。围栏异常时
/// 第一时间分辨「令牌没施加」vs「施加了但检查没拦」。
fn fence_dump_token(out_dir: &std::path::Path) {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, IsTokenRestricted, TOKEN_GROUPS, TOKEN_QUERY, TokenRestrictedSids,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    eprintln!("[fence-dump] out_dir={}", out_dir.display());
    unsafe {
        let mut tok: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            eprintln!("[fence-dump] OpenProcessToken 失败");
            return;
        }
        eprintln!("[fence-dump] IsTokenRestricted={}", IsTokenRestricted(tok));
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(
            tok,
            TokenRestrictedSids,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed > 0 {
            // u64 背书对齐缓冲（TOKEN_GROUPS 含 PSID 指针字段，对齐 8——
            // 同 mint_restricted 的对齐纪律）。
            let mut buf = vec![0u64; needed.div_ceil(size_of::<u64>() as u32) as usize];
            if GetTokenInformation(
                tok,
                TokenRestrictedSids,
                buf.as_mut_ptr() as *mut c_void,
                needed,
                &mut needed,
            ) != 0
            {
                let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
                let base = groups.Groups.as_ptr();
                for i in 0..groups.GroupCount as usize {
                    let e = *base.add(i);
                    if let Some(s) = sid_to_string(e.Sid) {
                        eprintln!("[fence-dump] restricted[{i}] = {s}");
                    }
                }
            }
        } else {
            eprintln!("[fence-dump] TokenRestrictedSids 查询长度为 0");
        }
        CloseHandle(tok);
    }
}

/// 用受限令牌 spawn 本二进制跑指定模式臂（公共驱动）。
///
/// ⚠ `DETACHED_PROCESS` 是生产形态：受限令牌子进程的 console 分配（连
/// CREATE_NO_WINDOW 的隐藏 console 也算）会对 \Device\ConDrv 做写类访问，
/// 白名单 restricting 下必死 0xC0000142（2026-09-27 实证）；executor 协议
/// stdio 全走管道，不需要 console。
fn spawn_child_mode(
    token: &WriteRestrictedToken,
    mode: &str,
    extra: &[(String, String)],
    timeout: std::time::Duration,
) -> Result<TxnOutcome, String> {
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
    let mut env: Vec<(String, String)> = vec![(CHILD_ENV.to_string(), mode.to_string())];
    env.extend_from_slice(extra);
    stdio_txn_raw(
        token,
        &std::env::current_exe().expect("current_exe 一定存在（本进程就是它）"),
        &[
            // ⚠ libtest --exact 匹配完整测试路径（backend::token_tests::…），
            //   裸 fn 名匹配 0 个测试 → 子进程空跑退 0，一切断言全部空转。
            "backend::token_tests::dacl_child_entry".to_string(),
            "--exact".to_string(),
            "--nocapture".to_string(),
        ],
        &env,
        "", // 哨兵子进程不读协议行；空请求行让 stdin 立即 EOF
        DETACHED_PROCESS,
        timeout,
    )
}

/// 令牌的 restricting SIDs → 字符串集合（TokenRestrictedSids 信息类查询；
/// TOKEN_GROUPS 是变长结构，按 GroupCount 指针步进而非数组索引）。
fn restricted_sid_strings(handle: windows_sys::Win32::Foundation::HANDLE) -> Vec<String> {
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_GROUPS, TokenRestrictedSids};
    unsafe {
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(
            handle,
            TokenRestrictedSids,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            return Vec::new();
        }
        // u64 背书对齐缓冲（同上——TOKEN_GROUPS 指针对齐 8）。
        let mut buf = vec![0u64; needed.div_ceil(size_of::<u64>() as u32) as usize];
        if GetTokenInformation(
            handle,
            TokenRestrictedSids,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        ) == 0
        {
            return Vec::new();
        }
        let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        let base = groups.Groups.as_ptr();
        let mut out = Vec::new();
        for i in 0..groups.GroupCount as usize {
            let entry = *base.add(i);
            if let Some(s) = sid_to_string(entry.Sid) {
                out.push(s);
            }
        }
        out
    }
}

/// 令牌铸造：成功 + IsTokenRestricted 为真 + restricting SIDs 满足白名单
/// 结构断言（ws_sid 在列 / 普通用户组面在列 / 高特权组与标签与用户自身
/// SID 不在列）+ 本机探针 Full。
#[test]
fn token_create_restricted_sids_introspection() {
    assert!(
        matches!(dacl_availability(), Availability::Full),
        "本机探针应 Full（Vista+ 恒真 API，失败=机器态异常）"
    );
    let ws_sid = "S-1-5-21-12345-67890-54321";
    let token = create_write_restricted_token(ws_sid).expect("令牌铸造（无需特权）");
    assert!(!token.handle.is_null());

    use windows_sys::Win32::Security::IsTokenRestricted;
    assert_ne!(
        unsafe { IsTokenRestricted(token.handle) },
        0,
        "令牌应是 restricted"
    );

    let sids = restricted_sid_strings(token.handle);
    assert!(
        sids.iter().any(|s| s == ws_sid),
        "workspace SID 应在 restricting 列表: {sids:?}"
    );
    // 组面 = Everyone-only（2026-09-28 围栏失守收窄后的生产形态）。
    assert!(
        sids.iter().any(|s| s == "S-1-1-0"),
        "Everyone 应在 restricting 列表: {sids:?}"
    );
    assert!(
        !sids.iter().any(|s| s == "S-1-5-11"),
        "Authenticated Users 不应入 restricting 列表（C:\\ 根默认 Modify 开口，围栏失守元凶）: {sids:?}"
    );
    assert!(
        !sids.iter().any(|s| s == "S-1-5-32-545"),
        "BUILTIN\\Users 不应入 restricting 列表（单组实验非必需；组授予位=开口面）: {sids:?}"
    );
    assert!(
        sids.iter().any(|s| s.starts_with("S-1-5-5-")),
        "logon sid 应在 restricting 列表: {sids:?}"
    );
    // S-1-2-1（console logon）实证不进列表：不解锁新 console 分配（孙进程
    // 默认 flags 仍死），AttachConsole 放行也不依赖它（diag_attach 对照）——
    // 两头无用即死代码（2026-09-27 已从生产白名单删除）。
    assert!(
        !sids.iter().any(|s| s == "S-1-2-1"),
        "S-1-2-1 不应入 restricting 列表（实证死代码）: {sids:?}"
    );
    // 高特权组排除（admin 全盘写面穿透围栏的实证教训）
    assert!(
        !sids.iter().any(|s| s == "S-1-5-32-544"),
        "Administrators 不应入 restricting 列表: {sids:?}"
    );
    assert!(
        !sids.iter().any(|s| s == "S-1-5-113"),
        "S-1-5-113（本地账号+admin 成员）不应入 restricting 列表: {sids:?}"
    );
    assert!(
        !sids.iter().any(|s| s.starts_with("S-1-16-")),
        "完整性标签 SID 不应入 restricting 列表: {sids:?}"
    );
    let user = current_user_sid();
    assert!(
        sids.iter().all(|s| s.as_str() != user),
        "用户自身 SID 不应入 restricting 列表（收窄面）: {sids:?}"
    );
}

/// 本进程基础令牌的用户 SID（字符串形态；用户排除断言用）。
fn current_user_sid() -> String {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        assert_ne!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token),
            0,
            "OpenProcessToken"
        );
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        assert_ne!(needed, 0, "TokenUser 长度查询");
        // u64 背书对齐缓冲（TOKEN_USER 含 PSID 指针字段，对齐 8）。
        let mut buf = vec![0u64; needed.div_ceil(size_of::<u64>() as u32) as usize];
        assert_ne!(
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut c_void,
                needed,
                &mut needed
            ),
            0,
            "TokenUser 内容查询"
        );
        CloseHandle(token);
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_to_string(user.User.Sid).expect("用户 SID 字符串化")
    }
}

/// spawn 事务基本回路：受限令牌子进程正常退出，退出码如实回收。
#[test]
fn stdio_txn_spawns_child_collects_exit_code() {
    let token = create_write_restricted_token("S-1-5-21-12345-67890-54321").unwrap();
    let outcome = spawn_child_mode(&token, "exit_ok", &[], std::time::Duration::from_secs(120))
        .expect("spawn 事务应成功");
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "stderr: {}",
        outcome.stderr_tail
    );
}

/// env_extra 注入：正向（带 marker → 0）与对照（不带 marker → 7）同型 spawn
/// 只差一条 env——退出码差异即注入生效的直接证据。
#[test]
fn stdio_txn_env_extra_reaches_child() {
    let token = create_write_restricted_token("S-1-5-21-12345-67890-54321").unwrap();
    let positive = spawn_child_mode(
        &token,
        "env_probe",
        &[(CHILD_ENV_MARKER.to_string(), "v4-wr-token".to_string())],
        std::time::Duration::from_secs(120),
    )
    .expect("正向 spawn 应成功");
    assert_eq!(
        positive.exit_code,
        Some(0),
        "stderr: {}",
        positive.stderr_tail
    );

    let negative = spawn_child_mode(
        &token,
        "env_probe",
        &[],
        std::time::Duration::from_secs(120),
    )
    .expect("对照 spawn 应成功");
    assert_eq!(
        negative.exit_code,
        Some(EXIT_MARKER_MISS as u32),
        "无注入时子进程应报 marker 缺失；stderr: {}",
        negative.stderr_tail
    );
}

/// 超时窗：子进程挂住 → WaitForSingleObject 超时 → TerminateProcess + Err。
#[test]
fn stdio_txn_timeout_kills_child() {
    let token = create_write_restricted_token("S-1-5-21-12345-67890-54321").unwrap();
    let err = spawn_child_mode(&token, "hang", &[], std::time::Duration::from_secs(2))
        .expect_err("挂住子进程必须超时");
    assert!(err.contains("timed out"), "错误信息应指明超时: {err}");
}

/// 核心围栏 E2E：预置 GRANT ACE 树的工作区 + 受限令牌子进程——
/// 写工作区内放行 / 写工作区外拒绝 / **读工作区外放行**（write-restricted
/// 读不设防语义的实证，见 dacl_child_entry fence 臂 b/c 注释）/ **树内改名
/// 与删除放行**（基线能力锚：mask 保含 DELETE——rename/git/cargo/mv 与无
/// DACL 基线一致；「掐 DELETE 断 rename」的 F9 推演已被本测试实证推翻）/
/// 树内→树外改名拒绝（DELETE 放行不开树外通道——围栏完整性锚）。
/// 任一期望失守 → 子进程专属退出码 → 本测试红（诚实暴露设计断言被推翻）。
#[test]
fn restricted_token_write_fence_e2e() {
    let ws = tempfile::tempdir().unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let sid = derive_workspace_sid(ws.path()).unwrap();
    let n = ensure_grant_ace_tree(ws.path(), &sid, 10_000).expect("standing ACE 树预置");
    assert!(n >= 1, "至少根目录要打上 ACE（实打 {n}）");
    std::fs::write(out_dir.path().join("outside.txt"), "outside-marker").unwrap();

    let token = create_write_restricted_token(&sid).unwrap();
    let outcome = spawn_child_mode(
        &token,
        "fence",
        &[
            (
                CHILD_ENV_WS.to_string(),
                ws.path().to_string_lossy().into_owned(),
            ),
            (
                CHILD_ENV_OUT.to_string(),
                out_dir.path().to_string_lossy().into_owned(),
            ),
        ],
        std::time::Duration::from_secs(120),
    )
    .expect("围栏子进程 spawn 应成功");
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "围栏子进程验证失败（21=内写被拒 22=外读被拒 23=围栏失守 27=内改名被拒\
         29=改名逃逸 30=内删除被拒），stderr: {}",
        outcome.stderr_tail
    );
    // F9 证伪的输出面证据：树内改名/删除放行、树外改名被拒都要在 stderr 里
    // 有正向痕迹（exit 0 只证没踩失败臂，这里钉语义真发生）。
    assert!(
        outcome.stderr_tail.contains("树内改名放行"),
        "树内改名放行证据缺失: {}",
        outcome.stderr_tail
    );
    assert!(
        outcome.stderr_tail.contains("树内删除放行"),
        "树内删除放行证据缺失: {}",
        outcome.stderr_tail
    );
    assert!(
        outcome.stderr_tail.contains("树外改名被拒"),
        "树内→树外改名拦截证据缺失: {}",
        outcome.stderr_tail
    );
}

/// 孙进程兼容 E2E：受限子进程内再 spawn 控制台孙进程（exec 工具链的真实
/// 形态——executor 跑 shell 命令）。白名单围栏不能挡住孙进程的存活，否则
/// 本档对 exec 场景不可用。
#[test]
fn restricted_token_grandchild_console_spawn() {
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
    let token = create_write_restricted_token("S-1-5-21-12345-67890-54321").unwrap();
    let outcome = stdio_txn_raw(
        &token,
        &std::env::current_exe().unwrap(),
        &[
            "backend::token_tests::dacl_grandchild_entry".to_string(),
            "--exact".to_string(),
            "--nocapture".to_string(),
        ],
        &[(CHILD_ENV.to_string(), "grandchild".to_string())],
        "",
        DETACHED_PROCESS,
        std::time::Duration::from_secs(120),
    )
    .expect("孙进程测试 spawn 应成功");
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "孙进程兼容失败（24=受限子进程内 spawn 控制台孙进程死），stderr: {}",
        outcome.stderr_tail
    );
    // 臂语义已在 stderr 观测点落档（default/NO_WINDOW 死、DETACHED 活），
    // 子进程 exit 0 = DETACHED 臂（生产契约）通过。
    assert!(
        outcome.stderr_tail.contains("DETACHED_PROCESS：孙进程存活"),
        "DETACHED 臂存活证据缺失，stderr: {}",
        outcome.stderr_tail
    );
}

/// exec 工具形态二分（D4 真进程，2026-09-27）：受限子进程内 spawn `cmd /C`
/// 的四臂矩阵——cwd=ACE 树 workspace / 不设 cwd × DETACHED / 默认 flags，
/// 外加 cwd=无 ACE 目录对照。定位「生产 exec 工具 spawn error 5」卡在哪个
/// 变量上；四臂全活即固化为本档对 exec 场景的回归锚。
#[test]
fn restricted_token_exec_tool_shape_spawn() {
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
    let ws = tempfile::tempdir().unwrap();
    let sid = derive_workspace_sid(ws.path()).unwrap();
    let n = ensure_grant_ace_tree(ws.path(), &sid, 10_000).expect("standing ACE 树预置");
    assert!(n >= 1);
    let no_ace_dir = tempfile::tempdir().unwrap();
    let token = create_write_restricted_token(&sid).unwrap();
    let wsp = ws.path().to_string_lossy().into_owned();
    let nap = no_ace_dir.path().to_string_lossy().into_owned();

    let run = |name: &str, cwd_ws: Option<&str>, flags: u32| -> TxnOutcome {
        let mut extra = vec![
            (CHILD_ENV_EXEC_FLAGS.to_string(), flags.to_string()),
            (
                CHILD_ENV_EXEC_FULLPATH.to_string(),
                if name.contains("full") { "1" } else { "" }.to_string(),
            ),
            (
                CHILD_ENV_EXEC_PIPED.to_string(),
                if name.contains("inherit") {
                    "0"
                } else if name.contains("filepipe") {
                    "file"
                } else {
                    "1"
                }
                .to_string(),
            ),
        ];
        if let Some(dir) = cwd_ws {
            extra.push((CHILD_ENV_WS.to_string(), dir.to_string()));
            extra.push((CHILD_ENV_EXEC_CWD.to_string(), "1".to_string()));
        }
        spawn_child_mode(
            &token,
            "execsim",
            &extra,
            std::time::Duration::from_secs(120),
        )
        .unwrap_or_else(|e| panic!("{name}: spawn 事务失败: {e}"))
    };

    // 四臂全跑收集（失败不中断——一次拿到完整矩阵），最后逐臂断言。
    let a = run("arm1 cwd=ws DETACHED", Some(&wsp), DETACHED_PROCESS);
    let b = run("arm2 no-cwd DETACHED", None, DETACHED_PROCESS);
    let c = run("arm3 cwd=ws default-flags", Some(&wsp), 0);
    let d = run("arm4 cwd=no-ace DETACHED", Some(&nap), DETACHED_PROCESS);
    // 臂 5/6：全路径 cmd.exe（grandchild 同款）——切「PATH 搜索」变量。
    let e = run("arm5 full no-cwd DETACHED", None, DETACHED_PROCESS);
    let f = run("arm6 full cwd=ws DETACHED", Some(&wsp), DETACHED_PROCESS);
    // 臂 7：inherit stdio（不建新管道）——切「piped 管道创建」变量。
    let g = run("arm7 full no-cwd DETACHED inherit", None, DETACHED_PROCESS);
    // 臂 8：文件重定向捕获（D4 修复形态候选：cwd=ACE 树 + DETACHED + file）。
    let h = run(
        "arm8 filepipe cwd=ws DETACHED",
        Some(&wsp),
        DETACHED_PROCESS,
    );
    eprintln!(
        "[execsim-matrix] arm1(exit={:?}) arm2(exit={:?}) arm3(exit={:?}) arm4(exit={:?}) arm5(exit={:?}) arm6(exit={:?}) arm7(exit={:?}) arm8(exit={:?})",
        a.exit_code,
        b.exit_code,
        c.exit_code,
        d.exit_code,
        e.exit_code,
        f.exit_code,
        g.exit_code,
        h.exit_code
    );
    eprintln!("[execsim-matrix] arm1 tail: {}", a.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm2 tail: {}", b.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm3 tail: {}", c.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm4 tail: {}", d.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm5 tail: {}", e.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm6 tail: {}", f.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm7 tail: {}", g.stderr_tail.trim());
    eprintln!("[execsim-matrix] arm8 tail: {}", h.stderr_tail.trim());
    // arm1-6（piped）为根因观测点：std 匿名管道=命名管道实现，受限令牌对
    // \Device\NamedPipe 目录的写类访问被 restricting 侧拒（error 5）——
    // 全部死是**当前 std 实现下的预期**；不设「必须死」硬门（std 内部实现
    // 变化会让它翻转，翻转即提示本根因链需要重评）。
    for (name, o) in [
        ("arm1", &a),
        ("arm2", &b),
        ("arm3", &c),
        ("arm4", &d),
        ("arm5", &e),
        ("arm6", &f),
    ] {
        if o.exit_code != Some(0) {
            eprintln!(
                "[execsim-matrix] {name} 死（piped 语义当前预期）: {}",
                o.stderr_tail.trim()
            );
        }
    }
    // arm7（inherit）与 arm8（文件重定向）是 2026-09-28 根因链的固化锚：
    // 两形态是受限树内捕获子进程输出的唯二活路（exec 工具修复形态）。
    // arm8 额外断言输出真的经文件捕获回来了（exec 工具修复形态的语义面）。
    assert_eq!(
        g.exit_code,
        Some(0),
        "arm7 (inherit) 失败——推翻管道根因链；stderr: {}",
        g.stderr_tail
    );
    assert_eq!(
        h.exit_code,
        Some(0),
        "arm8 (文件重定向) 失败——exec 工具修复形态不可用；stderr: {}",
        h.stderr_tail
    );
    assert!(
        h.stderr_tail.contains("stdout=execsim_ok"),
        "arm8 输出必须经文件捕获回读：{}",
        h.stderr_tail
    );
}

/// 树外写语义对照观测（2026-09-28）：真进程验收抓出「cmd 重定向写树外成功」
/// （fence 的 std::fs::write 树外被拒与生产 exec 的 cmd 重定向行为相矛盾）。
/// 本测试在**同一受限进程**内跑 cmd 重定向写树外，把 status/落盘/cmd_stderr
/// 打进 stderr_tail 供父侧打印——它就是抓出「Authenticated Users 白名单开口」
/// 的探针（%TEMP% 拒 / target/ 放行 → icacls 对比定位 C:\ 根默认 AU Modify）。
/// 白名单收窄 Everyone-only 后：两目录 cmd 重定向都不得落盘——这是**生产形态
/// 围栏的硬回归锚**（生产 exec 的 spawn 形态 + 生产白名单，fence e2e 只钉
/// fs::write 形态、minimal_set_probe 钉的是 wl-minus-au 实验集）。
#[test]
fn restricted_token_cmd_outside_redirect_observation() {
    let ws = tempfile::tempdir().unwrap();
    // 外部目录用生产复现点（repo target/）与 %TEMP% 两形态各跑一遍——
    // 2026-09-28 首跑实证：%TEMP% 下 cmd 重定向被拒（exit 1），但生产
    // exec 里 cmd 写 C:\AI\NemesisBot_Rust\target\ 成功——目录位置变量
    // 的对照观测。
    for outside in [
        std::env::temp_dir(),
        std::path::PathBuf::from("C:\\AI\\NemesisBot_Rust\\target"),
    ] {
        let sid = derive_workspace_sid(ws.path()).unwrap();
        ensure_grant_ace_tree(ws.path(), &sid, 10_000).unwrap();
        let token = create_write_restricted_token(&sid).unwrap();
        let target = outside.join("violated_by_cmd.txt");
        let _ = std::fs::remove_file(&target); // 上轮残留清扫（断言前归零）
        let outcome = spawn_child_mode(
            &token,
            "execsim",
            &[
                // 观测文件（cmd 的 stdout/stderr 捕获）必须落 ACE 树内才能建
                // 出来——缺这个 env 时子进程侧 `ws` 为空、obs 文件相对路径
                // 解析到无 ACE 的 cwd，File::create 被静默拒（if let Ok 跳过）
                // → 观测退化 inherit、cmd_stderr 恒空（2026-09-28 复查修复）。
                (CHILD_ENV_WS.to_string(), ws.path().to_string_lossy().into_owned()),
                (CHILD_ENV_EXEC_OUTSIDE.to_string(), "1".to_string()),
                (
                    CHILD_ENV_OUT.to_string(),
                    outside.to_string_lossy().into_owned(),
                ),
                (CHILD_ENV_EXEC_PIPED.to_string(), "0".to_string()),
            ],
            std::time::Duration::from_secs(120),
        )
        .expect("outside 对照臂 spawn 应成功");
        assert_eq!(
            outcome.exit_code,
            Some(0),
            "outside={}: stderr: {}",
            outside.display(),
            outcome.stderr_tail
        );
        eprintln!(
            "[outside-obs dir={}] {}",
            outside.display(),
            outcome.stderr_tail.trim()
        );
        // 硬断言：生产白名单（Everyone-only）下 cmd 树外写不得落盘——
        // 白名单再扩面（如加回 AU）时这里先红，不必等真实任务复现。
        assert!(
            !target.exists(),
            "生产白名单下 cmd 树外写落盘（围栏开口回归）: {}；stderr: {}",
            target.display(),
            outcome.stderr_tail
        );
    }
}

/// 最小 restricting 集对照实验（2026-09-28 围栏失守排查）：白名单组里的
/// Authenticated Users（S-1-5-11）在本机整个 C:\AI 树有继承 Modify
/// （icacls 实证）→ 白名单形态的围栏对组可写目录**全线开口**（生产任务 B
/// 实锤：受限 exec 的 cmd 重定向写 target\ 成功）。本实验验证「无组面」
/// 最小集 = `[logon sid, ws_sid]`：
/// ① restricting 组成 introspection（组面不在列）；
/// ② 子进程初始化存活（arm8 文件重定向必活）——首版「窄集合必死」结论
///   的实验用的是 `[ws_sid, S-1-5-12]`，死因可能全在毒 SID S-1-5-12，
///   本实验是污染结论的复检；
/// ③ fence 收紧：cmd 重定向写 %TEMP% 与 C:\AI\NemesisBot_Rust\target
///   两形态文件都不得落盘。
/// ④ AttachConsole 臂观测（无组面下 console 附着是否仍放行——只打印不断
///   言，与既有 attach 测试同风格）。
#[test]
fn restricted_token_minimal_restricting_set_probe() {
    let ws = tempfile::tempdir().unwrap();
    let sid = derive_workspace_sid(ws.path()).unwrap();
    ensure_grant_ace_tree(ws.path(), &sid, 10_000).unwrap();
    let token = create_write_restricted_token_with_groups(&sid, &[]).expect("最小集铸造");

    // ① 组成 introspection。
    let sids = restricted_sid_strings(token.handle);
    assert!(
        sids.iter().any(|s| s.starts_with("S-1-5-5-")),
        "logon sid 应在列: {sids:?}"
    );
    assert!(sids.iter().any(|s| *s == sid), "ws_sid 应在列: {sids:?}");
    for banned in [
        "S-1-1-0",
        "S-1-5-11",
        "S-1-5-32-545",
        "S-1-5-4",
        "S-1-5-15",
        "S-1-2-0",
    ] {
        assert!(
            !sids.iter().any(|s| s == banned),
            "组面 {banned} 不应入最小集: {sids:?}"
        );
    }

    // ② 组面二分矩阵（观测——一次拿全数据再定生产集）：每集跑 arm8（文件
    // 重定向 + cwd=ws），exit=Some(0) 即初始化存活。空集已实证死 0xC0000142
    // （首轮实验）；这里找「初始化必需的最小组面」，重点验证「白名单去掉
    // S-1-5-11（Authenticated Users——本机 C:\ 根默认 Modify，围栏开口
    // 元凶）」是否存活。
    let wsp = ws.path().to_string_lossy().into_owned();
    let matrix: &[(&str, &[&str])] = &[
        ("empty", &[]),
        ("au-only", &["S-1-5-11"]),
        ("everyone-only", &["S-1-1-0"]),
        ("users-only", &["S-1-5-32-545"]),
        (
            "wl-minus-au",
            &[
                "S-1-1-0",      // Everyone
                "S-1-5-32-545", // BUILTIN\Users
                "S-1-5-4",      // Interactive
                "S-1-5-14",     // Remote Interactive
                "S-1-5-15",     // This Organization
                "S-1-2-0",      // LOCAL
                "S-1-5-64-10",  // NTLM Auth
                "S-1-5-64-14",  // SCHANNEL Auth
            ],
        ),
    ];
    for (name, groups) in matrix {
        let tok = match create_write_restricted_token_with_groups(&sid, groups) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[group-matrix {name}] mint FAILED: {e}");
                continue;
            }
        };
        let outcome = spawn_child_mode(
            &tok,
            "execsim",
            &[
                (CHILD_ENV_WS.to_string(), wsp.clone()),
                (CHILD_ENV_EXEC_CWD.to_string(), "1".to_string()),
                (CHILD_ENV_EXEC_PIPED.to_string(), "file".to_string()),
            ],
            std::time::Duration::from_secs(120),
        );
        match outcome {
            Ok(o) => eprintln!(
                "[group-matrix {name}] exit={:?} alive={} tail={}",
                o.exit_code,
                o.exit_code == Some(0),
                o.stderr_tail.trim()
            ),
            Err(e) => eprintln!("[group-matrix {name}] spawn FAILED: {e}"),
        }
    }

    // ③ fence 收紧观测（wl-minus-au 集——若矩阵显示该集存活）：两外部目录
    // cmd 重定向写全拒（文件不得落盘）。
    let cand = create_write_restricted_token_with_groups(
        &sid,
        matrix
            .iter()
            .find(|(name, _)| *name == "wl-minus-au")
            .expect("wl-minus-au 行必在矩阵里")
            .1,
    )
    .expect("wl-minus-au 铸造");
    for outside in [
        std::env::temp_dir(),
        std::path::PathBuf::from("C:\\AI\\NemesisBot_Rust\\target"),
    ] {
        let target = outside.join("violated_by_cmd.txt");
        let _ = std::fs::remove_file(&target);
        let outcome = spawn_child_mode(
            &cand,
            "execsim",
            &[
                (CHILD_ENV_WS.to_string(), wsp.clone()),
                (CHILD_ENV_EXEC_OUTSIDE.to_string(), "1".to_string()),
                (
                    CHILD_ENV_OUT.to_string(),
                    outside.to_string_lossy().into_owned(),
                ),
                (CHILD_ENV_EXEC_PIPED.to_string(), "0".to_string()),
            ],
            std::time::Duration::from_secs(120),
        );
        match outcome {
            Ok(o) => {
                let landed = target.exists();
                eprintln!(
                    "[wl-minus-au-fence dir={}] exit={:?} 落盘={landed} tail={}",
                    outside.display(),
                    o.exit_code,
                    o.stderr_tail.trim()
                );
                if o.exit_code == Some(0) {
                    assert!(
                        !landed,
                        "wl-minus-au 下 cmd 树外写仍落盘（该集围栏仍开口）: {}",
                        target.display()
                    );
                }
            }
            Err(e) => eprintln!(
                "[wl-minus-au-fence dir={}] spawn FAILED: {e}",
                outside.display()
            ),
        }
    }

    // ④ AttachConsole 观测（wl-minus-au 下 console 附着存活面）。
    let attach = spawn_child_mode(
        &cand,
        "attachconsole",
        &[],
        std::time::Duration::from_secs(120),
    )
    .expect("attachconsole spawn 事务应成功");
    eprintln!(
        "[wl-minus-au-attach] exit={:?} tail={}",
        attach.exit_code,
        attach.stderr_tail.trim()
    );
}

/// AttachConsole 链固化测试：受限子进程（DETACHED，console-less）附着到有
/// console 的祖先进程后，默认 flags 孙进程存活（2026-09-27 实证定论：附着
/// 放行且与 S-1-2-1 无关——「顶部进程持 console + 受限树附着」是解锁
/// cargo→rustc 类第三方链式工具链的生产形态）。保持观测性输出而非硬断言：
/// 父测试进程是否带 console 依运行环境而变（headless CI 下 attach 本就该
/// 失败退 25），失败面打 stderr 供报告留痕。
#[test]
fn restricted_token_attach_console_chain() {
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
    let token = create_write_restricted_token("S-1-5-21-12345-67890-54321").unwrap();
    let outcome = stdio_txn_raw(
        &token,
        &std::env::current_exe().unwrap(),
        &[
            "backend::token_tests::dacl_child_entry".to_string(),
            "--exact".to_string(),
            "--nocapture".to_string(),
        ],
        &[(CHILD_ENV.to_string(), "attachconsole".to_string())],
        "",
        DETACHED_PROCESS,
        std::time::Duration::from_secs(120),
    );
    match outcome {
        Ok(o) if o.exit_code == Some(0) => {
            assert!(
                o.stderr_tail.contains("AttachConsole 成功"),
                "attach 链证据缺失，stderr: {}",
                o.stderr_tail
            );
            eprintln!(
                "[attachconsole] 实验结果：附着放行 + 附着后默认孙进程存活（生产可用顶部 console 链）"
            );
        }
        Ok(o) => {
            eprintln!(
                "[attachconsole] 实验结果：附着链失败（exit={:?}，25=AttachConsole 被拒或附着后仍死）——受限树内 console 只有 DETACHED/继承两条活路，stderr: {}",
                o.exit_code, o.stderr_tail
            );
        }
        Err(e) => {
            eprintln!("[attachconsole] 实验结果：spawn 层失败（{e}）——与附着实验结论同向");
        }
    }
}
