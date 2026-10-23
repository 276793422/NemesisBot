//! L9 selfupdate 单测：journal 往返、待装包（updates/ pending）往返与损坏
//! 整清、commit 身份提取、boot 收口相位矩阵、PID 存活探测；`selfupdate`
//! feature 下追加 zip/tar.gz 双臂解包（含 `./` 前缀、unix mode、目录跳过、
//! symlink/嵌套条目拒收、条目名归一）、plugins 绑定校验矩阵、plugins 安装
//! /回滚舞步、apply/download 前置闸（estop/journal/单飞/平台——全部先于网
//! 络，可离线测）与 **R1 签名更新清单**套件（验签解析往返/篡改/字段校验/
//! exe+plugins 摘要绑定/applied 副本写入读回/sequence 顺序闸矩阵）。exe
//! 本体验签的在线路径归集成验证（见 docs/REPORT 交付记录的诚实边界节）。

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
        // 默认无 plugins 舞步记录（exe 单侧场景）；plugins 用例单独构造。
        plugins_backup_dir: String::new(),
        plugins_new_dir: String::new(),
    }
}

// ---------------------------------------------------------------------------
// journal
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

// ---------------------------------------------------------------------------
// plugins 整包替换舞步（rollback 侧不门控 feature——journal 双字段随主
// 结构进出，boot 收口在任何 feature 组合下都可能读到 plugins 舞步记录）
// ---------------------------------------------------------------------------

/// 双空字段（旧 journal / 无插件包）= plugins 侧 noop，可识别前缀在场。
#[test]
fn rollback_plugins_noop_when_no_journal_fields() {
    let detail = rollback_plugins_to("", "", std::path::Path::new("unused-plugins"))
        .expect("无舞步记录 = Ok");
    assert!(
        detail.starts_with(ROLLBACK_PLUGINS_NOOP_PREFIX),
        "noop 分支必须带可识别前缀: {detail}"
    );
    assert!(detail.contains("无 plugins 舞步记录"), "{detail}");
}

/// rollback_update 合成语义：exe .old 缺席 + plugins 双字段空 = 双 noop
/// （noop=true 的唯一形态——boot 收口据此判 rollback_noop 而非 rolled_back）。
#[test]
fn rollback_update_double_noop_when_no_recoverable_state() {
    let ws = tmp_dir("rollback-double-noop");
    let exe = ws.join("current.exe");
    std::fs::write(&exe, b"cur").unwrap();
    let j = journal(
        "restarting",
        "ff00",
        &ws.join("nope.old").display().to_string(),
    );
    let (detail, noop) = rollback_update(&j, &exe).expect("双 noop = Ok");
    assert!(noop, "exe .old 缺席 + plugins 无记录 = 全 noop: {detail}");
    assert!(detail.contains(ROLLBACK_NOOP_PREFIX), "{detail}");
    assert!(detail.contains(ROLLBACK_PLUGINS_NOOP_PREFIX), "{detail}");
    let _ = std::fs::remove_dir_all(&ws);
}

