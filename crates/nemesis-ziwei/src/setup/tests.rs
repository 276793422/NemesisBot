//! setup 单测（.2-.7 文件面）。
//!
//! 纯文件步骤（annotate/define_struct/define_tool/commit/abort/status）
//! 跨平台直测；select_target/snapshot/diff 依赖真实进程 + payload 管道，
//! 归 UAT（dummy-draw 全链路，`test-tools/resource/ziwei-uat/m3/`）。

use std::path::PathBuf;

use super::{
    SetupInfo, SetupManager, SetupState, draft_dir, global, init_draft_files, load_draft,
    save_draft, save_draft_anchors,
};
// rollback_draft_error 的诚实性单测是 windows 门（share_mode）——Linux 测试
// 目标不引用它，跟随生产侧 cfg(windows) 门，避免未使用导入红。
#[cfg(windows)]
use super::rollback_draft_error;
use nemesis_ziwei_profile::{AnchorSet, Machine, Profile, ProfileStore, SCHEMA_VERSION};

// ── 夹具 ────────────────────────────────────────────────────────────────

fn temp_ws(tag: &str) -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "zwsetup-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

fn cleanup(ws: &PathBuf) {
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(ws);
}

/// 最小 draft（select_target 在 windows 下的产物形态）。
fn test_draft(id: &str) -> Profile {
    let now = chrono::Utc::now();
    serde_json::from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "profile_id": id,
            "name": format!("draft: dummy.exe"),
            "created_at": now.to_rfc3339(),
            "last_verified_at": now.to_rfc3339(),
            "status": "active",
            "target": {
                "exe_name": "dummy.exe",
                "machine": "x64",
                "file_sha256": format!("{:064x}", 42u32),
                "verify_policy": "hash_or_version",
            },
            "verification": {
                "on_attach": ["file_sha256", "anchors_vote>=2"],
                "stale_action": "notify_user",
            },
        }))
        .unwrap(),
    )
    .unwrap()
}

/// 建 draft 目录 + 文件（模拟 select_target 的落盘产物）。
fn seed_draft(ws: &std::path::Path, id: &str) {
    let dir = draft_dir(ws, id);
    std::fs::create_dir_all(&dir).unwrap();
    save_draft(ws, &test_draft(id)).unwrap();
    save_draft_anchors(
        ws,
        &AnchorSet {
            profile_id: id.to_string(),
            anchors: Vec::new(),
        },
    )
    .unwrap();
}

/// pid=0 占位形态的 info（select_target 预占态 / analyze 重建态共用形状）。
fn placeholder_info(id: &str) -> SetupInfo {
    SetupInfo {
        profile_id: id.to_string(),
        pid: 0,
        module_base: 0,
        module_size: 0,
        machine: Machine::X64,
        exe_path: PathBuf::new(),
        exe_name: String::new(),
        file_sha256: String::new(),
    }
}

/// 预占占位态（select_target 锁内插入的窗口期形态）。
fn inject_reserved(mgr: &SetupManager, id: &str) {
    mgr.state.lock().unwrap().replace(SetupState {
        info: placeholder_info(id),
        analysis: None,
        reserved: true,
    });
}

fn inject_state(mgr: &SetupManager, id: &str) {
    mgr.state.lock().unwrap().replace(SetupState {
        info: SetupInfo {
            profile_id: id.to_string(),
            pid: 4242,
            module_base: 0x140000000,
            module_size: 0x10000,
            machine: Machine::X64,
            exe_path: PathBuf::from("C:/dummy/dummy.exe"),
            exe_name: "dummy.exe".to_string(),
            file_sha256: "aa".repeat(32),
        },
        analysis: None,
        reserved: false,
    });
}

fn annotate_body(semantic: &str, with_anchors: bool) -> serde_json::Value {
    serde_json::json!({
        "rva": 0x2A10u64,
        "semantic": semantic,
        "anchors": if with_anchors {
            vec![serde_json::json!({
                "kind": "byte_pattern",
                "pattern": "90 90 90 90 b8 8d 7c 6b 5a",
            })]
        } else {
            vec![]
        },
    })
}

// ── annotate ────────────────────────────────────────────────────────────

