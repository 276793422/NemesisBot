//! P24（2026-09-25 能力扩展 WS1）Windows ACL 沙盒后端测试。
//!
//! 全平台编译：Windows + `acl` feature 跑真实现（真 API、真令牌、真 ACL、
//! 真 No-Write-Up 围栏）；其余平台跑 stub 契约（Unavailable / Err）。
//! Windows 形态的用例逐个挂 `#[cfg(windows)]`（仓库 Windows 测试标记约定
//! ——Linux 上编译期消失而非运行期跳过）。
//!
//! ⚠ 进程级副作用纪律：**完整性降级不可逆**（同进程升不回去），绝不在
//! 测试进程内直接调 `lower_current_process_integrity` / `apply_to_self`
//! ——会把整个测试二进制的令牌降到 Low，毒化并行的其他测试（tempdir 全
//! 写不了）。deny 生效验证改走两条通路：
//! 1. **DACL deny**（Everyone deny ACE）：进程内安全（可撤销、可恢复）；
//! 2. **完整性围栏**：spawn **自身测试二进制**为子进程（env 哨兵路由到
//!    `*_child_impl` 测试），子进程内降级 + 断言 + 显式退出码——父进程
//!    令牌不受影响，验收「进程内写被拒」为真进程语义。
//!
//! 子进程退出码：0 = 全部断言过；2-9 = 各断言臂失败（见各 impl 文案）。

use super::*;

/// 子进程哨兵 env（值 = 子进程要执行的测试名尾段）。
const CHILD_SENTINEL: &str = "NEMESIS_P24_CHILD";

// ---------------------------------------------------------------------------
// stub 契约（非 Windows 平台 / acl feature 被裁掉的构建）
// ---------------------------------------------------------------------------

/// stub 可用性 = Unavailable（选型决策表据此排除该档）。
#[cfg(not(all(target_os = "windows", feature = "acl")))]
#[test]
fn acl_stub_availability_unavailable() {
    let a = AclBackend::new().availability();
    assert!(
        matches!(a, Availability::Unavailable(ref reason) if !reason.is_empty()),
        "stub 应如实 Unavailable: {a:?}"
    );
}

/// stub 的 apply / 自由函数全部 Err（诚实失败，绝不假装隔离成功）。
#[cfg(not(all(target_os = "windows", feature = "acl")))]
#[test]
fn acl_stub_surface_all_err() {
    let b = AclBackend::new();
    let conf = SandboxConf::for_executor(std::path::Path::new("/tmp/x"), false);
    assert!(b.apply_to_self(&conf).is_err(), "stub apply_to_self 应 Err");
    let p = std::path::Path::new("/tmp/x");
    assert!(set_integrity_label(p, IntegrityLevel::Low).is_err());
    assert!(get_integrity_label(p).is_err());
    assert!(remove_integrity_label(p).is_err());
    assert!(label_tree(p, IntegrityLevel::Low, 10).is_err());
    assert!(current_process_integrity().is_err());
    assert!(lower_current_process_integrity(IntegrityLevel::Low).is_err());
    assert!(add_deny_write_ace(p, "S-1-1-0").is_err());
    assert!(revoke_ace(p, "S-1-1-0").is_err());
}

// ---------------------------------------------------------------------------
// Windows 真实现（acl feature）
// ---------------------------------------------------------------------------

/// Windows 上本档可用（恒非 Unavailable；本机探测到 Partial = 带实验性
/// 缺口标注，属预期）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_availability_on_windows() {
    let a = AclBackend::new().availability();
    assert!(
        !matches!(a, Availability::Unavailable(_)),
        "Windows 上 acl 档不应 Unavailable: {a:?}"
    );
}

/// 完整性标签 API 往返：目录 / 文件都可 set → get → remove（SDDL ↔ SACL
/// ↔ RID 全链走真 API）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_integrity_label_roundtrip_on_dir_and_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        get_integrity_label(dir.path()).expect("query 新目录"),
        None,
        "新 tempdir 应无显式完整性标签"
    );
    set_integrity_label(dir.path(), IntegrityLevel::Low).expect("set Low");
    assert_eq!(
        get_integrity_label(dir.path()).expect("query Low"),
        Some(4096),
        "Low 标签读回 = S-1-16-4096"
    );
    // 文件对象同 API（SE_FILE_OBJECT 覆盖 dir/file 两态）。
    let f = dir.path().join("a.txt");
    std::fs::write(&f, b"x").expect("seed file");
    set_integrity_label(&f, IntegrityLevel::Medium).expect("set Medium");
    assert_eq!(
        get_integrity_label(&f).expect("query Medium"),
        Some(8192),
        "Medium 标签读回 = S-1-16-8192"
    );
    // 移除标签 → 回落默认（None）。
    remove_integrity_label(&f).expect("remove");
    assert_eq!(
        get_integrity_label(&f).expect("query removed"),
        None,
        "移除后无显式标签"
    );
}

