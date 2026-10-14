//! L9 selfupdate 单测：journal/fuse 往返、commit 身份提取、boot 收口相位
//! 矩阵、PID 存活探测；`selfupdate` feature 下追加 zip 解包与 apply 前置
//! 闸（estop/journal/fuse——全部先于网络下载，可离线测）。审批/验签/staging
//! 的在线路径归集成验证（见 docs/REPORT 交付记录的诚实边界节）。

use super::*;
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
    // 原子写不留 tmp 残留。
    assert!(!journal_path(&ws).with_extension("json.tmp").exists());
    clear_journal(&ws);
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
    stamp_fuse(&ws);
    let last = last_attempt_ms(&ws).expect("stamped");
    assert!(now_ms() >= last && now_ms() - last < 60_000, "fuse ≈ now");
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
    let msg = rollback_old_exe_to("definitely-not-there.old", std::path::Path::new("t.exe"));
    assert!(msg.contains("不在场"));
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
        stamp_fuse(&ws);
        let m = manager_for(
            ws.clone(),
            std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        );
        let err = m.apply("v1", "0123456", false).await.unwrap_err();
        assert!(err.contains("保险丝"), "频率闸拒: {err}");
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