#[tokio::test]
async fn annotate_forces_anchor_and_persists() {
    let ws = temp_ws("ann");
    let id = "zw-pro-d111";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 强制锚：无锚 = 拒绝
    let err = mgr
        .annotate(&ws, &annotate_body("push_record", false))
        .unwrap_err();
    assert!(err.contains("≥1"), "实际：{err}");

    // 带锚：通过；draft.functions/anchors 落盘
    let out = mgr
        .annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    assert_eq!(out["annotated"], "push_record");
    assert_eq!(out["anchors"], 1);
    let profile = load_draft(&ws, id).unwrap();
    assert_eq!(profile.functions.len(), 1);
    assert_eq!(profile.functions[0].semantic, "push_record");
    assert_eq!(
        profile.functions[0].locate.kind,
        nemesis_ziwei_profile::LocateKind::BytePattern,
        "byte_pattern 锚应成为函数 locate"
    );
    let anchors = super::load_draft_anchors(&ws, id).unwrap();
    assert_eq!(anchors.anchors.len(), 1);
    assert_eq!(anchors.anchors[0].function, "push_record");
    assert_eq!(anchors.anchors[0].module.as_deref(), Some("dummy.exe"));

    // 重复语义名 = 拒绝；重复 RVA = 拒绝
    let dup = mgr
        .annotate(&ws, &annotate_body("push_record", true))
        .unwrap_err();
    assert!(dup.contains("已标注"), "实际：{dup}");
    let mut other = annotate_body("feed_render", true);
    other["rva"] = serde_json::json!(0x2A10u64); // 同 RVA 不同名
    let dup_rva = mgr.annotate(&ws, &other).unwrap_err();
    assert!(dup_rva.contains("已被"), "实际：{dup_rva}");

    // 坏 pattern = 拒绝（奇数 hex；独立 RVA——RVA 查重先于 pattern 解析）
    let mut bad = annotate_body("other_fn", true);
    bad["rva"] = serde_json::json!(0x2A20u64);
    bad["anchors"] = serde_json::json!([{"kind": "byte_pattern", "pattern": "9 zz"}]);
    let bad_pat = mgr.annotate(&ws, &bad).unwrap_err();
    assert!(bad_pat.contains("pattern 非法"), "实际：{bad_pat}");
    cleanup(&ws);
}

// ── define_struct / define_tool ─────────────────────────────────────────

#[tokio::test]
async fn define_struct_and_tool_roundtrip() {
    let ws = temp_ws("ds");
    let id = "zw-pro-d222";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 结构定义
    let ds = serde_json::json!({
        "name": "DataHeader",
        "size": 24,
        "fields": [
            {"offset": 0, "name": "magic", "type": "u32"},
            {"offset": 4, "name": "layout_version", "type": "u32"},
            {"offset": 8, "name": "count", "type": "u32"},
            {"offset": 12, "name": "capacity", "type": "u32"},
            {"offset": 16, "name": "last_tick_ms", "type": "u64"},
        ],
    });
    let out = mgr.define_struct(&ws, &ds).unwrap();
    assert_eq!(out["struct"], "DataHeader");

    // 越界字段 = 拒绝
    let bad = serde_json::json!({
        "name": "Bad", "size": 8,
        "fields": [{"offset": 4, "name": "v", "type": "u64"}],
    });
    let err = mgr.define_struct(&ws, &bad).unwrap_err();
    assert!(err.contains("越界"), "实际：{err}");

    // 未知类型 = 拒绝
    let bad_ty = serde_json::json!({
        "name": "Bad2", "size": 8,
        "fields": [{"offset": 0, "name": "v", "type": "int128"}],
    });
    assert!(mgr.define_struct(&ws, &bad_ty).is_err());

    // poll 工具（struct: 引用已定义 → 校验过）
    let poll = serde_json::json!({
        "tool_name": "dummy_area_read",
        "description": "读 DataHeader+记录",
        "action": {
            "primitive": "poll_report",
            "addr": "struct:DataHeader",
            "mode": "on_change",
        },
        "output": {"dest": "ziwei/runs/latest.json"},
    });
    let out = mgr.define_tool(&ws, &poll).unwrap();
    assert_eq!(out["tools"], 1);

    // call_fn 引用未标注函数 = 拒绝
    let bad_call = serde_json::json!({
        "tool_name": "dummy_call",
        "description": "调 push_record",
        "action": {"primitive": "call_fn", "addr": "fn:push_record"},
    });
    let err = mgr.define_tool(&ws, &bad_call).unwrap_err();
    assert!(err.contains("不在 functions"), "实际：{err}");

    // 标注后 call_fn 可定义（默认 run_policy=confirm，决议 1）
    mgr.annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    let call = mgr.define_tool(&ws, &bad_call).unwrap();
    assert_eq!(call["tools"], 2);
    let profile = load_draft(&ws, id).unwrap();
    let call_tool = profile
        .tools
        .iter()
        .find(|t| t.tool_name == "dummy_call")
        .unwrap();
    assert_eq!(
        call_tool.run_policy,
        nemesis_ziwei_profile::RunPolicy::Confirm,
        "call_fn 默认 confirm"
    );
    cleanup(&ws);
}