/// DACL deny ACE 生效验证（进程内安全通路）：deny Everyone 写 → 本进程
/// （令牌含 Everyone）创建/追加被拒 → revoke 恢复可写。这是「deny 生效、
/// 进程内写被拒」验收的 DACL 臂。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_deny_write_ace_blocks_and_revoke_restores() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pre = dir.path().join("pre.txt");
    std::fs::write(&pre, b"seed").expect("seed pre");
    // 基线：deny 前可写。
    std::fs::write(dir.path().join("baseline.txt"), b"ok").expect("baseline write");

    add_deny_write_ace(dir.path(), "S-1-1-0").expect("加 deny Everyone 写 ACE");

    // 新建被拒（目录上的 GENERIC_WRITE deny 覆盖 FILE_ADD_FILE）。
    let created = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(dir.path().join("blocked.txt"));
    assert!(
        matches!(&created, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "deny 后创建应 PermissionDenied: {created:?}"
    );
    // 存量文件追加被拒（deny ACE OICI 继承到子对象）。
    let appended = std::fs::OpenOptions::new().append(true).open(&pre);
    assert!(
        matches!(&appended, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied),
        "存量文件追加应被继承 deny 拦住: {appended:?}"
    );

    // 撤销 → 恢复可写（tempdir 清理也不受残留 DACL 影响）。
    revoke_ace(dir.path(), "S-1-1-0").expect("撤销 Everyone 的 ACE");
    std::fs::write(dir.path().join("post.txt"), b"ok").expect("revoke 后恢复可写");
}

/// 递归重标：子目录/存量文件都被打上 Low；预算超限诚实 Err。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_label_tree_relabels_existing_entries_and_honors_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).expect("mkdir");
    let f1 = dir.path().join("f1.txt");
    let f2 = sub.join("f2.txt");
    std::fs::write(&f1, b"1").expect("seed f1");
    std::fs::write(&f2, b"2").expect("seed f2");

    let n = label_tree(dir.path(), IntegrityLevel::Low, 100).expect("label_tree");
    // 根 + f1 + sub + f2 = 4。
    assert_eq!(n, 4, "重标对象数（含根）: {n}");
    assert_eq!(get_integrity_label(dir.path()).expect("root"), Some(4096));
    assert_eq!(
        get_integrity_label(&f1).expect("f1"),
        Some(4096),
        "存量文件被重标"
    );
    assert_eq!(get_integrity_label(&f2).expect("f2"), Some(4096));

    // 预算 1 < 实际 4 → Err 且已标部分保持生效（诚实截断语义）。
    let r = label_tree(dir.path(), IntegrityLevel::Low, 1);
    assert!(r.is_err(), "预算超限应 Err");
    assert_eq!(
        get_integrity_label(&f2).expect("f2 after budget"),
        Some(4096),
        "预算耗尽前已标的对象保持生效"
    );
}

/// junction 穿透行为锁定（P24 复检候选 2a 实证固化）：对 junction 打标
/// **不跟随**到目标目录（reparse point 自有 SD）——目标目录与经 junction
/// 新建的子文件都保持无显式标签。若未来 Win32 层行为变化（跟随目标），
/// 此测试先红，防止 label_tree 把工作区内的链接目标（可能指向树外）静默
/// 重标、给树外新建文件开 No-Write-Up 缺口。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_label_on_junction_does_not_follow_to_target() {
    let base = tempfile::tempdir().expect("tempdir");
    let real = base.path().join("real");
    let junc = base.path().join("junc");
    std::fs::create_dir_all(&real).expect("mkdir real");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junc)
        .arg(&real)
        .output()
        .expect("run mklink");
    assert!(
        out.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    set_integrity_label(&junc, IntegrityLevel::Low).expect("label junction");
    assert_eq!(
        get_integrity_label(&junc).expect("query junction"),
        Some(4096),
        "junction 自身 SD 打上 Low（SE_FILE_OBJECT 读写都作用于 reparse point 自身）"
    );
    assert_eq!(
        get_integrity_label(&real).expect("query target dir"),
        None,
        "junction 打标不得跟随到目标目录"
    );
    // 经 junction 新建的文件继承**目标目录**的标签（无标签）→ 不被 Low 波及。
    let via_junc = junc.join("newfile.txt");
    std::fs::write(&via_junc, b"x").expect("write via junction");
    assert_eq!(
        get_integrity_label(&real.join("newfile.txt")).expect("query new file"),
        None,
        "经 junction 新建文件继承目标目录（无标签），未被 junction 的 Low 继承位波及"
    );
}

