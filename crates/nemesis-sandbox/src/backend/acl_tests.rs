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

/// 子进程：走 `AclBackend::apply_to_self` 完整装配链（标签 + TMP 重定向 +
/// 令牌降级 + Partial/gaps 语义），再做围栏双向断言。
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

    let backend = AclBackend::new();
    let conf = SandboxConf::for_executor(std::path::Path::new(&ws), false);
    match backend.apply_to_self(&conf) {
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
        Ok(Enforcement::Partial(_)) => {}
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
    // 存量 Medium 文件（engage 前父进程建的）→ 写被拒——这是文档化的
    // 「existing-files」缺口（需 label_tree 补救），钉死为已知行为。
    match std::fs::OpenOptions::new().append(true).open(&denied) {
        Ok(_) => {
            eprintln!("存量 Medium 文件应不可写（existing-files 缺口语义变了？）");
            std::process::exit(7);
        }
        Err(e) if e.kind() == PermissionDenied => {}
        Err(e) => {
            eprintln!("预期 PermissionDenied，实际 {e}");
            std::process::exit(7);
        }
    }
    // 工作区外的写仍被拒（围栏主语义）。
    let outside = std::path::Path::new(&denied)
        .parent()
        .expect("denied parent")
        .join("from_child.txt");
    if std::fs::write(&outside, b"escape").is_ok() {
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

    let out = spawn_self_child(
        &[
            ("NEMESIS_P24_WS", dir_ws.path().to_str().expect("ws utf-8")),
            ("NEMESIS_P24_DENIED", denied.to_str().expect("denied utf-8")),
        ],
        concat!(module_path!(), "::", "acl_child_engage_impl"),
    );
    assert_child_ok(&out, "engage 子进程断言");
}