// ── commit / abort / status ─────────────────────────────────────────────

#[tokio::test]
async fn commit_migrates_draft_to_profile() {
    let ws = temp_ws("commit");
    let id = "zw-pro-d333";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 无函数 = 拒绝提交
    let err = mgr.commit(&ws).unwrap_err();
    assert!(err.contains("至少 annotate"), "实际：{err}");

    // 补齐：结构 + 函数 + 工具
    mgr.define_struct(
        &ws,
        &serde_json::json!({
            "name": "DataHeader", "size": 24,
            "fields": [{"offset": 0, "name": "magic", "type": "u32"}],
        }),
    )
    .unwrap();
    mgr.annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    mgr.define_tool(
        &ws,
        &serde_json::json!({
            "tool_name": "dummy_area_read",
            "description": "读",
            "action": {"primitive": "poll_report", "addr": "struct:DataHeader"},
        }),
    )
    .unwrap();

    let out = mgr.commit(&ws).unwrap();
    assert_eq!(out["committed"], id);
    assert_eq!(out["functions"], 1);
    assert_eq!(out["tools"], 1);
    assert_eq!(out["anchors"], 1);

    // profiles/ 有正式 profile 且可读回；draft 目录已清
    let store = ProfileStore::new(&ws);
    let profile = store.load(id).unwrap();
    assert_eq!(profile.functions[0].semantic, "push_record");
    let anchors = store.load_anchors(id).unwrap();
    assert_eq!(anchors.anchors.len(), 1);
    assert!(!draft_dir(&ws, id).exists(), "draft 目录应已删除");
    // 提交后状态清空（再次 commit = 无向导）
    assert!(mgr.commit(&ws).is_err());
    cleanup(&ws);
}

#[tokio::test]
async fn abort_clears_orphan_draft_and_state() {
    let ws = temp_ws("abort");
    let id = "zw-pro-d444";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id);
    assert!(!draft_dir(&ws, id).exists(), "draft 目录应已删除");
    assert!(
        !ws.join("ziwei").join("setup").join(id).exists(),
        "setup 目录应已删除"
    );
    // 二次 abort = 无向导
    assert!(mgr.abort(&ws).await.is_err());
    cleanup(&ws);
}

#[tokio::test]
async fn status_falls_back_to_disk_after_restart() {
    let ws = temp_ws("status");
    let id = "zw-pro-d555";
    seed_draft(&ws, id);
    let mgr = SetupManager::new(); // 模拟 gateway 重启：内存态空

    let st = mgr.status(&ws).unwrap();
    assert_eq!(st["profile_id"], id);
    assert_eq!(st["resumed"], true);

    // 文件面步骤照常可用（require_info 重建路径）
    let out = mgr
        .annotate(&ws, &annotate_body("fn_after_restart", true))
        .unwrap();
    assert_eq!(out["annotated"], "fn_after_restart");

    // 快照类步骤诚实拒绝（基址不可信；Linux 形态诚实报「仅 Windows」——
    // 同 select_target 用例的平台双臂断言先例）
    let err = mgr.snapshot(&ws).await.unwrap_err();
    assert!(
        err.contains("ASLR") || err.contains("重新 select_target") || err.contains("仅 Windows"),
        "实际：{err}"
    );
    cleanup(&ws);
}

#[tokio::test]
async fn select_target_rejects_when_draft_exists() {
    let ws = temp_ws("sel");
    let id = "zw-pro-d666";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    // 有 draft 未收尾 → 无论平台，select_target 必须拒绝（开新向导前提）
    let err = mgr.select_target(&ws, Some(1), None).await.unwrap_err();
    assert!(
        err.contains("draft") || err.contains("仅 Windows"),
        "实际：{err}"
    );
    let _ = id;
    cleanup(&ws);
}

// ── 全局槽 / 辅助 ───────────────────────────────────────────────────────