// ---------------------------------------------------------------------------
// 子进程 harness：完整性围栏的真进程验证
// ---------------------------------------------------------------------------

/// spawn 自身测试二进制跑指定哨兵测试（libtest `--exact` 过滤）。输出带回
/// 给父测试断言用（失败诊断要看子进程 stdout/stderr）。
#[cfg(all(target_os = "windows", feature = "acl"))]
fn spawn_self_child(env: &[(&str, &str)], test_name: &str) -> std::process::Output {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([test_name, "--exact", "--nocapture"])
        .env(CHILD_SENTINEL, test_name);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn 自身测试二进制为子进程")
}

/// 断言子进程退出码 0，非 0 时带 stdout/stderr 全文（失败诊断不用二猜）。
#[cfg(all(target_os = "windows", feature = "acl"))]
fn assert_child_ok(out: &std::process::Output, what: &str) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{what} 失败（code={:?}）\n--- child stdout ---\n{}\n--- child stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 子进程：手工链路（set 标签 + 降令牌 + 双向写断言）。父进程正常跑本
/// 测试时无哨兵 → 立即通过（真正的断言发生在 spawn_self_child 的子进程里）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_child_fence_impl() {
    let Ok(mode) = std::env::var(CHILD_SENTINEL) else {
        return; // 父进程常规跑：空过
    };
    if mode != "acl_child_fence_impl" {
        return;
    }
    use std::io::ErrorKind::PermissionDenied;
    let denied = std::env::var("NEMESIS_P24_DENIED").expect("denied path env");
    let allowed = std::env::var("NEMESIS_P24_ALLOWED").expect("allowed path env");

    if let Err(e) = lower_current_process_integrity(IntegrityLevel::Low) {
        eprintln!("令牌降级失败: {e}");
        std::process::exit(2);
    }
    match current_process_integrity() {
        Ok(4096) => {}
        Ok(other) => {
            eprintln!("降级后 IL 应 4096，实际 {other}");
            std::process::exit(3);
        }
        Err(e) => {
            eprintln!("IL 查询失败: {e}");
            std::process::exit(3);
        }
    }
    // No-Write-Up：Low 令牌写 Medium 对象 → 拒。
    match std::fs::OpenOptions::new().append(true).open(&denied) {
        Ok(_) => {
            eprintln!("围栏失守：Low 令牌写穿了 Medium 对象 {denied}");
            std::process::exit(4);
        }
        Err(e) if e.kind() == PermissionDenied => {} // 预期
        Err(e) => {
            eprintln!("预期 PermissionDenied，实际 {e}");
            std::process::exit(5);
        }
    }
    // 读放行（No-Read-Up 未启用——执行体要读编译器/依赖）。
    if let Err(e) = std::fs::read(&denied) {
        eprintln!("读也被拒（No-Read-Up 意外生效？）: {e}");
        std::process::exit(6);
    }
    // 写 Low 标签对象 → 放行（工作区写能力保留）。
    if let Err(e) = std::fs::write(&allowed, b"child write ok") {
        eprintln!("工作区（Low 标签）写失败: {e}");
        std::process::exit(7);
    }
    std::process::exit(0);
}

