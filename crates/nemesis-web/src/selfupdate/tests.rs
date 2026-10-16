//! L9 selfupdate 单测：journal/fuse 往返、commit 身份提取、boot 收口相位
//! 矩阵、PID 存活探测；`selfupdate` feature 下追加 zip 解包、apply 前置闸
//! （estop/journal/fuse——全部先于网络下载，可离线测）与 **R1 签名更新
//! 清单**套件（验签解析往返/篡改/字段校验/摘要绑定/applied 副本写入读回/
//! sequence 顺序闸矩阵）。审批/exe 本体验签的在线路径归集成验证（见
//! docs/REPORT 交付记录的诚实边界节）。

use super::*;
use crate::handlers::release_fetch::{parse_release_list, resolve_release_for_asset};
use std::path::PathBuf;

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "nemesis-selfupdate-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("create tmp dir");
    d
}

fn journal(phase: &str, target: &str, old: &str) -> UpdateJournal {
    UpdateJournal {
        phase: phase.to_string(),
        target_commit: target.to_string(),
        target_version: "v-test".to_string(),
        old_path: old.to_string(),
        started_ms: now_ms(),
        target_manifest: String::new(),
    }
}

// ---------------------------------------------------------------------------
// journal / fuse
// ---------------------------------------------------------------------------