#[test]
fn global_slot_stable() {
    let a = global();
    let b = global();
    assert!(std::sync::Arc::ptr_eq(&a, &b), "全局槽应返回同一实例");
}

#[test]
fn normalize_pattern_shapes_hex() {
    // 经 annotate 间接验证（函数私有）：byte_pattern 锚 pattern 归一化大写
    let ws = temp_ws("norm");
    let id = "zw-pro-d777";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);
    mgr.annotate(
        &ws,
        &serde_json::json!({
            "rva": 0x10u64,
            "semantic": "norm_fn",
            "anchors": [{"kind": "byte_pattern", "pattern": "b8 8d 7c 6b 5a"}],
        }),
    )
    .unwrap();
    let anchors = super::load_draft_anchors(&ws, id).unwrap();
    assert_eq!(
        anchors.anchors[0].pattern.as_deref(),
        Some("B8 8D 7C 6B 5A")
    );
    cleanup(&ws);
}

// ──：draft 与已提交库的隔离根因钉死 ────────────────────

/// 根因不对称：向导期 profile_id 只存在于 `ziwei/drafts/<id>/`（本向导
/// 新造的毫秒戳 id，已提交库 `ziwei/profiles/<id>/` 必不存在）——旧 hook
/// 臂用 `ProfileStore:load`（只读已提交库）恒 NotFound → hook_notes
/// 「draft profile 不可用」+ fn_addrs 永不设置 = hook=true 臂整条死路。
/// 修复后 hook 臂走 `load_draft`（本测试钉死：同一 id load_draft 成功、
/// ProfileStore:load 诚实 NotFound）。
#[test]
fn draft_only_id_is_invisible_to_committed_store() {
    let ws = temp_ws("draft-only");
    let id = "zw-pro-d1751700000000";
    seed_draft(&ws, id);

    // 新路径：hook 臂同源读法，成功
    let profile = load_draft(&ws, id).expect("load_draft 应命中向导 draft");
    assert_eq!(profile.profile_id, id);

    // 旧路径：已提交库对同一 id 诚实 NotFound（死路根因）
    let committed = ProfileStore::new(&ws).load(id);
    assert!(committed.is_err(), "已提交库不得看到 draft：{committed:?}");

    cleanup(&ws);
}

// ── 预占态竞态修复（2026-11-05 风险报告评审）：reserved 单一真相源 ────────
//
// select_target 预占窗口（BUG-20261105-019）以 pid=0 占位态插入，旧形态无
// 显式标记：并发 abort 跳过 detach 后无条件删 draft + 清态，预占者写回无
// 条件复活 → 僵尸活动态 + 注入会话泄漏；Err 臂无条件清态可误清他人态。
// 修复 = SetupState.reserved 显式字段 + 清态/写回归属校验式 + abort 拒绝
// 预占态。pid==0 不可作占位判据（analyze 重建态合法 pid=0），重建态照常
// 可 abort 由 abort_clears_rebuilt_state 钉死。
// select_target_reserved 本体（windows + 真实进程）归 UAT。

#[test]
fn clear_state_if_owned_only_clears_own_state() {
    let mgr = SetupManager::new();

    // 自己的态：清空 + true
    inject_state(&mgr, "zw-pro-c1");
    assert!(mgr.clear_state_if_owned("zw-pro-c1"));
    assert!(mgr.state.lock().unwrap().is_none());

    // 他人态（id 不匹配）：不清 + false
    inject_state(&mgr, "zw-pro-c1");
    assert!(!mgr.clear_state_if_owned("zw-pro-c2"));
    assert!(mgr.state.lock().unwrap().is_some(), "易主时不得误清他人态");

    // None：false
    *mgr.state.lock().unwrap() = None;
    assert!(!mgr.clear_state_if_owned("zw-pro-c1"));
    assert!(mgr.state.lock().unwrap().is_none());
}