/// 验收主测试：父进程备好 Medium 保护目录 + Low 标签工作区，子进程降令牌
/// 后「工作区外写被拒 / 工作区内写放行 / 读不设防」三条全部钉死。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_no_write_up_fence_end_to_end() {
    let dir_protected = tempfile::tempdir().expect("protected tempdir");
    let dir_ws = tempfile::tempdir().expect("workspace tempdir");
    // 工作区先打 Low 标签（OICI），之后父进程（Medium 令牌）新建的文件
    // 继承 Low——「先标目录、再建文件」时序即生产装配时序。
    set_integrity_label(dir_ws.path(), IntegrityLevel::Low).expect("label workspace");
    let allowed = dir_ws.path().join("ok.txt");
    std::fs::write(&allowed, b"seed").expect("seed allowed");
    // 保护目录不打标（默认 Medium）——工作区外敏感路径的替身。
    let denied = dir_protected.path().join("secret.txt");
    std::fs::write(&denied, b"secret").expect("seed denied");

    let out = spawn_self_child(
        &[
            (
                "NEMESIS_P24_DENIED",
                denied.to_str().expect("denied path utf-8"),
            ),
            (
                "NEMESIS_P24_ALLOWED",
                allowed.to_str().expect("allowed path utf-8"),
            ),
        ],
        concat!(module_path!(), "::", "acl_child_fence_impl"),
    );
    assert_child_ok(&out, "完整性围栏子进程断言");
}

/// 子进程：走 `AclBackend::apply_to_self` 完整装配链（标签 + label_tree
/// 存量重标 + TMP 重定向 + 令牌降级 + Partial/gaps 语义），再做围栏双向
/// 断言 + 工作区存量文件可写断言。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_child_engage_impl() {
    let Ok(mode) = std::env::var(CHILD_SENTINEL) else {
        return;
    };
    if mode != "acl_child_engage_impl" {
        return;
    }
    use std::io::ErrorKind::PermissionDenied;
    let ws = std::env::var("NEMESIS_P24_WS").expect("ws env");
    let denied = std::env::var("NEMESIS_P24_DENIED").expect("denied env");
    let preexisting = std::env::var("NEMESIS_P24_PREEXISTING").expect("preexisting env");

    let backend = AclBackend::new();
    let conf = SandboxConf::for_executor(std::path::Path::new(&ws), false);
    let gaps = match backend.apply_to_self(&conf) {
        Err(e) => {
            eprintln!("engage 失败: {e}");
            std::process::exit(2);
        }
        Ok(Enforcement::Full) => {
            eprintln!("engage 意外返回 Full——ACL 档诚实边界丢失（应恒 Partial）");
            std::process::exit(3);
        }
        Ok(Enforcement::Partial(gaps)) if gaps.is_empty() => {
            eprintln!("Partial 但 gaps 为空——诚实标注缺失");
            std::process::exit(3);
        }
        Ok(Enforcement::Partial(gaps)) => gaps,
    };
    // label_tree 接线契约：小工作区远小于预算，存量重标应完成——
    // existing-files 缺口不得在场。
    if let Some(g) = gaps.iter().find(|g| g.starts_with("existing-files")) {
        eprintln!("预算充足时不应出现 existing-files 缺口: {g}");
        std::process::exit(9);
    }
    match current_process_integrity() {
        Ok(4096) => {}
        Ok(other) => {
            eprintln!("engage 后 IL 应 4096，实际 {other}");
            std::process::exit(4);
        }
        Err(e) => {
            eprintln!("IL 查询失败: {e}");
            std::process::exit(4);
        }
    }
    // TMP/TEMP 已重定向进工作区（编译类工具临时文件落点）。
    let tmp = std::env::var("TMP").unwrap_or_default();
    if !tmp.starts_with(&ws) {
        eprintln!("TMP 未重定向进工作区: {tmp:?}");
        std::process::exit(5);
    }
    // 工作区内新建文件（继承 Low）→ 可写。
    let new_file = std::path::Path::new(&ws).join("child_new.txt");
    if let Err(e) = std::fs::write(&new_file, b"engage write ok") {
        eprintln!("工作区新建写失败: {e}");
        std::process::exit(6);
    }
    // 工作区存量文件（engage 前父进程建的）→ label_tree 重标后可写——
    // 这是本接线修复的核心契约（修复前 Medium IL 写被拒）。
    if let Err(e) = std::fs::OpenOptions::new().append(true).open(&preexisting) {
        eprintln!("存量文件重标后应可写（label_tree 接线失效？）: {e}");
        std::process::exit(9);
    }
    // 工作区外的写仍被拒（围栏主语义：denied 在工作区外目录，Medium IL
    // 对象对 Low 令牌 No-Write-Up）。
    let outside = std::path::Path::new(&denied);
    match std::fs::OpenOptions::new().append(true).open(outside) {
        Ok(_) => {
            eprintln!("工作区外 Medium 文件应不可写（No-Write-Up 围栏失守）");
            std::process::exit(7);
        }
        Err(e) if e.kind() == PermissionDenied => {}
        Err(e) => {
            eprintln!("预期 PermissionDenied，实际 {e}");
            std::process::exit(7);
        }
    }
    // 工作区外新建文件同样被拒。
    let outside_new = outside
        .parent()
        .expect("denied parent")
        .join("from_child.txt");
    if std::fs::write(&outside_new, b"escape").is_ok() {
        eprintln!("围栏失守：Low 令牌写穿了工作区外目录");
        std::process::exit(8);
    }
    std::process::exit(0);
}