/// exe 侧真实回滚（.old 在场）+ plugins 侧 noop = 合成非 noop——boot 收口
/// 必须报 rolled_back 而非 rollback_noop（有一侧真动了）。
#[test]
fn rollback_update_exe_side_real_rollback_not_noop() {
    let ws = tmp_dir("rollback-exe-side");
    let old = ws.join("nemesisbot.exe.old");
    std::fs::write(&old, b"old-bytes").unwrap();
    let exe = ws.join("current.exe");
    std::fs::write(&exe, b"broken-new").unwrap();
    let j = journal(
        "restarting",
        "ffffffffffffffffffffffffffffffffffffffff",
        &old.display().to_string(),
    );
    let (detail, noop) = rollback_update(&j, &exe).expect("exe 回滚 + plugins noop");
    assert!(!noop, "exe 侧真实回滚 = 非 noop: {detail}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old-bytes", ".old 归位");
    assert!(
        detail.contains(ROLLBACK_PLUGINS_NOOP_PREFIX),
        "plugins noop 段仍在场（双字段空）: {detail}"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// plugins 回滚全舞步：backup 归位（同名让位先删）→ new-only 清理（用户
/// 自装件不受伤）→ staging 双目录收尾；二次调用幂等 = noop 前缀。
#[test]
fn rollback_plugins_restores_and_clears_new_only_then_noop() {
    let dir = tmp_dir("rollback-plugins");
    let plugins = dir.join("plugins");
    let backup = dir.join("plugins.old-7");
    let new = dir.join("plugins.new-7");
    for d in [&plugins, &backup, &new] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(plugins.join("a.dll"), b"new-a").unwrap();
    std::fs::write(plugins.join("keep.dll"), b"user-keep").unwrap();
    std::fs::write(plugins.join("c.dll"), b"new-c").unwrap();
    std::fs::write(backup.join("a.dll"), b"old-a").unwrap();
    std::fs::write(new.join("a.dll"), b"new-a").unwrap();
    std::fs::write(new.join("c.dll"), b"new-c").unwrap();

    let detail = rollback_plugins_to(
        &backup.display().to_string(),
        &new.display().to_string(),
        &plugins,
    )
    .expect("rollback");
    assert!(
        !detail.starts_with(ROLLBACK_PLUGINS_NOOP_PREFIX),
        "有真回滚 = 非 noop: {detail}"
    );
    assert_eq!(
        std::fs::read(plugins.join("a.dll")).unwrap(),
        b"old-a",
        "旧版归位"
    );
    assert_eq!(
        std::fs::read(plugins.join("keep.dll")).unwrap(),
        b"user-keep",
        "用户自装插件不受伤（不在 backup/new 名单 = 不动）"
    );
    assert!(
        !plugins.join("c.dll").exists(),
        "new-only 文件清理（只同名覆盖不删，回滚侧才删 new-only）"
    );
    assert!(!backup.exists() && !new.exists(), "staging 双目录收尾");

    // 幂等：现场已收光，二次回滚 = noop。
    let again = rollback_plugins_to(
        &backup.display().to_string(),
        &new.display().to_string(),
        &plugins,
    )
    .expect("rollback 2");
    assert!(
        again.starts_with(ROLLBACK_PLUGINS_NOOP_PREFIX),
        "二次回滚 = noop: {again}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 旁置名形态钉死（追加非替换）：同词干不同扩展名（a.dll/a.wasm）的旁置名
/// 必须互不相撞——with_extension 是替换扩展位，两文件撞同一个 `a.failed-P`
/// 时第二份让位在 Windows 上撞已存在目标直接失败，boot 回滚卡死循环。
#[test]
fn loaded_sidecar_name_appends_not_replaces_extension() {
    let dir = tmp_dir("sidecar-name");
    let pid = std::process::id();
    let a = loaded_sidecar_path(&dir.join("a.dll"));
    let b = loaded_sidecar_path(&dir.join("a.wasm"));
    assert_ne!(a, b, "同词干不同扩展名旁置名互不相撞");
    assert_eq!(a, dir.join(format!("a.dll.failed-{pid}")));
    assert_eq!(b, dir.join(format!("a.wasm.failed-{pid}")));
    // 无扩展名文件同样成立（追加语义天然覆盖）。
    let c = loaded_sidecar_path(&dir.join("plain"));
    assert_eq!(c, dir.join(format!("plain.failed-{pid}")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// remove_loaded_file 常规双路径：可删文件直接删干净（无旁置残留）；缺席
/// = 幂等 Ok。
#[test]
fn remove_loaded_file_removes_and_ignores_missing() {
    let dir = tmp_dir("remove-loaded");
    let f = dir.join("x.dll");
    std::fs::write(&f, b"x").unwrap();
    super::remove_loaded_file(&f).expect("可删文件直接删");
    assert!(!f.exists(), "已删干净");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(leftovers.is_empty(), "无旁置残留: {leftovers:?}");
    super::remove_loaded_file(&f).expect("缺席 = Ok（幂等）");
    let _ = std::fs::remove_dir_all(&dir);
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
        assert!(
            err.contains("nemesisbot") && err.contains("包结构异常"),
            "诚实报包结构异常（双形态名 nemesisbot(.exe)）: {err}"
        );
    }

    #[test]
    fn extract_zip_captures_flat_plugins_and_rejects_nested() {
        // Windows 主包（zip）形态：plugins/ 平面文件全集捕获，回滚/绑定面可用。
        let flat = zip_bytes(vec![
            ("nemesisbot.exe", b"pe".to_vec()),
            (
                "version.txt",
                b"Commit     : 0123456789abcdef0123456789abcdef01234567\n".to_vec(),
            ),
            ("plugins/a.dll", b"pa".to_vec()),
            ("plugins/libui.so", b"pb".to_vec()),
        ]);
        let bundle = super::super::extract_update_bundle(&flat).expect("flat plugins");
        assert_eq!(bundle.plugins.len(), 2);
        assert_eq!(
            bundle.plugins.get("a.dll").map(Vec::as_slice),
            Some(b"pa".as_slice())
        );
        // 嵌套子目录 = 拒收（插件平面名清单，listdir 语义装不下子树）。
        let nested = zip_bytes(vec![
            ("nemesisbot.exe", b"pe".to_vec()),
            ("plugins/sub/x.dll", b"px".to_vec()),
        ]);
        let err = super::super::extract_update_bundle(&nested).unwrap_err();
        assert!(err.contains("子目录嵌套"), "{err}");
    }

    // -----------------------------------------------------------------------
    // tar.gz 臂（Linux 主包 / macOS 留位同型）：CI `tar -czf … -C staging .`
    // 条目带 `./` 前缀、目录条目跳过、裸名 nemesisbot、unix mode 记录。
    // -----------------------------------------------------------------------

    fn tar_gz_bytes(entries: Vec<(tar::EntryType, &str, Option<u32>, Vec<u8>)>) -> Vec<u8> {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(enc);
        for (ty, name, mode, data) in entries {
            let mut h = tar::Header::new_gnu();
            match ty {
                tar::EntryType::Regular => {
                    h.set_size(data.len() as u64);
                    h.set_mode(mode.unwrap_or(0o644));
                    h.set_cksum();
                    b.append_data(&mut h, name, std::io::Cursor::new(data))
                        .unwrap();
                }
                _ => {
                    h.set_size(0);
                    h.set_entry_type(ty);
                    if let Some(m) = mode {
                        h.set_mode(m);
                    }
                    h.set_cksum();
                    b.append_data(&mut h, name, std::io::empty()).unwrap();
                }
            }
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extract_tar_gz_reads_bare_exe_mode_and_plugins() {
        let full = "0123456789abcdef0123456789abcdef01234567";
        let vt = format!("NemesisBot\nCommit     : {full}\n");
        let bytes = tar_gz_bytes(vec![
            (
                tar::EntryType::Regular,
                "./nemesisbot",
                Some(0o755),
                b"elf-bytes".to_vec(),
            ),
            (
                tar::EntryType::Regular,
                "./version.txt",
                None,
                vt.into_bytes(),
            ),
            (tar::EntryType::Directory, "./plugins", None, Vec::new()),
            (
                tar::EntryType::Regular,
                "./plugins/libui.so",
                None,
                b"so-bytes".to_vec(),
            ),
        ]);
        let bundle = super::super::extract_update_bundle(&bytes).expect("tar.gz extract");
        assert_eq!(
            bundle.exe_bytes, b"elf-bytes",
            "裸名 nemesisbot = 主程序条目"
        );
        assert_eq!(bundle.commit.as_deref(), Some(full));
        assert_eq!(
            bundle.exe_unix_mode,
            Some(0o755),
            "unix mode 记录（重装 chmod 恢复执行位）"
        );
        assert_eq!(
            bundle.plugins.get("libui.so").map(Vec::as_slice),
            Some(b"so-bytes".as_slice()),
            "目录条目跳过、平面文件捕获"
        );
    }

    #[test]
    fn extract_tar_gz_rejects_symlink_entry() {
        let bytes = tar_gz_bytes(vec![
            (
                tar::EntryType::Regular,
                "./nemesisbot",
                Some(0o755),
                b"elf".to_vec(),
            ),
            (tar::EntryType::Symlink, "./evil", None, Vec::new()),
        ]);
        let err = super::super::extract_update_bundle(&bytes).unwrap_err();
        assert!(err.contains("symlink"), "link 型条目拒收并点名: {err}");
    }

    #[test]
    fn extract_rejects_unknown_magic() {
        let err = super::super::extract_update_bundle(b"MZ\xffbinary").unwrap_err();
        assert!(err.contains("格式不可识别"), "{err}");
    }

    #[test]
    fn normalize_entry_name_matrix() {
        use super::super::normalize_entry_name;
        assert_eq!(normalize_entry_name("./nemesisbot").unwrap(), "nemesisbot");
        assert_eq!(
            normalize_entry_name("a\\b.dll").unwrap(),
            "a/b.dll",
            "Windows 反斜杠归一"
        );
        assert_eq!(
            normalize_entry_name("././plugins/x").unwrap(),
            "plugins/x",
            "循环剥 ./ 前缀"
        );
        assert!(
            normalize_entry_name("/abs/x")
                .unwrap_err()
                .contains("绝对路径"),
            "绝对路径条目拒收"
        );
        assert!(
            normalize_entry_name("a/../b").unwrap_err().contains(".."),
            "逃逸条目拒收"
        );
        assert!(normalize_entry_name("").unwrap_err().contains("空"));
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
            "built_at": "2026-10-16T00:00:00Z",
            "plugins_sha256": {"plugin-x.dll": sha256_hex(b"plugin-x-bytes")}
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
        assert_eq!(
            m.plugins_sha256.get("plugin-x.dll").map(String::as_str),
            Some(sha256_hex(b"plugin-x-bytes").as_str()),
            "plugins 逐文件绑定表随清单往返"
        );
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
            plugins_sha256: Default::default(),
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
        assert!(
            j.plugins_backup_dir.is_empty() && j.plugins_new_dir.is_empty(),
            "plugins 双字段同样缺键默认空（无舞步记录 = 回滚 noop）"
        );
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
        // 2026-10-21 两段式改版：审批卡退役（确认在 Dashboard 侧完成，v4 验签
        // 是安全闸）——Manager 不再持有 ApprovalManager；新增下载编排三件
        // （download_flight/download_cancel/download_progress）+ check 缓存。
        SelfUpdateManager {
            workspace: ws,
            estop,
            audit: None,
            shutdown: std::sync::Arc::new(|| {}),
            flight: tokio::sync::Mutex::new(()),
            download_flight: tokio::sync::Mutex::new(()),
            download_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            download_progress: std::sync::Arc::new(parking_lot::Mutex::new(
                DownloadProgress::default(),
            )),
            last_check: std::sync::Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    // -----------------------------------------------------------------------
    // 两段式 D2：pending 包（updates/）往返与损坏整清
    // -----------------------------------------------------------------------

    fn sample_pending() -> PendingUpdate {
        PendingUpdate {
            asset: TARGET_ASSET.to_string(),
            release_tag: "v0.0.0-test".to_string(),
            target_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            zip_sha256: sha256_hex(b"zip-bytes"),
            size_bytes: 9,
            downloaded_at_ms: 1_700_000_000_000,
        }
    }

    /// updates/ 空（目录已删，或删挂起幻影目录内零条目——Windows
    /// RemoveDirectory 已成功但名字仍可短暂枚举，断言条目而非目录存在性）。
    fn updates_dir_is_empty(exe: &std::path::Path) -> bool {
        match std::fs::read_dir(updates_dir(exe)) {
            Err(_) => true,
            Ok(rd) => rd.filter_map(|e| e.ok()).next().is_none(),
        }
    }

    /// meta 后写不变量 + 正常往返：zip 先于 meta 在场时 load=None（半截下载
    /// 不产生可装形态——「见 meta 必有 zip」）；meta 落盘后完整读回。
    #[test]
    fn pending_roundtrip_meta_last_invariant() {
        let dir = tmp_dir("pending-roundtrip");
        let exe = dir.join("nemesisbot.exe");
        std::fs::write(&exe, b"pe").unwrap();
        assert!(load_pending(&exe).is_none(), "无 meta = None");
        std::fs::create_dir_all(updates_dir(&exe)).unwrap();
        std::fs::write(pending_package_path(&exe, TARGET_ASSET), b"zip-bytes").unwrap();
        assert!(load_pending(&exe).is_none(), "只有包无 meta = None");
        save_pending(&exe, &sample_pending()).expect("save meta");
        let p = load_pending(&exe).expect("loaded");
        assert_eq!(p.release_tag, "v0.0.0-test");
        assert_eq!(p.target_commit, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(p.zip_sha256, sha256_hex(b"zip-bytes"));
        assert_eq!(p.size_bytes, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏 meta（用户拍板：坏包宁可重下）→ None + 整清 updates/。
    #[test]
    fn pending_bad_meta_clears_updates_dir() {
        let dir = tmp_dir("pending-bad-meta");
        let exe = dir.join("nemesisbot.exe");
        std::fs::create_dir_all(updates_dir(&exe)).unwrap();
        std::fs::write(pending_package_path(&exe, TARGET_ASSET), b"zip").unwrap();
        std::fs::write(pending_meta_path(&exe), "{not json").unwrap();
        assert!(load_pending(&exe).is_none(), "坏 meta = None");
        assert!(updates_dir_is_empty(&exe), "坏 meta → updates/ 整清重来");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sha 复验不匹配（下载后字节被改/包损坏）→ None + 整清，不给坏包安装机会。
    #[test]
    fn pending_sha_mismatch_clears_updates_dir() {
        let dir = tmp_dir("pending-sha");
        let exe = dir.join("nemesisbot.exe");
        std::fs::create_dir_all(updates_dir(&exe)).unwrap();
        std::fs::write(pending_package_path(&exe, TARGET_ASSET), b"corrupted").unwrap();
        save_pending(&exe, &sample_pending()).expect("save meta");
        assert!(load_pending(&exe).is_none(), "sha 不符 = None");
        assert!(updates_dir_is_empty(&exe), "sha 不符 → updates/ 整清重来");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// meta 在 zip 不在（正常顺序见不到的异常现场）→ None + 整清。
    #[test]
    fn pending_meta_without_zip_clears_updates_dir() {
        let dir = tmp_dir("pending-no-zip");
        let exe = dir.join("nemesisbot.exe");
        std::fs::create_dir_all(updates_dir(&exe)).unwrap();
        save_pending(&exe, &sample_pending()).expect("save meta");
        assert!(load_pending(&exe).is_none(), "zip 缺失 = None");
        assert!(updates_dir_is_empty(&exe), "zip 缺失 → updates/ 整清");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// download 前置闸（离线可达）：estop 生效 = 拒绝（pending 未就绪时
    /// estop 闸是第一道——测试进程 exe 目录无 updates/，不触网）。
    #[tokio::test]
    async fn download_denied_by_estop() {
        let estop = std::sync::Arc::new(nemesis_agent::estop::EstopState::new());
        estop.trigger();
        let m = manager_for(tmp_dir("dl-estop"), estop);
        let err = m.download("0123456").await.unwrap_err();
        assert!(err.contains("estop"), "estop 闸拒下载: {err}");
    }

    // apply 全链路主体：Windows + Linux 同一份 cfg 编译体（2026-10-22 Linux
    // 接入）；macOS/android 是桩（诚实拒，见下方 apply_refused_on_stub_platforms
    // _before_any_network）。闸序测试与被测函数同 cfg 域（漏 gate 实录：Linux
    // coverage 三连红，2026-10-14——教训保持）。
    #[cfg(any(windows, target_os = "linux"))]
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

    #[cfg(any(windows, target_os = "linux"))]
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

    /// R2 进程内单飞：flight 已被持有时 apply 诚实拒绝（estop 未触发、
    /// journal 干净——单飞闸是这一轮唯一的拒绝者）。
    #[cfg(any(windows, target_os = "linux"))]
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

    /// 平台闸（桩平台：macOS 留位 / Android APK 发行形态）：必须**先于一切
    /// 网络/磁盘动作**诚实拒绝——Android 文案点名 APK 通道（非「暂不支持」，
    /// 那是能力缺席不是形态错位）；macOS 文案「暂不支持」+ 手动替换指引。
    /// 本测试在 Windows/Linux 编译期消失（Linux 2026-10-22 起接入全链路主
    /// 体），由对应桩平台的本地构建兜底执行。
    #[cfg(all(test, not(any(target_os = "windows", target_os = "linux"))))]
    #[tokio::test]
    async fn apply_refused_on_stub_platforms_before_any_network() {
        let ws = tmp_dir("gate-platform");
        // 故意放一个未收口 journal：平台闸必须排更前——若平台闸失效，
        // 这里会先撞 journal 错误信息，测试即红。
        save_journal(&ws, &journal("staging", "x", "y")).unwrap();
        let m = manager_for(
            ws.clone(),
            std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        );
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        if TARGET_ASSET.is_empty() {
            assert!(err.contains("APK"), "Android 桩诚实指认 APK 通道: {err}");
        } else {
            assert!(
                err.contains("暂不支持"),
                "macOS 留位桩诚实拒（先于 journal/estop/网络）: {err}"
            );
        }
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

    // -----------------------------------------------------------------------
    // plugins 整包替换：绑定校验矩阵 + 安装舞步 + sweep 面
    // -----------------------------------------------------------------------

    fn manifest_with_plugins(table: &[(&str, &str)]) -> crate::selfupdate::UpdateManifest {
        crate::selfupdate::UpdateManifest {
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            sequence: 1,
            exe_sha256: format!("{:064x}", 0),
            built_at: "t".into(),
            plugins_sha256: table
                .iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect(),
        }
    }

    fn plugins_map(pairs: &[(&str, &[u8])]) -> std::collections::BTreeMap<String, Vec<u8>> {
        pairs
            .iter()
            .map(|(n, b)| (n.to_string(), b.to_vec()))
            .collect()
    }

    /// ②-b 绑定闸语义矩阵（用户拍板「1. 可以加」）：空表 = 诚实降级 Ok；
    /// 集合差异（数量不等 / 名单不同）= 拒；摘要失配 = 拒；全链大小写不
    /// 敏感（CI 侧 sha256sum 小写 / 手写大写清单同命中）。
    #[test]
    fn plugins_binding_matrix() {
        // 空绑定 + 包内有插件 = 降级 Ok（旧 CI 包 / 无插件包形态）。
        let m = manifest_with_plugins(&[]);
        assert!(
            verify_plugins_binding_map(&m, &plugins_map(&[("a.dll", b"x")])).is_ok(),
            "空表 = 无绑定语义，诚实降级"
        );
        // 数量不等（清单 2 / 包 1）= 拒，错误点名缺失与多出。
        let m =
            manifest_with_plugins(&[("a.dll", &sha256_hex(b"1")), ("b.dll", &sha256_hex(b"2"))]);
        let err = verify_plugins_binding_map(&m, &plugins_map(&[("a.dll", b"1")]))
            .expect_err("集合不一致必拒");
        assert!(err.contains("集合"), "指认集合不一致: {err}");
        // 数量相等、名单不同 = 拒（缺清单绑定项）。
        let m = manifest_with_plugins(&[("b.dll", &sha256_hex(b"2"))]);
        let err = verify_plugins_binding_map(&m, &plugins_map(&[("a.dll", b"1")]))
            .expect_err("名单不同必拒");
        assert!(err.contains("缺清单绑定"), "点名缺失文件: {err}");
        // 摘要不匹配 = 拒（完整性问题）。
        let m = manifest_with_plugins(&[("a.dll", &sha256_hex(b"good"))]);
        let err = verify_plugins_binding_map(&m, &plugins_map(&[("a.dll", b"evil")]))
            .expect_err("摘要失配必拒");
        assert!(err.contains("摘要不匹配"), "指认完整性问题: {err}");
        // 大小写不敏感：大写文件名 + 大写摘要清单 vs 包内小写字节 = Ok。
        let m = manifest_with_plugins(&[("A.DLL", &sha256_hex(b"payload").to_uppercase())]);
        assert!(
            verify_plugins_binding_map(&m, &plugins_map(&[("a.dll", b"payload")])).is_ok(),
            "hex 与文件名全归一"
        );
        // 正例：精确匹配 = Ok。
        let m = manifest_with_plugins(&[("a.dll", &sha256_hex(b"payload"))]);
        assert!(verify_plugins_binding_map(&m, &plugins_map(&[("a.dll", b"payload")])).is_ok(),);
    }

    /// 清单 plugins_sha256 字段级校验（签名后仍拒垃圾）：文件名空/绝对路径/
    /// 反斜杠/逃逸/嵌套 + 摘要非 64 hex，全部诚实拒绝且点名 plugins_sha256。
    #[test]
    fn manifest_plugins_sha256_field_validation() {
        let kh = nemesis_verify::keygen::generate().unwrap();
        let anchors = vec![kh.root_anchor_fingerprint()];
        let good = sha256_hex(b"x");
        let cases = [
            ("空文件名", serde_json::json!({"": good})),
            ("绝对路径名", serde_json::json!({"/abs.dll": good})),
            ("反斜杠名", serde_json::json!({"a\\b.dll": good})),
            ("逃逸名", serde_json::json!({"../x.dll": good})),
            ("嵌套名", serde_json::json!({"sub/x.dll": good})),
            ("短摘要", serde_json::json!({"a.dll": "abc123"})),
        ];
        for (tag, table) in cases {
            let json = serde_json::json!({
                "commit": "0123456789abcdef0123456789abcdef01234567",
                "sequence": 1,
                "exe_sha256": format!("{:064x}", 0),
                "built_at": "t",
                "plugins_sha256": table,
            });
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
                .expect_err(&format!("{tag} 必拒"));
            assert!(
                err.contains("plugins_sha256"),
                "{tag}: 错误须点名字段: {err}"
            );
        }
    }

    /// 安装舞步（用户拍板「2. 可以」）：new_dir 全集先写 + 同名让位进
    /// backup + 只同名覆盖不删（用户自装插件不受伤）。
    #[test]
    fn install_plugins_files_replaces_and_preserves_extras() {
        let dir = tmp_dir("install-plugins");
        let plugins = dir.join("plugins");
        let backup = dir.join("plugins.old-1");
        let new = dir.join("plugins.new-1");
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(plugins.join("a.dll"), b"old-a").unwrap();
        std::fs::write(plugins.join("keep.dll"), b"user-keep").unwrap();
        let files = std::collections::BTreeMap::from([
            ("a.dll".to_string(), b"new-a".to_vec()),
            ("c.dll".to_string(), b"new-c".to_vec()),
        ]);
        let replaced =
            super::super::install_plugins_files(&plugins, &backup, &new, &files).expect("install");
        assert_eq!(replaced, 2, "replaced = 同名让位数 + 新装数（全集）");
        assert_eq!(
            std::fs::read(plugins.join("a.dll")).unwrap(),
            b"new-a",
            "同名覆盖"
        );
        assert_eq!(
            std::fs::read(plugins.join("keep.dll")).unwrap(),
            b"user-keep",
            "用户自装插件不在包内 = 不动"
        );
        assert_eq!(
            std::fs::read(plugins.join("c.dll")).unwrap(),
            b"new-c",
            "新装落位"
        );
        assert_eq!(
            std::fs::read(backup.join("a.dll")).unwrap(),
            b"old-a",
            "旧版让位进 backup（回滚原料）"
        );
        assert_eq!(
            std::fs::read(new.join("a.dll")).unwrap(),
            b"new-a",
            "new_dir 全集副本（回滚 new-only 清理名单）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同名**目录**冲突预检（拍板 ② 目录面）：包内平面文件撞既有目录 =
    /// 诚实拒绝，用户目录原地不动、backup 不创建（预检先于任何落盘）。
    /// 不拒的后果：目录被让位进 backup，回滚名单只认文件、收尾整删 backup
    /// = 用户自装内容永久丢失。
    #[test]
    fn install_plugins_files_rejects_directory_collision() {
        let dir = tmp_dir("install-plugins-dircoll");
        let plugins = dir.join("plugins");
        let backup = dir.join("plugins.old-1");
        let new = dir.join("plugins.new-1");
        let user_dir = plugins.join("a.dll");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(user_dir.join("user-data.txt"), b"keep").unwrap();
        let files = std::collections::BTreeMap::from([("a.dll".to_string(), b"new-a".to_vec())]);
        let err = super::super::install_plugins_files(&plugins, &backup, &new, &files)
            .expect_err("目录冲突必须拒");
        assert!(err.contains("目录"), "错误须点名目录冲突: {err}");
        assert!(
            user_dir.join("user-data.txt").exists(),
            "用户目录原地不动（未被让位）"
        );
        assert!(!backup.exists(), "预检失败 = backup 不创建（先于任何落盘）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sweep 扩展面：plugins.staging 双前缀目录清除；**裸 plugins 真实目录
    /// 永不在清扫面**（前缀排除裸名——真插件目录沾清扫面 = 生产事故）。
    #[test]
    fn sweep_stale_staging_removes_plugins_dirs_spares_real_plugins() {
        let ws = tmp_dir("sweep-plugins");
        let exe = ws.join("nemesisbot.exe");
        std::fs::write(&exe, b"running").unwrap();
        let real = ws.join("plugins");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("keep.dll"), b"real").unwrap();
        let stale_old = ws.join(format!("plugins.old-{}", std::process::id() + 1));
        let stale_new = ws.join(format!("plugins.new-{}", std::process::id() + 2));
        for d in [&stale_old, &stale_new] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("x.dll"), b"x").unwrap();
        }
        let removed = super::super::sweep_stale_staging(&exe);
        assert_eq!(removed, 2, "两个 plugins staging 目录清除");
        assert!(!stale_old.exists() && !stale_new.exists());
        assert!(
            real.join("keep.dll").exists(),
            "裸 plugins 真实目录不在清扫面"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}