#[test]
fn publish_state_if_owned_closes_reservation_and_refuses_takeover() {
    let mgr = SetupManager::new();
    let info = SetupInfo {
        profile_id: "zw-pro-p1".to_string(),
        pid: 1234,
        module_base: 0x140000000,
        module_size: 0x10000,
        machine: Machine::X64,
        exe_path: PathBuf::from("C:/x/t.exe"),
        exe_name: "t.exe".to_string(),
        file_sha256: "bb".repeat(32),
    };

    // 预占态 + 同 id 写回：替换为完整态且 reserved 落 false（窗口收口）
    inject_reserved(&mgr, "zw-pro-p1");
    assert!(mgr.publish_state_if_owned("zw-pro-p1", info.clone()));
    {
        let st = mgr.state.lock().unwrap();
        let st = st.as_ref().expect("写回后状态应在场");
        assert_eq!(st.info.pid, 1234);
        assert!(!st.reserved, "写回应落 reserved=false（预占收口）");
    }

    // 易主（当前态 id ≠ 写回 id）：拒写且不覆盖
    inject_state(&mgr, "zw-pro-p1");
    let mut foreign = info.clone();
    foreign.profile_id = "zw-pro-other".to_string();
    assert!(!mgr.publish_state_if_owned("zw-pro-other", foreign));
    assert_eq!(
        mgr.state.lock().unwrap().as_ref().unwrap().info.profile_id,
        "zw-pro-p1",
        "易主时不得覆盖他人态"
    );

    // None：拒写且不复活（旧实现无条件写回正是僵尸态来源）
    *mgr.state.lock().unwrap() = None;
    assert!(!mgr.publish_state_if_owned("zw-pro-p1", info));
    assert!(mgr.state.lock().unwrap().is_none(), "None 态不得被写回复活");
}

#[test]
fn init_draft_files_creates_all_three() {
    let ws = temp_ws("init-ok");
    let id = "zw-pro-init1";
    let profile = test_draft(id);
    let draft = init_draft_files(&ws, &profile).unwrap();
    assert_eq!(draft, draft_dir(&ws, id));
    assert!(draft.join("draft.json").exists(), "draft.json 应落盘");
    assert!(draft.join("anchors.json").exists(), "anchors.json 应落盘");
    assert!(load_draft(&ws, id).is_ok(), "draft.json 应可回读");
    cleanup(&ws);
}

#[test]
fn init_draft_files_rolls_back_partial_failure() {
    let ws = temp_ws("init-fail");
    let id = "zw-pro-init2";
    let profile = test_draft(id);
    // 失败注入：draft.json 位置预置为目录 → save_draft 的 fs::write 必败
    let draft = draft_dir(&ws, id);
    std::fs::create_dir_all(draft.join("draft.json")).unwrap();
    let err = init_draft_files(&ws, &profile).unwrap_err();
    assert!(err.contains("draft.json"), "实际：{err}");
    assert!(
        !draft.exists(),
        "失败应回滚整个 draft 目录（不留半成品挡死新向导）"
    );
    cleanup(&ws);
}

#[tokio::test]
async fn abort_refuses_during_reservation() {
    let ws = temp_ws("abort-res");
    let id = "zw-pro-abt1";
    let mgr = SetupManager::new();
    inject_reserved(&mgr, id);

    let err = mgr.abort(&ws).await.unwrap_err();
    assert!(err.contains("预占"), "实际：{err}");
    // 状态保留：select_target 继续持有预占，收尾写回不受干扰
    let st = mgr.status(&ws).unwrap();
    assert_eq!(st["profile_id"], id, "预占态应原样保留");
    cleanup(&ws);
}

#[tokio::test]
async fn abort_clears_rebuilt_state_from_disk() {
    // gateway 重启后 analyze 重建的 pid=0 态（reserved=false）必须照常可
    // abort——预占拒绝只针对 reserved=true 占位态，不误伤重建态合法路径。
    let ws = temp_ws("abort-rebuild");
    let id = "zw-pro-abt2";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    mgr.state.lock().unwrap().replace(SetupState {
        info: placeholder_info(id),
        analysis: None,
        reserved: false,
    });

    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id);
    assert!(mgr.state.lock().unwrap().is_none(), "abort 后状态应清空");
    assert!(!draft_dir(&ws, id).exists(), "draft 目录应已删除");
    cleanup(&ws);
}

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

// ── 清理失败诚实化（横扫族 C，2026-11-06）────────────────────────────────
// Windows 形态测试（编译期消失，Linux 打开句柄不阻碍删除）。失败注入：
// 持有句柄且 share_mode 不含 FILE_SHARE_DELETE → remove_dir_all 报
// sharing violation（ERROR 32），确定性失败。注意裸 File::open 挡不住——
// std 默认 share mode 含 FILE_SHARE_DELETE（实测实证），必须显式 share_mode。
// 失败注入会让 remove_dir_all_retry 走满 ~4.4s 退避窗口，两测各多耗数秒。