/// 验收主测试：`apply_to_self` 完整链路的真进程验证（子进程执行，父进程
/// 只断言退出码 0）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_engage_apply_to_self_end_to_end() {
    let dir_protected = tempfile::tempdir().expect("protected tempdir");
    let dir_ws = tempfile::tempdir().expect("workspace tempdir");
    let denied = dir_protected.path().join("existing.txt");
    std::fs::write(&denied, b"existing outside file").expect("seed denied");
    // 工作区存量文件：engage 前创建（Medium IL），验证 label_tree 接线。
    let preexisting = dir_ws.path().join("existing_inside.txt");
    std::fs::write(&preexisting, b"pre-existing inside ws").expect("seed preexisting");

    let out = spawn_self_child(
        &[
            ("NEMESIS_P24_WS", dir_ws.path().to_str().expect("ws utf-8")),
            ("NEMESIS_P24_DENIED", denied.to_str().expect("denied utf-8")),
            (
                "NEMESIS_P24_PREEXISTING",
                preexisting.to_str().expect("preexisting utf-8"),
            ),
        ],
        concat!(module_path!(), "::", "acl_child_engage_impl"),
    );
    assert_child_ok(&out, "engage 子进程断言");
}

/// existing-files gap 文案形态（label_tree 接线的降级路径——预算/IO 失败时
/// 缺口文本要带原因与补救指引，不是裸错误）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_existing_files_gap_message_shape() {
    let g = super::acl_impl::existing_files_gap("label_tree: 预算耗尽（已重标 3 > 上限 2）");
    assert!(g.starts_with("existing-files:"), "前缀契约: {g}");
    assert!(g.contains("预算耗尽"), "原因入列: {g}");
    assert!(g.contains("Medium IL"), "后果说明: {g}");
}

// ---------------------------------------------------------------------------
// DACL 定向档 D2（2026-09-27）：standing GRANT ACE 树
// ---------------------------------------------------------------------------

/// 测试辅助：读对象 DACL，返回**目标 SID** 名下的 (mode, mask) 列表
/// （GRANT_ACCESS=2 / DENY_ACCESS=3，与 windows-sys 常量同值）。
#[cfg(all(target_os = "windows", feature = "acl"))]
fn dacl_aces_for_sid(path: &std::path::Path, sid_str: &str) -> Vec<(i32, u32)> {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACCESS_DENIED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION,
        EqualSid, GetAce, PSECURITY_DESCRIPTOR, PSID,
    };

    let path_w: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let sid_w: Vec<u16> = sid_str.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
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
        assert_eq!(hr, 0, "GetNamedSecurityInfoW 失败: {hr}");
        let mut sid: PSID = std::ptr::null_mut();
        assert_ne!(
            ConvertStringSidToSidW(sid_w.as_ptr(), &mut sid),
            0,
            "ConvertStringSidToSidW 失败"
        );
        let mut out = Vec::new();
        if !dacl.is_null() {
            for i in 0..(*dacl).AceCount {
                let mut pace: *mut c_void = std::ptr::null_mut();
                if GetAce(dacl, i as u32, &mut pace) == 0 || pace.is_null() {
                    continue;
                }
                let hdr = pace as *const ACE_HEADER;
                let (mode, mask, ace_sid) = match (*hdr).AceType {
                    0 => {
                        let a = pace as *const ACCESS_ALLOWED_ACE;
                        (2i32, (*a).Mask, &(*a).SidStart as *const u32 as PSID)
                    }
                    1 => {
                        let a = pace as *const ACCESS_DENIED_ACE;
                        (3i32, (*a).Mask, &(*a).SidStart as *const u32 as PSID)
                    }
                    _ => continue,
                };
                if EqualSid(ace_sid, sid) != 0 {
                    out.push((mode, mask));
                }
            }
        }
        LocalFree(sid as _);
        LocalFree(psd as _);
        out
    }
}