#[test]
fn journal_roundtrip_and_clear() {
    let ws = tmp_dir("journal");
    assert!(load_journal(&ws).is_none(), "无 journal = None");
    let j = journal("staging", "abc123def456", "C:\\x\\nemesisbot.old");
    save_journal(&ws, &j).expect("save");
    let loaded = load_journal(&ws).expect("loaded");
    assert_eq!(loaded.phase, "staging");
    assert_eq!(loaded.target_commit, "abc123def456");
    assert_eq!(loaded.old_path, "C:\\x\\nemesisbot.old");
    // 原子写不留 tmp 残留（write_file_atomic 成功路径把唯一临时名 rename 走，
    // state 目录里只剩 journal 本体）。
    let names: Vec<String> = std::fs::read_dir(journal_path(&ws).parent().expect("state dir"))
        .expect("read state dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["selfupdate.json".to_string()],
        "原子写成功后 state 目录无 tmp 残留"
    );
    assert!(clear_journal(&ws), "clear 成功");
    assert!(load_journal(&ws).is_none(), "clear 后 = None");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn journal_corrupt_json_reads_none() {
    let ws = tmp_dir("journal-bad");
    std::fs::create_dir_all(ws.join("state")).unwrap();
    std::fs::write(journal_path(&ws), "{not json").unwrap();
    assert!(
        load_journal(&ws).is_none(),
        "坏 JSON = None（boot 收口当无事）"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn fuse_stamp_and_read() {
    let ws = tmp_dir("fuse");
    assert!(last_attempt_ms(&ws).is_none());
    stamp_fuse(&ws).expect("fuse stamp ok");
    let last = last_attempt_ms(&ws).expect("stamped");
    assert!(now_ms() >= last && now_ms() - last < 60_000, "fuse ≈ now");
    let _ = std::fs::remove_dir_all(&ws);
}

/// fail-closed 前提：state 目录不可写（存在同名常规文件）时记账必须报错
/// 而不是吞掉——apply 审批后的 fail-closed 闸依赖这个 Err。
#[test]
fn stamp_fuse_fail_closed_when_state_unwritable() {
    let ws = tmp_dir("fuse-fail");
    std::fs::write(ws.join("state"), b"x").expect("make state a file");
    assert!(
        stamp_fuse(&ws).is_err(),
        "state 不可写 = Err（fail-closed 前提）"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// 同理：journal 原子写在 state 目录不可写时诚实报错（不静默丢现场）。
#[test]
fn save_journal_err_when_state_unwritable() {
    let ws = tmp_dir("journal-fail");
    std::fs::write(ws.join("state"), b"x").expect("make state a file");
    assert!(
        save_journal(&ws, &journal("staging", "x", "y")).is_err(),
        "journal 落盘失败必须上抛"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// commit 身份
// ---------------------------------------------------------------------------

#[test]
fn commit_matches_prefix_and_empty_semantics() {
    let full = "0123456789abcdef0123456789abcdef01234567";
    assert!(commit_matches("0123456", full));
    assert!(commit_matches(full, full));
    assert!(!commit_matches("9999999", full));
    assert!(!commit_matches("", full), "当前未知 = 不命中");
    assert!(!commit_matches("0123456", ""), "目标未知 = 不命中");
}

#[test]
fn version_txt_commit_extraction() {
    let real = "NemesisBot Nightly Build\n========================\nBuild Date : 2026-10-13 00:00:00 UTC\nCommit     : 0123456789ABCDEF0123456789ABCDEF01234567\nBranch     : main\n";
    assert_eq!(
        extract_commit_from_version_txt(real).as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567"),
        "真实模板命中（大小写归一）"
    );
    // 「Commit Date」行不误命中（冒号前多一段）。
    let date_only = "Commit Date : 2026-10-13 00:00:00 UTC\n";
    assert_eq!(extract_commit_from_version_txt(date_only), None);
    assert_eq!(extract_commit_from_version_txt("no commit here"), None);
}

#[test]
fn release_body_commit_extraction() {
    let body = "## 📦 Nightly Build\n\n| 项 | 值 |\n|---|---|\n| Commit | `fedcba9876543210fedcba9876543210fedcba98` |\n| Branch | `main` |\n";
    assert_eq!(
        extract_commit_from_release_body(body).as_deref(),
        Some("fedcba9876543210fedcba9876543210fedcba98")
    );
    assert_eq!(extract_commit_from_release_body("no table"), None);
    // 非全量 40 hex 的反引号段不命中。
    let short = "| Commit | `abc123` |\n";
    assert_eq!(extract_commit_from_release_body(short), None);
}

// ---------------------------------------------------------------------------
// 发布列表同源解析（check / apply 单一权威源）
// ---------------------------------------------------------------------------

/// 复评整改 B1b 回归锁：首条 release 无主包资产、更旧的第二条有 →
/// resolve 落在第二条，meta（tag/body commit/下载 URL）同源——check 展示
/// 与 apply 实际下载从此指向同一对象。
#[test]
fn release_list_resolution_same_source() {
    let json = serde_json::json!([
        {
            "tag_name": "nightly-build",
            "name": "Nightly（主包未产出）",
            "body": "| Commit | `aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa` |",
            "assets": [
                { "name": "nightly-windows-plugins.zip",
                  "browser_download_url": "https://example.com/plugins.zip" }
            ]
        },
        {
            "tag_name": "nightly-build",
            "name": "Nightly（含主包）",
            "body": "| Commit | `bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb` |",
            "assets": [
                { "name": "nightly-windows.zip",
                  "browser_download_url": "https://example.com/pkg.zip" },
                { "name": "nightly-linux.tar.gz",
                  "browser_download_url": "https://example.com/pkg.tar.gz" }
            ]
        }
    ]);
    let list = parse_release_list(&json).expect("parse ok");
    assert_eq!(list.len(), 2, "顺序保留（新→旧）");

    let resolved =
        resolve_release_for_asset(&list, "nightly-windows.zip").expect("落在含资产的第二条");
    assert_eq!(
        resolved.body_commit().as_deref(),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        "body commit 来自含资产的 release（非列表首条）"
    );
    assert_eq!(
        resolved.asset_url("nightly-windows.zip"),
        Some("https://example.com/pkg.zip"),
        "下载 URL 同源"
    );
    assert!(resolved.has_asset("nightly-windows.zip"));

    assert!(
        resolve_release_for_asset(&list, "nightly-missing.zip").is_none(),
        "无资产命中 = None（调用方诚实回落）"
    );
}

#[test]
fn parse_release_list_rejects_non_array() {
    let err = parse_release_list(&serde_json::json!({"message": "Not Found"}))
        .expect_err("非列表响应必须报错");
    assert!(err.contains("不是列表"), "诚实报错: {err}");
}

// ---------------------------------------------------------------------------
// boot 收口相位矩阵
// ---------------------------------------------------------------------------

#[test]
fn boot_recovery_no_journal_is_none() {
    let ws = tmp_dir("boot-none");
    let r = recover_on_boot_in(&ws, "0123456", std::path::Path::new("unused.exe"));
    assert_eq!(r.action, "none");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn boot_recovery_restarting_commit_hit_success() {
    let ws = tmp_dir("boot-hit");
    let full = "0123456789abcdef0123456789abcdef01234567";
    save_journal(&ws, &journal("restarting", full, "nowhere.old")).unwrap();
    let r = recover_on_boot_in(&ws, "0123456", std::path::Path::new("unused.exe"));
    assert_eq!(
        r.action, "update_success",
        "命中目标 = 收口成功: {:?}",
        r.detail
    );
    assert!(load_journal(&ws).is_none(), "成功后 journal 清除");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn boot_recovery_restarting_mismatch_rolls_back() {
    let ws = tmp_dir("boot-rollback");
    // .old 现场文件 + 假 exe 目标（不碰测试二进制）。
    let old = ws.join("nemesisbot.exe.old");
    std::fs::write(&old, b"old-bytes").unwrap();
    let target_exe = ws.join("current.exe");
    std::fs::write(&target_exe, b"broken-new").unwrap();
    save_journal(
        &ws,
        &journal(
            "restarting",
            "ffffffffffffffffffffffffffffffffffffffff",
            &old.display().to_string(),
        ),
    )
    .unwrap();

    let r = recover_on_boot_in(&ws, "0123456", &target_exe);
    assert_eq!(r.action, "rolled_back");
    assert!(load_journal(&ws).is_none(), "回滚后 journal 清除");
    // .old 已归位到 target（内容 = old-bytes）。
    let restored = std::fs::read(&target_exe).unwrap();
    assert_eq!(restored, b"old-bytes", "回滚后 target 内容 = .old 内容");
    assert!(!old.exists(), ".old 消失（已 rename 走）");
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn boot_recovery_staging_phase_rolls_back() {
    let ws = tmp_dir("boot-staging");
    let old = ws.join("nemesisbot.exe.old");
    std::fs::write(&old, b"old").unwrap();
    let target_exe = ws.join("current.exe");
    std::fs::write(&target_exe, b"partial").unwrap();
    save_journal(&ws, &journal("staging", "ff00", &old.display().to_string())).unwrap();
    let r = recover_on_boot_in(&ws, "anything", &target_exe);
    assert_eq!(r.action, "rolled_back");
    assert_eq!(std::fs::read(&target_exe).unwrap(), b"old");
    assert!(load_journal(&ws).is_none());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn boot_recovery_unknown_phase_cleared() {
    let ws = tmp_dir("boot-unknown");
    save_journal(&ws, &journal("bogus", "x", "y")).unwrap();
    let r = recover_on_boot_in(&ws, "0123456", std::path::Path::new("unused.exe"));
    assert_eq!(r.action, "rolled_back");
    assert!(r.detail.contains("unknown phase"));
    assert!(load_journal(&ws).is_none());
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn rollback_missing_old_is_noop_message() {
    let msg = rollback_old_exe_to("definitely-not-there.old", std::path::Path::new("t.exe"))
        .expect("缺 .old = Ok（无需回滚）");
    assert!(msg.contains("不在场"));
    assert!(
        msg.starts_with("rollback_noop"),
        "noop 分支必须带可识别前缀供调用方分臂：{msg}"
    );
}

/// R2 复查补口：restarting + commit 不匹配 + `.old` 缺失 = 无可恢复现场的
/// noop——journal 照常清（重试修不回来，留着只会闸死后续 apply），但 action
/// 必须是 rollback_noop 而非失实的 rolled_back。
#[test]
fn boot_recovery_missing_old_is_noop_not_rolled_back() {
    let ws = tmp_dir("boot-noop");
    let target_exe = ws.join("current.exe");
    std::fs::write(&target_exe, b"whatever").unwrap();
    save_journal(
        &ws,
        &journal(
            "restarting",
            "ffffffffffffffffffffffffffffffffffffffff",
            &ws.join("nope.old").display().to_string(),
        ),
    )
    .unwrap();

    let r = recover_on_boot_in(&ws, "0123456", &target_exe);
    assert_eq!(r.action, "rollback_noop", "没回滚就不能宣称 rolled_back");
    assert!(r.detail.contains("不在场"));
    assert!(
        load_journal(&ws).is_none(),
        "journal 照常清理（重试无意义）"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// R2 复查补口：staging 名去固定化（PID 后缀）移除了旧固定名方案的隐式
/// 回收，apply 持锁窗口的显式清扫必须删掉历史 `.old-{pid}`/`.new-{pid}`
/// 残留；`.failed` 是钦定诊断现场，不清。
#[cfg(feature = "selfupdate")]
#[test]
fn sweep_stale_staging_removes_pid_suffixed_leaves_failed() {
    let ws = tmp_dir("sweep-staging");
    let exe = ws.join("nemesisbot.exe");
    std::fs::write(&exe, b"running").unwrap();
    // 文件名与生产同源：staging_paths 本体生成（with_extension 是**替换**
    // 扩展位——`nemesisbot.exe` → `nemesisbot.old-{pid}`，不是 exe.old-）。
    let (old_path, new_path) = staging_paths(&exe);
    let stale = [
        ws.join(format!(
            "nemesisbot.old-{}",
            std::process::id() + 1 // 模拟他进程 PID 残留（避开本进程真名）
        )),
        old_path,
        new_path,
    ];
    for f in &stale {
        std::fs::write(f, b"stale").unwrap();
    }
    let failed = exe.with_extension("failed");
    std::fs::write(&failed, b"diagnostic").unwrap();

    let removed = sweep_stale_staging(&exe);
    assert_eq!(removed, 3, "三个历史 staging 残留全部清掉");
    for f in &stale {
        assert!(!f.exists(), "{} 应已删除", f.display());
    }
    assert!(failed.exists(), ".failed 诊断现场不得清扫");
    assert!(exe.exists(), "本体不得清扫");
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// R2：回滚诚实化（失败保留现场）/ 单飞 / 跨进程锁 / staging 名去固定化
// ---------------------------------------------------------------------------

/// R2 主断言（boot restarting 相位）：回滚执行失败 → action=recovery_failed、
/// journal **保留**（下次 boot 重试）、目标 exe 内容不被破坏。
/// 失败注入（Windows）：.old 被本进程以 share_mode=0 独占打开 → rename 报
/// 占用。Windows 舞步下运行中 exe（target）先让位 .failed，第二步 .old→target
/// 撞锁失败后 .failed 回手——target 最终保持原内容。
#[cfg(windows)]
#[test]
fn rollback_failure_keeps_journal_and_reports_recovery_failed() {
    use std::os::windows::fs::OpenOptionsExt;
    let ws = tmp_dir("boot-rollback-fail");
    let old = ws.join("nemesisbot.exe.old");
    std::fs::write(&old, b"old-bytes").unwrap();
    let target_exe = ws.join("current.exe");
    std::fs::write(&target_exe, b"broken-new").unwrap();
    save_journal(
        &ws,
        &journal(
            "restarting",
            "ffffffffffffffffffffffffffffffffffffffff",
            &old.display().to_string(),
        ),
    )
    .unwrap();
    // 独占 .old（无共享）→ rename(.old→target) 必败。
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&old)
        .expect("open old with share_mode=0");

    let r = recover_on_boot_in(&ws, "0123456", &target_exe);
    assert_eq!(
        r.action, "recovery_failed",
        "回滚失败要诚实上报: {:?}",
        r.detail
    );
    assert!(
        load_journal(&ws).is_some(),
        "回滚失败 = journal 保留（下次 boot 重试）"
    );
    assert_eq!(
        std::fs::read(&target_exe).unwrap(),
        b"broken-new",
        "目标位内容不变（.failed 回手成功）"
    );
    assert!(old.exists(), ".old 现场保留");
    let _ = std::fs::remove_dir_all(&ws);
}

/// R2 主断言（boot staging 相位）：回滚失败同样 recovery_failed + journal 保留。
#[cfg(windows)]
#[test]
fn boot_recovery_staging_rollback_failure_keeps_journal() {
    use std::os::windows::fs::OpenOptionsExt;
    let ws = tmp_dir("boot-staging-fail");
    let old = ws.join("nemesisbot.exe.old");
    std::fs::write(&old, b"old").unwrap();
    let target_exe = ws.join("current.exe");
    std::fs::write(&target_exe, b"partial").unwrap();
    save_journal(&ws, &journal("staging", "ff00", &old.display().to_string())).unwrap();
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&old)
        .expect("lock old");

    let r = recover_on_boot_in(&ws, "anything", &target_exe);
    assert_eq!(r.action, "recovery_failed");
    assert!(load_journal(&ws).is_some(), "staging 回滚失败 journal 保留");
    let _ = std::fs::remove_dir_all(&ws);
}

/// Windows 舞步正例（boot restarting 相位，目标被占用态由测试文件模拟）：
/// 回滚成功 = target 内容为 .old、.old 消失、被替换的 exe 留 .failed 诊断现场、
/// journal 清除。
#[test]
fn boot_recovery_rollback_windows_dance_leaves_failed_diagnostics() {
    let ws = tmp_dir("boot-dance");
    let old = ws.join("nemesisbot.exe.old");
    std::fs::write(&old, b"old-bytes").unwrap();
    let target_exe = ws.join("current.exe");
    std::fs::write(&target_exe, b"broken-new").unwrap();
    save_journal(
        &ws,
        &journal(
            "restarting",
            "ffffffffffffffffffffffffffffffffffffffff",
            &old.display().to_string(),
        ),
    )
    .unwrap();

    let r = recover_on_boot_in(&ws, "0123456", &target_exe);
    assert_eq!(r.action, "rolled_back");
    assert_eq!(std::fs::read(&target_exe).unwrap(), b"old-bytes");
    assert!(!old.exists());
    assert!(load_journal(&ws).is_none());
    #[cfg(windows)]
    {
        let failed = target_exe.with_extension("failed");
        assert!(
            failed.exists() && std::fs::read(&failed).unwrap() == b"broken-new",
            "被替换的 exe 留 .failed 诊断现场"
        );
    }
    let _ = std::fs::remove_dir_all(&ws);
}

/// staging 名 PID 后缀去固定化：两名字都带 pid、互不相同、扩展位形态正确。
#[cfg(feature = "selfupdate")]
#[test]
fn staging_paths_are_pid_suffixed_and_distinct() {
    let exe = std::path::Path::new("C:\\install\\nemesisbot.exe");
    let (old, new) = staging_paths(exe);
    let pid = std::process::id();
    let old_name = old.file_name().unwrap().to_string_lossy();
    let new_name = new.file_name().unwrap().to_string_lossy();
    assert!(
        old_name.contains(&format!("old-{pid}")),
        "old 名带 PID: {old_name}"
    );
    assert!(
        new_name.contains(&format!("new-{pid}")),
        "new 名带 PID: {new_name}"
    );
    assert_ne!(old, new);
    assert_eq!(old.parent(), exe.parent(), "staging 名与 exe 同目录");
}

/// 安装目录跨进程锁：第一次获取成功；持锁期间第二次获取诚实拒绝；释放后
/// 可再获取（OS 文件锁语义）。
#[cfg(feature = "selfupdate")]
#[test]
fn install_lock_serializes_holders() {
    let dir = tmp_dir("install-lock");
    let first = acquire_install_lock(&dir).expect("首次获取成功");
    let err = acquire_install_lock(&dir).unwrap_err();
    assert!(err.contains("另一进程正在自更新"), "持锁期间拒绝: {err}");
    drop(first);
    let again = acquire_install_lock(&dir).expect("释放后可再获取");
    drop(again);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// PID 存活探测
// ---------------------------------------------------------------------------

#[test]
fn wait_for_pid_exit_returns_fast_for_dead_pid() {
    // Windows：u32::MAX 不可能是真实 PID——tasklist 探不到 → 判死。
    // Linux：procps 4 的 /usr/bin/kill 对超 pid_t 域的值反而 rc=0
    // （判活假象，CI 实录 2026-10-14；bash builtin rc=1 但 Command 找的
    // 是 PATH 上的二进制）——改用 ns_last_pid+N 构造「合法域内但当前
    // 未分配」的死 PID；procfs 不可读则诚实跳过（环境探针纪律）。
    #[cfg(windows)]
    let dead_pid = u32::MAX;
    #[cfg(not(windows))]
    let dead_pid = match std::fs::read_to_string("/proc/sys/kernel/ns_last_pid")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        Some(last) => last.saturating_add(1000),
        None => {
            eprintln!("skip: ns_last_pid unavailable, cannot construct in-range dead pid");
            return;
        }
    };
    assert!(wait_for_pid_exit(dead_pid, 5_000), "死 PID 立即判退场");
}

#[test]
fn wait_for_pid_exit_timeout_for_self() {
    // 自身 PID 恒存活 → 等到超时返回 false（1s 上限控制测试时长）。
    assert!(!wait_for_pid_exit(std::process::id(), 1_000));
}

// ---------------------------------------------------------------------------
// feature=selfupdate：zip 解包 + apply 前置闸（离线可达的三闸）
// ---------------------------------------------------------------------------

#[cfg(feature = "selfupdate")]
mod gated {
    use super::*;
    use crate::selfupdate::SelfUpdateManager;

    fn zip_bytes(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::SimpleFileOptions = Default::default();
            for (name, data) in entries {
                w.start_file(name, opts).unwrap();
                std::io::Write::write_all(&mut w, &data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extract_bundle_reads_exe_and_commit() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        let vt = format!("NemesisBot\nCommit     : {full}\n");
        let bytes = zip_bytes(vec![
            ("nemesisbot.exe", b"pe-bytes".to_vec()),
            ("version.txt", vt.into_bytes()),
            ("plugins/x.dll", b"plug".to_vec()),
        ]);
        let bundle = super::super::extract_update_bundle(&bytes).expect("extract");
        assert_eq!(bundle.exe_bytes, b"pe-bytes");
        assert_eq!(bundle.commit.as_deref(), Some(full));
    }

    #[test]
    fn extract_bundle_missing_exe_rejected() {
        let bytes = zip_bytes(vec![("version.txt", b"Commit : 0123456".to_vec())]);
        let err = super::super::extract_update_bundle(&bytes).unwrap_err();
        assert!(err.contains("nemesisbot.exe"), "诚实报包结构异常: {err}");
    }

    // -----------------------------------------------------------------------
    // R1：sequence 顺序闸（签名清单权威，列表位次判定退役）
    // -----------------------------------------------------------------------

    use crate::selfupdate::{DowngradeVerdict, downgrade_gate_error, sequence_verdict};

    /// sequence 数值比较判定矩阵：None（applied 基准缺失）= Unknown；
    /// 数值比较三态。列表位次版（downgrade_verdict）已退役。
    #[test]
    fn sequence_verdict_matrix() {
        assert_eq!(
            sequence_verdict(None, 42),
            DowngradeVerdict::Unknown,
            "无 applied 基准 = Unknown（存量安装首次更新）"
        );
        assert_eq!(
            sequence_verdict(Some(41), 42),
            DowngradeVerdict::Upgrade,
            "目标序号更新 = 升级"
        );
        assert_eq!(sequence_verdict(Some(42), 42), DowngradeVerdict::Same);
        assert_eq!(
            sequence_verdict(Some(43), 42),
            DowngradeVerdict::Downgrade,
            "目标序号更旧 = 降级"
        );
    }

    /// 闸语义（R1 语义反转）：Downgrade 与 **Unknown** 未显式豁免都拒；
    /// allow_downgrade 豁免两者；Upgrade 恒放行；Same 恒拒（防御性——
    /// 同版应在更早的同版闸短路，漏到此处 = 异常形态，诚实拒绝）。
    #[test]
    fn downgrade_gate_error_matrix() {
        assert!(
            downgrade_gate_error(DowngradeVerdict::Downgrade, false).is_some(),
            "降级未豁免 = 拒"
        );
        assert!(
            downgrade_gate_error(DowngradeVerdict::Downgrade, true).is_none(),
            "显式豁免 = 放行"
        );
        assert!(
            downgrade_gate_error(DowngradeVerdict::Unknown, false).is_some(),
            "顺序未知未豁免 = 拒（R1 语义反转主断言）"
        );
        assert!(
            downgrade_gate_error(DowngradeVerdict::Unknown, true).is_none(),
            "顺序未知显式豁免 = 放行"
        );
        assert!(
            downgrade_gate_error(DowngradeVerdict::Same, false).is_some()
                && downgrade_gate_error(DowngradeVerdict::Same, true).is_some(),
            "Same 恒拒（不随豁免放开）"
        );
        assert!(
            downgrade_gate_error(DowngradeVerdict::Upgrade, false).is_none(),
            "升级恒放行"
        );
    }

    // -----------------------------------------------------------------------
    // R1：签名更新清单（验签解析 / 完整性绑定 / applied 副本链路）
    // -----------------------------------------------------------------------

    /// 测试签名锚：keygen 三级链 + 根证书指纹（与 builtin_root_anchors 无关，
    /// 免环境注入——boot 端到端不测的原因，见模块头诚实边界）。
    fn sign_manifest(json: &serde_json::Value, now: u64) -> (Vec<u8>, Vec<[u8; 32]>) {
        let kh = nemesis_verify::keygen::generate().expect("keygen");
        let body = serde_json::to_vec(json).expect("serialize manifest json");
        let carrier =
            nemesis_verify::verify::sign_content_v4(&body, &kh.leaf_sk, now, &kh.chain(), None)
                .expect("sign manifest carrier");
        (carrier, vec![kh.root_anchor_fingerprint()])
    }

    /// 当前 unix 秒（证书按当前时刻铸，验签时刻必须落在有效窗内——固定历史
    /// 时间戳会撞 not-before = Expired 误报）。
    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn sample_manifest_json() -> serde_json::Value {
        serde_json::json!({
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "sequence": 42,
            "exe_sha256": format!("{:064x}", 0xdead_beefu64),
            "built_at": "2026-10-16T00:00:00Z"
        })
    }

    #[test]
    fn manifest_verify_parse_roundtrip() {
        let (carrier, anchors) = sign_manifest(&sample_manifest_json(), now_secs());
        let m = verify_and_parse_manifest(&carrier, &anchors, now_secs() + 60)
            .expect("Valid + 剥 footer + 解析全通");
        assert_eq!(m.commit, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(m.sequence, 42);
        assert_eq!(m.exe_sha256, format!("{:064x}", 0xdead_beefu64));
    }

    #[test]
    fn manifest_tampered_byte_rejected() {
        let (mut carrier, anchors) = sign_manifest(&sample_manifest_json(), now_secs());
        carrier[0] ^= 1;
        let err = verify_and_parse_manifest(&carrier, &anchors, now_secs() + 60)
            .expect_err("单字节翻转必须拒绝");
        assert!(err.contains("清单"), "错误指认清单验签失败: {err}");
    }

    #[test]
    fn manifest_untrusted_anchor_rejected() {
        let (carrier, _) = sign_manifest(&sample_manifest_json(), now_secs());
        let other = nemesis_verify::keygen::generate().expect("other keygen");
        let wrong_anchors = vec![other.root_anchor_fingerprint()];
        assert!(
            verify_and_parse_manifest(&carrier, &wrong_anchors, now_secs() + 60).is_err(),
            "锚不匹配 = Untrusted = 拒"
        );
    }

    #[test]
    fn manifest_bare_json_rejected() {
        // 裸 JSON（无 CMS/footer）：签名验证不可能 Valid——诚实拒绝而非解析放行。
        let body = serde_json::to_vec(&sample_manifest_json()).unwrap();
        let kh = nemesis_verify::keygen::generate().unwrap();
        let anchors = vec![kh.root_anchor_fingerprint()];
        let err =
            verify_and_parse_manifest(&body, &anchors, now_secs() + 60).expect_err("裸 JSON 必拒");
        assert!(err.contains("清单"), "错误指认清单验签: {err}");
    }

    #[test]
    fn manifest_field_validation_rejects_garbage() {
        let kh = nemesis_verify::keygen::generate().unwrap();
        let anchors = vec![kh.root_anchor_fingerprint()];
        let cases = [
            (
                "空 commit",
                serde_json::json!({"commit": "", "sequence": 1,
             "exe_sha256": format!("{:064x}", 0), "built_at": "t"}),
            ),
            (
                "sequence 0",
                serde_json::json!({"commit": "0123456789abcdef0123456789abcdef01234567",
             "sequence": 0, "exe_sha256": format!("{:064x}", 0), "built_at": "t"}),
            ),
            (
                "坏 sha 长度",
                serde_json::json!({"commit": "0123456789abcdef0123456789abcdef01234567",
             "sequence": 1, "exe_sha256": "abc123", "built_at": "t"}),
            ),
            (
                "commit 非 hex",
                serde_json::json!({"commit": "zzzz456789abcdef0123456789abcdef01234567",
             "sequence": 1, "exe_sha256": format!("{:064x}", 0), "built_at": "t"}),
            ),
        ];
        for (tag, json) in cases {
            let body = serde_json::to_vec(&json).unwrap();
            let carrier = nemesis_verify::verify::sign_content_v4(
                &body,
                &kh.leaf_sk,
                now_secs(),
                &kh.chain(),
                None,
            )
            .unwrap();
            let err = verify_and_parse_manifest(&carrier, &anchors, now_secs() + 60)
                .expect_err(&format!("{tag} 必须被字段校验拒绝"));
            assert!(!err.is_empty(), "{tag} 错误信息非空");
        }
    }
    #[test]
    fn manifest_exe_digest_binding() {
        let m = crate::selfupdate::UpdateManifest {
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            sequence: 1,
            exe_sha256: sha256_hex(b"pe-bytes"),
            built_at: "t".into(),
        };
        assert!(
            manifest_exe_digest_matches(&m, b"pe-bytes"),
            "摘要一致 = 绑定成立"
        );
        assert!(
            !manifest_exe_digest_matches(&m, b"other-bytes"),
            "摘要不一致 = 调包/损坏，apply 侧无条件硬拒"
        );
        // 大小写不敏感（CI 侧 sha256sum 小写、清单若手写大写同样命中）。
        let upper = crate::selfupdate::UpdateManifest {
            exe_sha256: sha256_hex(b"pe-bytes").to_uppercase(),
            ..m
        };
        assert!(
            manifest_exe_digest_matches(&upper, b"pe-bytes"),
            "hex 大小写归一"
        );
    }

    #[test]
    fn applied_manifest_save_load_roundtrip() {
        let dir = tmp_dir("applied-roundtrip");
        let exe = dir.join("nemesisbot.exe");
        std::fs::write(&exe, b"pe").unwrap();
        let (carrier, anchors) = sign_manifest(&sample_manifest_json(), now_secs());
        save_applied_manifest(&exe, &carrier, &anchors, now_secs() + 60).expect("save");
        let seq = load_applied_sequence(&exe, &anchors, now_secs() + 120);
        assert_eq!(seq, Some(42), "applied 副本读回 sequence");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applied_manifest_tampered_loads_none() {
        let dir = tmp_dir("applied-tamper");
        let exe = dir.join("nemesisbot.exe");
        std::fs::write(&exe, b"pe").unwrap();
        let (mut carrier, anchors) = sign_manifest(&sample_manifest_json(), now_secs());
        carrier[3] ^= 0x80;
        std::fs::write(applied_manifest_path(&exe), &carrier).unwrap();
        assert_eq!(
            load_applied_sequence(&exe, &anchors, now_secs() + 60),
            None,
            "applied 副本被篡改 = 验签失败 = 顺序 Unknown（默认拒）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applied_manifest_wrong_anchor_loads_none() {
        let dir = tmp_dir("applied-anchor");
        let exe = dir.join("nemesisbot.exe");
        std::fs::write(&exe, b"pe").unwrap();
        let (carrier, _) = sign_manifest(&sample_manifest_json(), now_secs());
        std::fs::write(applied_manifest_path(&exe), &carrier).unwrap();
        let other = nemesis_verify::keygen::generate().unwrap();
        assert_eq!(
            load_applied_sequence(&exe, &[other.root_anchor_fingerprint()], now_secs() + 60),
            None,
            "锚不匹配的 applied 副本不可作顺序基准"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_applied_manifest_requires_anchors() {
        let dir = tmp_dir("applied-noanchor");
        let exe = dir.join("nemesisbot.exe");
        std::fs::write(&exe, b"pe").unwrap();
        let (carrier, _) = sign_manifest(&sample_manifest_json(), now_secs());
        // 空锚表 → 验签必 Untrusted → 拒绝写入（fail-closed，无独立锚闸——
        // 验签路径本身就是闸）。
        let err = save_applied_manifest(&exe, &carrier, &[], now_secs() + 60)
            .expect_err("空锚表必须写入失败");
        assert!(err.contains("验签"), "诚实报验签未过: {err}");
        assert!(
            !applied_manifest_path(&exe).exists(),
            "失败路径不落任何 applied 副本"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_bundle_reads_manifest_entry() {
        let (carrier, _) = sign_manifest(&sample_manifest_json(), now_secs());
        let bytes = zip_bytes(vec![
            ("nemesisbot.exe", b"pe-bytes".to_vec()),
            (
                "version.txt",
                b"Commit     : 0123456789abcdef0123456789abcdef01234567\n".to_vec(),
            ),
            ("manifest.json", carrier.clone()),
        ]);
        let bundle = super::super::extract_update_bundle(&bytes).expect("extract");
        assert_eq!(
            bundle.manifest_bytes.as_deref(),
            Some(carrier.as_slice()),
            "manifest 原样字节出包（签名覆盖面不被解包破坏）"
        );
    }

    #[test]
    fn extract_bundle_without_manifest_is_none() {
        let bytes = zip_bytes(vec![
            ("nemesisbot.exe", b"pe-bytes".to_vec()),
            (
                "version.txt",
                b"Commit     : 0123456789abcdef0123456789abcdef01234567\n".to_vec(),
            ),
        ]);
        let bundle = super::super::extract_update_bundle(&bytes).expect("extract");
        assert!(
            bundle.manifest_bytes.is_none(),
            "旧 CI 包 / 用户自打包 = 无清单（apply 侧 Unknown 类处理）"
        );
    }

    /// journal 旧文件兼容：R1 之前的 journal JSON（无 target_manifest 键）
    /// 必须能反序列化（serde default 空 = 无清单，收口不写 applied 副本）。
    #[test]
    fn journal_without_target_manifest_field_loads() {
        let legacy = serde_json::json!({
            "phase": "restarting",
            "target_commit": "0123456789abcdef0123456789abcdef01234567",
            "target_version": "v-test",
            "old_path": "C:\\x\\old",
            "started_ms": 1
        });
        let j: UpdateJournal = serde_json::from_value(legacy).expect("旧 journal 兼容");
        assert_eq!(j.target_manifest, "", "缺键 = 空串默认");
        // 反向：带清单的 journal 往返不丢字段。
        let full = UpdateJournal {
            target_manifest: "Y2Fycmllcg==".to_string(),
            ..journal("staging", "abc", "old")
        };
        let round: UpdateJournal =
            serde_json::from_value(serde_json::to_value(&full).unwrap()).unwrap();
        assert_eq!(round.target_manifest, "Y2Fycmllcg==");
    }

    fn manager_for(
        ws: PathBuf,
        estop: std::sync::Arc<nemesis_agent::estop::EstopState>,
    ) -> SelfUpdateManager {
        struct Allow;
        impl nemesis_security::auditor::ApprovalManager for Allow {
            fn is_running(&self) -> bool {
                true
            }
            fn request_approval_sync(
                &self,
                _request_id: &str,
                _operation: &str,
                _target: &str,
                _risk_level: &str,
                _reason: &str,
                _timeout_secs: u64,
            ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
                Ok(nemesis_security::auditor::ApprovalVerdict {
                    approved: true,
                    note: None,
                })
            }
        }
        SelfUpdateManager {
            workspace: ws,
            estop,
            approval: std::sync::Arc::new(Allow),
            audit: None,
            shutdown: std::sync::Arc::new(|| {}),
            flight: tokio::sync::Mutex::new(()),
        }
    }

    // apply 全链路 v1 仅 Windows（平台闸排第一道，见下方
    // apply_refused_on_non_windows_before_any_network）——闸序测试
    // 只在 Windows 编译（漏 gate 实录：Linux coverage 三连红，2026-10-14）。
    #[cfg(windows)]
    #[tokio::test]
    async fn apply_denied_by_estop() {
        let ws = tmp_dir("gate-estop");
        let estop = std::sync::Arc::new(nemesis_agent::estop::EstopState::new());
        estop.trigger();
        let m = manager_for(ws.clone(), estop);
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        assert!(err.contains("estop"), "estop 闸拒: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn apply_denied_by_pending_journal() {
        let ws = tmp_dir("gate-journal");
        save_journal(&ws, &journal("staging", "x", "y")).unwrap();
        let m = manager_for(
            ws.clone(),
            std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        );
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        assert!(err.contains("未收口"), "journal 闸拒: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn apply_denied_by_frequency_fuse() {
        let ws = tmp_dir("gate-fuse");
        stamp_fuse(&ws).expect("fuse stamp ok");
        let m = manager_for(
            ws.clone(),
            std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        );
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        assert!(err.contains("保险丝"), "频率闸拒: {err}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// R2 进程内单飞：flight 已被持有时 apply 诚实拒绝（estop 未触发、
    /// journal/fuse 干净——单飞闸是这一轮唯一的拒绝者）。
    #[cfg(windows)]
    #[tokio::test]
    async fn apply_rejected_when_singleflight_held() {
        let ws = tmp_dir("gate-singleflight");
        let m = manager_for(
            ws.clone(),
            std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        );
        let held = m.flight.lock().await;
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        assert!(err.contains("已有自更新在进行"), "单飞闸拒: {err}");
        drop(held);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 平台闸（v1 仅 Windows）：非 Windows 必须**先于一切网络/磁盘动作**
    /// 诚实报「暂不支持」——不能等下完 300MB 主包才撞 zip 解包失败。
    /// 本测试在 Windows 编译期消失，由 Linux nightly 兜底执行。
    #[cfg(all(test, not(target_os = "windows")))]
    #[tokio::test]
    async fn apply_refused_on_non_windows_before_any_network() {
        let ws = tmp_dir("gate-platform");
        // 故意放一个未收口 journal：平台闸必须排更前——若平台闸失效，
        // 这里会先撞 journal 错误信息，测试即红。
        save_journal(&ws, &journal("staging", "x", "y")).unwrap();
        let m = manager_for(
            ws.clone(),
            std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        );
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        assert!(
            err.contains("暂不支持") && err.contains("Windows"),
            "平台闸诚实拒（先于 journal/estop/网络）: {err}"
        );
        // 未触达下载路径 → 保险丝未被盖章（审批即承诺语义没被误触）。
        assert!(last_attempt_ms(&ws).is_none(), "平台闸不盖保险丝");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 验签负例矩阵在 apply 缝合处的映射钉：九态各构造一次，断言
    /// outcome_kind 全部给出非 Valid 的诚实档名（Valid-only 收口——除
    /// Valid 外任何态都必须映射到自己的名字而不是笼统「失败」，排障时
    /// 日志能直接区分被篡改/无签名/吊销/过期）。
    #[test]
    fn outcome_kind_maps_all_nine_states() {
        use nemesis_verify::verify::VerifyOutcome;
        let samples = [
            VerifyOutcome::Valid {
                signed_at: 0,
                key_fp: [0u8; 32],
                pubkey: [0u8; 65],
            },
            VerifyOutcome::NoSignature,
            VerifyOutcome::Tampered("t".into()),
            VerifyOutcome::SignatureInvalid,
            VerifyOutcome::UnsupportedVersion("v".into()),
            VerifyOutcome::Malformed("m".into()),
            VerifyOutcome::Untrusted,
            VerifyOutcome::Revoked {
                dim: nemesis_verify::RevDim::KeyFp,
                value: "x".into(),
                revoked_at: 0,
                reason: "r".into(),
            },
            VerifyOutcome::Expired("e".into()),
        ];
        let kinds: Vec<&str> = samples.iter().map(outcome_kind).collect();
        assert_eq!(
            kinds,
            [
                "Valid",
                "NoSignature",
                "Tampered",
                "SignatureInvalid",
                "UnsupportedVersion",
                "Malformed",
                "Untrusted",
                "Revoked",
                "Expired"
            ]
        );
    }
}