#[cfg(windows)]
#[tokio::test]
async fn abort_reports_draft_cleanup_failure_and_keeps_state() {
    let ws = temp_ws("abort-lock");
    let id = "zw-pro-abt3";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 失败注入：draft 目录内放一个被持有句柄的文件 → 删除必败
    let locked = draft_dir(&ws, id).join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2) // READ|WRITE，无 DELETE → 删除必撞 sharing violation
        .open(&locked)
        .unwrap();

    let err = mgr.abort(&ws).await.unwrap_err();
    assert!(err.contains("清理失败"), "实际：{err}");
    assert!(err.contains("可重试"), "实际：{err}");
    // 未收尾：状态保留（可重试 abort），draft 目录仍在
    assert!(
        mgr.state.lock().unwrap().is_some(),
        "清理失败不得清态（假成功）"
    );
    assert!(draft_dir(&ws, id).exists(), "draft 目录应保留待重试");

    // 释放句柄后重试 abort = 收敛成功（幂等可重试的实证）
    drop(handle);
    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id);
    assert!(mgr.state.lock().unwrap().is_none());
    assert!(!draft_dir(&ws, id).exists());
    cleanup(&ws);
}

#[cfg(windows)]
#[tokio::test]
async fn commit_reports_draft_cleanup_failure_in_warnings() {
    let ws = temp_ws("commit-lock");
    let id = "zw-pro-cmt4";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);
    // 补齐提交前置（同 commit_migrates_draft_to_profile）
    mgr.define_struct(
        &ws,
        &serde_json::json!({
            "name": "DataHeader", "size": 24,
            "fields": [{"offset": 0, "name": "magic", "type": "u32"}],
        }),
    )
    .unwrap();
    mgr.annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    mgr.define_tool(
        &ws,
        &serde_json::json!({
            "tool_name": "dummy_area_read",
            "description": "读",
            "action": {"primitive": "poll_report", "addr": "struct:DataHeader"},
        }),
    )
    .unwrap();

    // 失败注入：draft 目录内放一个被持有句柄的文件 → 删除必败
    let locked = draft_dir(&ws, id).join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2) // READ|WRITE，无 DELETE → 删除必撞 sharing violation
        .open(&locked)
        .unwrap();

    // save 已落盘不可回滚 → commit 不假失败，清理失败进返回值注记
    let out = mgr.commit(&ws).unwrap();
    assert_eq!(out["committed"], id);
    let warnings = out["cleanup_warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty()
            && warnings
                .iter()
                .any(|w| w.as_str().unwrap_or("").contains("draft")),
        "cleanup_warnings 应注记 draft 清理失败：{warnings:?}"
    );
    // 正式 profile 已提交可读回（提交本体不受清理降级影响）
    let store = ProfileStore::new(&ws);
    assert!(store.load(id).is_ok());

    drop(handle);
    cleanup(&ws);
}

#[cfg(windows)]
#[test]
fn rollback_draft_error_is_honest_about_rollback_failure() {
    // 横扫族 C 二轮：select_target_reserved 三处失败臂的「已回滚」断言
    // 不得谎报——回滚失败时错误串必须含「回滚失败」+ abort 收尸指引、
    // 不得含 ok_suffix；回滚成功时 base+ok_suffix 原样拼接。
    let ws = temp_ws("rb-lock");
    let id = "zw-pro-rb5";
    seed_draft(&ws, id);
    let draft = draft_dir(&ws, id);

    // 失败注入：同款 share_mode 剥 DELETE 位 → 删除必撞 sharing violation
    let locked = draft.join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&locked)
        .unwrap();

    let err = rollback_draft_error(&draft, "主错误原文".to_string(), "（draft 已回滚）");
    assert!(err.starts_with("主错误原文"), "主错误保首位：{err}");
    assert!(err.contains("回滚失败"), "回滚失败必须说话：{err}");
    assert!(err.contains("abort 收尸"), "应带收尸指引：{err}");
    assert!(!err.contains("已回滚"), "不得谎报已回滚：{err}");
    assert!(draft.exists(), "残留目录应保留待收尸");

    // 释放句柄后重试：回滚成功 → base+ok_suffix 拼接，目录删除
    drop(handle);
    let ok = rollback_draft_error(&draft, "主错误原文".to_string(), "（draft 已回滚）");
    assert_eq!(ok, "主错误原文（draft 已回滚）");
    assert!(!draft.exists());
    cleanup(&ws);
}