/// 端到端（基线形态，2026-09-28 F9 证伪后的唯一形态）：树遍历后目录/文件
/// 都有 workspace SID 的 GRANT（mask 全覆盖，**含 DELETE**——rename/git/
/// cargo 基线能力；「掐 DELETE 断 rename」的 F9 推演已被真进程证伪）；目录
/// **无** DENY FILE_DELETE_CHILD；幂等重跑稳定；预算超限诚实 Err。旧形态
/// 残留滤除见 [`ensure_grant_ace_tree_filters_stale_ace_shapes`]。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn ensure_grant_ace_tree_idempotent_shape_and_budget() {
    use super::sid::derive_workspace_sid;

    let dir = tempfile::tempdir().expect("tempdir");
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).expect("mkdir");
    let f1 = dir.path().join("f1.txt");
    let f2 = sub.join("f2.txt");
    std::fs::write(&f1, b"1").expect("seed f1");
    std::fs::write(&f2, b"2").expect("seed f2");

    let ws_sid = derive_workspace_sid(dir.path()).expect("derive sid");

    // 首遍：根 + sub + f1 + f2 = 4。
    let n = ensure_grant_ace_tree(dir.path(), &ws_sid, 100).expect("ensure first pass");
    assert_eq!(n, 4, "首遍处理对象数: {n}");

    // 形态读回：目录 = grant（全量 mask 覆盖）且**无 deny**（生产从不打
    // deny——rename/delete 基线能力的 ACE 面）；文件 = grant（无继承要求）。
    for d in [dir.path(), &sub] {
        let aces = dacl_aces_for_sid(d, &ws_sid);
        assert!(
            aces.iter()
                .any(|&(m, mask)| m == 2 && mask & GRANT_MASK == GRANT_MASK),
            "目录 {d:?} 应有全量 grant（含 DELETE）: {aces:?}"
        );
        assert!(
            !aces.iter().any(|&(m, _)| m == 3),
            "目录不应有 deny ACE: {aces:?}"
        );
        assert!(
            aces.iter()
                .any(|&(m, mask)| m == 2 && mask & 0x0001_0000 != 0),
            "grant 必须保含 DELETE 位（F9 证伪锚）: {aces:?}"
        );
    }
    for f in [&f1, &f2] {
        let aces = dacl_aces_for_sid(f, &ws_sid);
        assert!(
            aces.iter()
                .any(|&(m, mask)| m == 2 && mask & GRANT_MASK == GRANT_MASK),
            "文件 {f:?} 应有全量 grant: {aces:?}"
        );
        assert!(
            !aces.iter().any(|&(m, _)| m == 3),
            "文件不应有 deny 子项面 ACE: {aces:?}"
        );
    }

    // 幂等：二遍全部达标跳过，返回值一致、形态不变。
    let n2 = ensure_grant_ace_tree(dir.path(), &ws_sid, 100).expect("ensure second pass");
    assert_eq!(n2, 4, "二遍对象数一致（standing ACE 复用路径）: {n2}");
    // 三遍后 grant 恰好一条、无 deny（无重复叠加——幂等的直接证据）。
    let aces = dacl_aces_for_sid(dir.path(), &ws_sid);
    assert_eq!(
        aces.iter().filter(|&&(m, _)| m == 2).count(),
        1,
        "grant 恰一条: {aces:?}"
    );
    assert_eq!(
        aces.iter().filter(|&&(m, _)| m == 3).count(),
        0,
        "deny 零条: {aces:?}"
    );

    // 预算：1 < 4 → Err（诚实截断，已打部分保持生效）。
    let r = ensure_grant_ace_tree(dir.path(), &ws_sid, 1);
    assert!(r.is_err(), "预算超限应 Err: {r:?}");

    // 清理：撤销整树 ACE（tempdir 删除不受残留 ACE 影响——grant/deny 只对
    // workspace SID 生效，tempdir 清理走用户令牌）。
    super::revoke_ace(dir.path(), &ws_sid).expect("revoke root");
    super::revoke_ace(&sub, &ws_sid).expect("revoke sub");
    super::revoke_ace(&f1, &ws_sid).expect("revoke f1");
    super::revoke_ace(&f2, &ws_sid).expect("revoke f2");
}

/// 旧形态残留滤除（stale_ws_ace 语义钉）：目标 SID 的现存 deny 项（只可能
/// 来自外部篡改或已证伪的旧加固档残留）在 ensure 遍历中被当作 stale 强制
/// 重建——deny 滤除、grant 重写为含 DELETE 全量形态。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn ensure_grant_ace_tree_filters_stale_ace_shapes() {
    use super::sid::derive_workspace_sid;

    const FILE_DELETE_CHILD: u32 = 0x40;
    const DELETE: u32 = 0x0001_0000;

    let dir = tempfile::tempdir().expect("tempdir");
    let f1 = dir.path().join("f1.txt");
    std::fs::write(&f1, b"1").expect("seed f1");
    let ws_sid = derive_workspace_sid(dir.path()).expect("derive sid");

    // ① 铺 deny 残留（外部篡改/旧加固档等价形态）。
    super::add_deny_write_ace(&f1, &ws_sid).expect("seed stale deny");

    // ② ensure 遍历：deny 残留 → stale → 确定性重建（deny 滤除 + 全量
    //    grant 落位）。
    ensure_grant_ace_tree(dir.path(), &ws_sid, 100).expect("ensure");
    let aces = dacl_aces_for_sid(&f1, &ws_sid);
    assert!(
        !aces.iter().any(|&(m, _)| m == 3),
        "deny 残留必须滤除: {aces:?}"
    );
    assert!(
        aces.iter().any(|&(m, mask)| m == 2 && mask & DELETE != 0),
        "重建 grant 必须含 DELETE（基线全量形态）: {aces:?}"
    );
    assert!(
        aces.iter()
            .any(|&(m, mask)| m == 2 && mask & GRANT_MASK == GRANT_MASK),
        "重建 grant 覆盖全量 mask: {aces:?}"
    );
    // deny 面 FILE_DELETE_CHILD 形态（加固档目录专属）在生产 ensure 下
    // 不应存在（单文件对象无从打起，顺带钉死）。
    assert!(
        !aces.iter().any(|&(m, mask)| m == 3 && mask & FILE_DELETE_CHILD != 0),
        "不得有任何 deny 残留: {aces:?}"
    );

    // 清理。
    super::revoke_ace(dir.path(), &ws_sid).expect("revoke root");
    super::revoke_ace(&f1, &ws_sid).expect("revoke f1");
}

/// junction DACL 跟随性**第一件事实证**（设计文档 §3.2 要求实现期首做）：
/// 给 junction 打 DACL ACE 只作用于 reparse point 自身、**不跟随**目标目录
/// （与完整性标签侧 `acl_label_on_junction_does_not_follow_to_target` 同向
/// 的行为记录）。产品实现（ensure_grant_ace_tree）对链接项保守跳过——本测
/// 试钉的是 Win32 层事实：即使有人改成全 entry 打点，junction 也不会把
/// workspace SID 的 ACE 泄漏到树外目标。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn acl_dacl_ace_on_junction_does_not_follow_to_target() {
    let base = tempfile::tempdir().expect("tempdir");
    let real = base.path().join("real");
    let junc = base.path().join("junc");
    std::fs::create_dir_all(&real).expect("mkdir real");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junc)
        .arg(&real)
        .output()
        .expect("run mklink");
    assert!(
        out.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let probe_sid = "S-1-5-21-424242-424242-424242"; // 假 workspace SID（无消费方）
    add_deny_write_ace(&junc, probe_sid).expect("deny ACE 打到 junction 自身");
    assert!(
        dacl_aces_for_sid(&real, probe_sid).is_empty(),
        "junction 打 ACE 不得跟随到目标目录"
    );
    assert!(
        !dacl_aces_for_sid(&junc, probe_sid).is_empty(),
        "junction 自身 SD 应带 ACE"
    );
    revoke_ace(&junc, probe_sid).expect("清理 junction ACE");
    assert!(
        dacl_aces_for_sid(&junc, probe_sid).is_empty(),
        "revoke 后 junction 无残留"
    );
}

/// 树内 junction：ensure 遍历跳过链接项（不 Err、不跟随），树内其余对象照常
/// 落位——jump 指向树内已有 ACE 的目录也不重打（幂等）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn ensure_grant_ace_tree_skips_junction_entries() {
    use super::sid::derive_workspace_sid;

    let dir = tempfile::tempdir().expect("tempdir");
    let real = dir.path().join("real");
    let junc = dir.path().join("junc");
    std::fs::create_dir_all(&real).expect("mkdir real");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junc)
        .arg(&real)
        .output()
        .expect("run mklink");
    assert!(out.status.success(), "mklink /J failed");

    let ws_sid = derive_workspace_sid(dir.path()).expect("derive sid");
    // 根 + real + junc（跳过不计）= 2。
    let n = ensure_grant_ace_tree(dir.path(), &ws_sid, 100).expect("ensure");
    assert_eq!(n, 2, "junction 项不入计数: {n}");
    let aces = dacl_aces_for_sid(&real, &ws_sid);
    assert!(
        aces.iter()
            .any(|&(m, mask)| m == 2 && mask & GRANT_MASK == GRANT_MASK),
        "树内真实子目录应有 grant: {aces:?}"
    );

    super::revoke_ace(dir.path(), &ws_sid).expect("revoke root");
    super::revoke_ace(&real, &ws_sid).expect("revoke real");
}

/// D4 状态面（2026-09-27；2026-09-28 基线单口径）：只读根 ACE 探针三态——
/// 未打标 (false,false,true) / 打标 (true,false,true)（grant 覆盖
/// [`GRANT_MASK`] 含 DELETE、无 deny 面）/ 撤销后全 false。grant 判定与
/// 铺设幂等同判据；deny 面纯观测（生产从不打，现存即外部篡改痕迹）。
/// 同时钉「零副作用」契约：探针调用前后 DACL 形态不变（不打标不写回）。
#[cfg(all(target_os = "windows", feature = "acl"))]
#[test]
fn root_standing_ace_state_readonly_probe_states() {
    use super::sid::derive_workspace_sid;

    let dir = tempfile::tempdir().expect("tempdir");
    let ws_sid = derive_workspace_sid(dir.path()).expect("derive sid");

    // 未打标：grant/deny 都 false；目录 = true。
    let before = super::root_standing_ace_state(dir.path(), &ws_sid).expect("probe before");
    assert_eq!(before, (false, false, true), "未打标探针读数: {before:?}");

    // 打标根目录：grant 达标（含 DELETE）、无 deny 面。
    let n = ensure_grant_ace_tree(dir.path(), &ws_sid, 100).expect("ensure");
    assert!(n >= 1);
    let stamped =
        super::root_standing_ace_state(dir.path(), &ws_sid).expect("probe stamped");
    assert_eq!(stamped, (true, false, true), "打标后探针读数: {stamped:?}");

    // 零副作用：探针不改变 DACL（grant 恰一条、无 deny）。
    let aces_after_probe = dacl_aces_for_sid(dir.path(), &ws_sid);
    assert_eq!(
        aces_after_probe.iter().filter(|&&(m, _)| m == 2).count(),
        1,
        "grant 恰一条: {aces_after_probe:?}"
    );
    assert!(
        !aces_after_probe.iter().any(|&(m, _)| m == 3),
        "无 deny: {aces_after_probe:?}"
    );

    // grant 必须含 DELETE（基线全量形态的探针面证据——F9 证伪锚）。
    assert!(
        aces_after_probe
            .iter()
            .any(|&(m, mask)| m == 2 && mask & 0x0001_0000 != 0),
        "打标 grant 应含 DELETE 位: {aces_after_probe:?}"
    );

    // 单撤整面（revoke_ace 撤全部该 SID 项——验证「探针反映真实 DACL」
    // 而非缓存：撤掉后 grant 读 false）。
    super::revoke_ace(dir.path(), &ws_sid).expect("revoke root");
    let revoked = super::root_standing_ace_state(dir.path(), &ws_sid).expect("probe revoked");
    assert_eq!(revoked, (false, false, true), "撤销后探针读数: {revoked:?}");

    // 文件对象：is_dir=false（文件无 deny 子项面，探针恒 false）。
    let f = dir.path().join("f.txt");
    std::fs::write(&f, b"x").expect("seed f");
    ensure_grant_ace_tree(&f, &ws_sid, 100).expect_err("单文件根不是目录 → Err");
    let (grant, deny_child, is_dir) =
        super::root_standing_ace_state(&f, &ws_sid).expect("probe file");
    assert!(!is_dir, "文件对象 is_dir=false");
    assert!(!grant, "未打标文件 grant=false");
    assert!(!deny_child, "文件对象 deny_child 恒 false（无子项面）");
}
