//! [`vm_runtime`] 单测（CI 面：纯函数 + 文件系统 registry + config 现读 +
//! 禁用/孤儿收敛——**不 spawn QEMU**；真机链路归 `vm_smoke` 与实机 IT，
//! 分层见实施计划 §9）。

use std::time::Duration;

use super::*;

fn write_config(path: &std::path::Path, vm_json: &str) {
    std::fs::write(path, format!(r#"{{ "executor": {{ "vm": {vm_json} }} }}"#))
        .expect("write config");
}

fn full_vm_json(enabled: bool) -> String {
    format!(
        r#"{{
            "enabled": {enabled},
            "arch": "x86_64",
            "hypervisor": "auto",
            "desktop": "minimal",
            "image": {{ "source": "base.qcow2", "sha256": "" }},
            "resources": {{ "vcpu": 2, "ram_mb": 2048, "disk_gb": 20 }},
            "max_instances": 2,
            "idle_sleep_secs": 300,
            "gc": {{ "max_total_disk_gb": 50, "over_limit": "warn" }}
        }}"#
    )
}

fn sample_record(agent_id: &str) -> VmInstanceRecord {
    VmInstanceRecord {
        agent_id: agent_id.into(),
        overlay: PathBuf::from("/tmp/o.qcow2"),
        token: "a".repeat(32),
        sleep_mode: "snapshot".into(),
        snapshot_tag: Some("p5sleep".into()),
        state_path: None,
        asleep_at: Some(1700000000000),
        arch: "x86_64".into(),
        accel: "whpx".into(),
        vcpu: 2,
        ram_mb: 2048,
        disk_gb: 20,
    }
}

// ---------------------------------------------------------------------------
// 记录：serde round-trip + 冷停语义 + wake 路由
// ---------------------------------------------------------------------------

#[test]
fn record_roundtrip() {
    let r = sample_record("agent-1");
    let json = serde_json::to_string(&r).expect("serde");
    let back: VmInstanceRecord = serde_json::from_str(&json).expect("serde");
    assert_eq!(back, r);
}

/// 冷停语义：清 wake 产物 + asleep_at 置 Some + sleep_mode=cold。
#[test]
fn mark_cold_clears_wake_artifacts() {
    let mut r = sample_record("agent-1");
    r.state_path = Some(PathBuf::from("/tmp/vm.state"));
    r.sleep_mode = "migrate-file".into();
    mark_cold(&mut r);
    assert_eq!(r.sleep_mode, "cold");
    assert!(r.snapshot_tag.is_none());
    assert!(r.state_path.is_none());
    assert!(r.asleep_at.is_some(), "冷停后无活进程语义");
}

/// wake 路由表：快照线优先（tag 恒随 overlay 在），迁移线次之，全无 = 冷启。
#[test]
fn wake_kind_routing_table() {
    let mut r = sample_record("a");
    assert_eq!(
        wake_kind_of(&r),
        WakeKind::FromSnapshot("p5sleep".into()),
        "快照线优先"
    );
    r.snapshot_tag = None;
    r.state_path = Some(PathBuf::from("/x/vm.state"));
    assert_eq!(
        wake_kind_of(&r),
        WakeKind::FromState(PathBuf::from("/x/vm.state")),
        "迁移线次之"
    );
    r.state_path = None;
    assert_eq!(wake_kind_of(&r), WakeKind::Cold, "全无 = 冷启");
}

// ---------------------------------------------------------------------------
// pidfile / 身份核验
// ---------------------------------------------------------------------------

#[test]
fn pidfile_parse_shapes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let p = tmp.path().join("qemu.pid");
    assert!(parse_pidfile(&p).is_none(), "缺席文件 = None");
    std::fs::write(&p, "12345\n").expect("write");
    assert_eq!(parse_pidfile(&p), Some(12345));
    std::fs::write(&p, "not-a-pid").expect("write");
    assert!(parse_pidfile(&p).is_none(), "非数字 = None");
}

/// 身份核验语义：按「pid 的可执行镜像 ≟ QEMU 二进制」判定。本进程
/// pid + 本进程镜像 = 平凡正例（解析通路活）；同 pid 换不同镜像路径 =
/// 不匹配（pid 复用防误杀的实质）；进程缺席 = 不匹配。
#[test]
fn identity_check_matches_image_path() {
    let me = std::env::current_exe().expect("current_exe");
    assert!(
        pid_matches_qemu_identity(std::process::id(), &me),
        "本进程镜像比对通路必须通（正例锚）"
    );
    let other = me.with_file_name("definitely-not-this-binary.exe");
    assert!(
        !pid_matches_qemu_identity(std::process::id(), &other),
        "同 pid 不同镜像 = 不匹配（pid 复用防误杀）"
    );
    assert!(
        !pid_matches_qemu_identity(u32::MAX - 1, &me),
        "进程缺席 = 不匹配"
    );
}

/// paths_equiv canonicalize 兜底（8.3 短名误判「pid 复用」的回归锁）：
/// gateway 经 `std::env::temp_dir()` 拿到 `LOCALA~1` 短名 spawn 路径，而
/// `QueryFullProcessImageNameW` 返回 kernel 展开长名——字面比较必失配，
/// 会把真孤儿误判成 pid 复用而拒绝清理（VM-ON-2 实录）。用 canonicalize
/// 的 verbatim `\\?\` 形态作「字面失配但同指一文件」的确定性代理。
#[cfg(windows)]
#[test]
fn paths_equiv_canonicalize_fallback() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let f = tmp.path().join("qemu-system-x86_64.exe");
    std::fs::write(&f, b"stub").expect("write");

    // 字面等价快路径（大小写不敏感）仍通。
    assert!(paths_equiv(&f, &tmp.path().join("QEMU-SYSTEM-X86_64.EXE")));

    // verbatim 前缀形态：字面必失配（前提自检），兜底展开后判等。
    let verbatim = std::fs::canonicalize(&f).expect("canonicalize");
    assert_ne!(
        f.to_string_lossy().to_lowercase(),
        verbatim.to_string_lossy().to_lowercase(),
        "前提自检：verbatim 形态与普通路径字面不同（否则没测到兜底分支）"
    );
    assert!(
        paths_equiv(&f, &verbatim),
        "字面失配但同指一文件 → canonicalize 兜底判等（8.3 短名场景形态代理）"
    );

    // 兜底不可能成功的一侧（文件不存在）= 保守失配（宁可漏杀不误杀）。
    let missing = tmp.path().join("no-such-dir").join("no-such.exe");
    assert!(!paths_equiv(&f, &missing), "canonicalize 失败 = 保守拒绝");
}

// ---------------------------------------------------------------------------
// 布局 + registry 读写（文件系统）
// ---------------------------------------------------------------------------

#[test]
fn layout_paths_derive() {
    let ws = PathBuf::from("/ws");
    assert_eq!(
        registry_path(&ws, "agent-1"),
        PathBuf::from("/ws/tools/vm/run/agent-1/instance.json")
    );
    assert_eq!(
        overlay_path(&ws, "agent-1"),
        PathBuf::from("/ws/tools/vm/overlays/agent-1.qcow2")
    );
    assert!(vm_run_dir(&ws, "agent-1").starts_with(vm_run_root(&ws)));
}

/// 路径穿越闸：[`sanitize_path_segment`] 只管长度（≤80 逐字节不变），
/// 含分隔符/`..` 的 id 由 [`validate_agent_id`] 在操作入口 loud 拒绝——
/// 绝不让 `vm.instance.*` 的用户输入拼出逃出 run 根的路径。
#[test]
fn agent_id_gate_rejects_traversal() {
    assert!(validate_agent_id("3f9c2a6b7d").is_ok(), "uuid 形态恒过");
    assert!(validate_agent_id("main").is_ok());
    let evil = format!("..{}evil", std::path::MAIN_SEPARATOR);
    assert!(validate_agent_id(&evil).is_err());
    assert!(validate_agent_id("../evil").is_err());
    assert!(validate_agent_id("a/b").is_err());
    assert!(validate_agent_id("..").is_err());
    assert!(validate_agent_id(".").is_err());
    assert!(validate_agent_id("").is_err());
    // 闸拦截在 supervisor 操作面（错误分类 Failed=不回落）。
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let tmp = tempfile::tempdir().expect("tempdir");
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), tmp.path().join("config.json"));
    let err = rt
        .block_on(async { sup.stop_agent("../evil").await })
        .unwrap_err();
    assert!(matches!(err, VmSupervisorError::Failed(m) if m.contains("agent_id 非法")));
}

#[test]
fn registry_roundtrip_fs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    assert!(load_record(ws, "agent-1").is_none(), "无记录 = None");
    let r = sample_record("agent-1");
    save_record(ws, "agent-1", &r).expect("save record");
    let back = load_record(ws, "agent-1").expect("roundtrip");
    assert_eq!(back, r);
}

// ---------------------------------------------------------------------------
// GC：用量统计 + 决策表 + 满员闸
// ---------------------------------------------------------------------------

#[test]
fn gc_usage_sums_all_three_trees() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let gb = |n: u64| n * GIB; // 用大数绕过块大小对齐噪声——断言按 GiB 粒度
    std::fs::create_dir_all(vm_images_root(ws)).unwrap();
    std::fs::create_dir_all(vm_overlays_root(ws)).unwrap();
    std::fs::create_dir_all(vm_run_root(ws).join("a")).unwrap();
    std::fs::write(
        vm_images_root(ws).join("base.qcow2"),
        vec![0u8; 3 * 1024 * 1024 * 1024 / 1000],
    )
    .unwrap(); // ~3MB
    std::fs::write(vm_overlays_root(ws).join("a.qcow2"), vec![0u8; 1024]).unwrap();
    std::fs::write(vm_run_root(ws).join("a").join("vm.state"), vec![0u8; 512]).unwrap();
    let used = disk_usage_bytes(ws);
    assert!(
        used > 3 * 1024 * 1024 && used < gb(1),
        "三棵树求和（base 约 3MB + 零头）: {used}"
    );
    assert_eq!(disk_usage_bytes(&tmp.path().join("nope")), 0, "缺席树 = 0");
}

fn gc_cfg(max_gb: u32, over: &str) -> nemesis_config::VmGcConfig {
    nemesis_config::VmGcConfig {
        max_total_disk_gb: max_gb,
        over_limit: over.into(),
    }
}

/// GC 决策表（typed schema 的行为锚）：0=关；warn 档告警；stop 档拒新。
#[test]
fn gc_decision_table() {
    assert_eq!(
        gc_decision(GIB * 999, &gc_cfg(0, "warn")),
        GcDecision::Allow,
        "0=不设预算"
    );
    assert_eq!(
        gc_decision(GIB, &gc_cfg(2, "warn")),
        GcDecision::Allow,
        "预算内"
    );
    assert_eq!(
        gc_decision(GIB * 3, &gc_cfg(2, "warn")),
        GcDecision::Warn {
            used_gb: 3,
            budget_gb: 2
        }
    );
    assert_eq!(
        gc_decision(GIB * 3, &gc_cfg(2, "stop")),
        GcDecision::Deny {
            used_gb: 3,
            budget_gb: 2
        }
    );
}

/// 满员闸判定：max=0 = 恒满（诚实拒新——配置为 0 不该悄悄放行）。
#[test]
fn capacity_gate_table() {
    assert!(capacity_full(0, 0), "max_instances=0 = 恒满");
    assert!(capacity_full(2, 2));
    assert!(!capacity_full(1, 2));
}

// ---------------------------------------------------------------------------
// config 现读 + 闸行为（不 spawn QEMU 的 supervisor 面）
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vm_settings_reads_and_gates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg.clone());

    std::fs::write(&cfg, "{}").expect("write");
    assert!(sup.vm_settings().is_none(), "无 executor.vm 段 = None");
    write_config(&cfg, "null");
    assert!(sup.vm_settings().is_none(), "损坏形态 = None");
    write_config(&cfg, &full_vm_json(false));
    assert!(
        sup.vm_settings().is_none(),
        "enabled=false = None（零激活铁律）"
    );
    write_config(&cfg, &full_vm_json(true));
    let s = sup.vm_settings().expect("enabled");
    assert_eq!(s.max_instances, 2);
    assert_eq!(s.resources.ram_mb, 2048);
    assert_eq!(s.idle_sleep_secs, 300);
}

/// 关闭态零激活：ensure_running / sleep_agent 全部诚实 Unavailable。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_config_every_operation_unavailable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    write_config(&cfg, &full_vm_json(false));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg);
    // match 而非 unwrap_err（VmSlot 无 Debug——成功臂不该进 Debug 格式化）。
    let err = match sup.ensure_running("agent-1").await {
        Err(e) => e,
        Ok(_) => panic!("enabled=false 时 ensure_running 必须不可用"),
    };
    assert!(
        matches!(&err, VmSupervisorError::Unavailable(m) if m.contains("enabled=false")),
        "{err:?}"
    );
    let err = sup.sleep_agent("agent-1").await.unwrap_err();
    assert!(matches!(err, VmSupervisorError::Unavailable(_)), "{err:?}");
}

/// config 整体缺席（headless 测试形态）= 同样诚实不可用。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_config_is_unavailable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sup = VmSupervisor::new(
        tmp.path().to_path_buf(),
        tmp.path().join("no-such-config.json"),
    );
    // match 而非 unwrap_err（VmSlot 无 Debug）。
    let err = match sup.ensure_running("agent-1").await {
        Err(e) => e,
        Ok(_) => panic!("config 缺席时 ensure_running 必须不可用"),
    };
    assert!(matches!(err, VmSupervisorError::Unavailable(_)), "{err:?}");
}

/// stop_agent 无槽 = Failed（调用已发生但目标不存在——不是档不可用）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_without_slot_is_failed_not_unavailable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    write_config(&cfg, &full_vm_json(true));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg);
    let err = sup.stop_agent("agent-1").await.unwrap_err();
    assert!(
        matches!(err, VmSupervisorError::Failed(_)),
        "无槽是操作面错误而非档不可用: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// F2（2026-10-21 三轮复评）：逐出两段式状态机（无须 QEMU——无槽形态下
// prepare 纯状态转换；断言面 = ensure_running 的生命周期拒绝文案）。
// ---------------------------------------------------------------------------

/// 唤醒并取错误串（测试环境槽/镜像缺席，不应真能启动）。
async fn wake_err_string(sup: &VmSupervisor, id: &str) -> String {
    match sup.ensure_running(id).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("测试环境（槽/镜像缺席）不应真能启动 VM"),
    }
}

/// prepare → 临时拒绝态生效（「删除在途」）；abort → 解除（回到非删除
/// 语义）；再 prepare → commit → 永久墓碑（「已删除」）；commit 后 abort
/// 幂等不得复活身份。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evict_two_phase_state_machine_without_slot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    write_config(&cfg, &full_vm_json(true));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg);

    // ① prepare：进临时拒绝态。
    sup.evict_agent_prepare("agent-a").await.unwrap();
    let err = wake_err_string(&sup, "agent-a").await;
    assert!(
        err.contains("删除在途"),
        "prepare 后临时拒绝态必须生效: {err}"
    );

    // ② abort：临时态解除，身份回到可唤醒语义（此处形态 = config 侧诚实
    // 报错，绝不再携带删除字样）。
    sup.evict_agent_abort("agent-a");
    let err = wake_err_string(&sup, "agent-a").await;
    assert!(
        !err.contains("删除在途") && !err.contains("已删除"),
        "abort 后身份必须恢复可唤醒语义: {err}"
    );

    // ③ 再 prepare → commit：永久墓碑落地。
    sup.evict_agent_prepare("agent-a").await.unwrap();
    sup.evict_agent_commit("agent-a");
    let err = wake_err_string(&sup, "agent-a").await;
    assert!(err.contains("已删除"), "commit 后永久墓碑必须生效: {err}");

    // ④ 幂等 + 顺序安全：commit 后 abort 不得清掉墓碑复活身份。
    sup.evict_agent_commit("agent-a");
    sup.evict_agent_abort("agent-a");
    let err = wake_err_string(&sup, "agent-a").await;
    assert!(
        err.contains("已删除"),
        "commit 后的 abort 不得复活身份: {err}"
    );
}

// ---------------------------------------------------------------------------
// 孤儿 sweep：清理不收编的收敛面（死 pid / 启动保护窗）
// ---------------------------------------------------------------------------

/// 把文件 mtime 拨老（绕过 [`SWEEP_STARTUP_GRACE`] 启动保护窗——测试不想
/// 等 120s；`File::set_modified` 稳定 std 面）。
fn backdate(path: &std::path::Path, age: Duration) {
    use std::io::Write as _;
    let past = std::time::SystemTime::now()
        .checked_sub(age)
        .expect("backdate offset");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for backdate");
    f.set_modified(past).expect("set_modified");
    f.flush().ok();
}

/// 「应在运行」+ pid 已死 → 只转冷停、不杀任何进程。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_marks_dead_record_cold() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let mut r = sample_record("agent-1");
    r.asleep_at = None; // 应在运行
    r.overlay = overlay_path(ws, "agent-1");
    save_record(ws, "agent-1", &r).expect("save record");
    backdate(&registry_path(ws, "agent-1"), Duration::from_secs(300));

    let sup = VmSupervisor::new(ws.to_path_buf(), ws.join("config.json"));
    let swept = sup.sweep_orphans().await;
    assert_eq!(swept, 0, "pid 已死/无 pidfile：零进程被杀");
    let after = load_record(ws, "agent-1").expect("record");
    assert_eq!(after.sleep_mode, "cold");
    assert!(after.snapshot_tag.is_none() && after.state_path.is_none());
    assert!(after.asleep_at.is_some());
}

/// 已睡眠记录（asleep_at=Some）+ 启动保护窗内记录：sweep 都不碰。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_skips_sleeping_and_recent_records() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    // 睡眠中记录。
    let mut sleeping = sample_record("agent-sleep");
    sleeping.overlay = overlay_path(ws, "agent-sleep");
    save_record(ws, "agent-sleep", &sleeping).expect("save record");
    // 刚写的「应在运行」记录（保护窗内——不 backdate）。
    let mut fresh = sample_record("agent-fresh");
    fresh.asleep_at = None;
    fresh.overlay = overlay_path(ws, "agent-fresh");
    save_record(ws, "agent-fresh", &fresh).expect("save record");

    let sup = VmSupervisor::new(ws.to_path_buf(), ws.join("config.json"));
    let swept = sup.sweep_orphans().await;
    assert_eq!(swept, 0);
    // 睡眠记录原样保留。
    let kept = load_record(ws, "agent-sleep").expect("record");
    assert_eq!(kept.sleep_mode, "snapshot");
    assert!(kept.snapshot_tag.is_some());
    // 保护窗内的记录不动（fresh 刚写过，mtime 新）。
    let kept = load_record(ws, "agent-fresh").expect("record");
    assert_eq!(kept.sleep_mode, "snapshot", "保护窗内不转冷停");
    assert!(kept.snapshot_tag.is_some());
}

// ---------------------------------------------------------------------------
// accel / arch 字符串 round-trip（与 guest_arch 值域同源锚定）
// ---------------------------------------------------------------------------

#[test]
fn accel_parse_as_str_roundtrip() {
    for a in QemuAccel::ALL {
        assert_eq!(parse_accel(a.as_str()), Some(*a));
    }
    assert_eq!(parse_accel(""), None);
    assert_eq!(parse_accel("tcg"), None, "TCG 不在值域（诚实降级铁律）");
    assert!(GuestArch::parse("x86_64").is_ok());
    assert!(GuestArch::parse("mips").is_err(), "未知值 loud 拒绝");
}

// ---------------------------------------------------------------------------
// Windows 身份核验正例（本进程镜像 = 自己，QueryFullProcessImageNameW 通）
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn process_image_path_resolves_own_pid() {
    let me = std::env::current_exe().expect("current_exe");
    let got = process_image_path(std::process::id()).expect("own pid resolvable");
    assert_eq!(
        got.to_string_lossy().to_lowercase(),
        me.to_string_lossy().to_lowercase()
    );
}

// ---------------------------------------------------------------------------
// P5 第 4 步：执行接入面——guest 子集闸（先于一切 config/IO）
// ---------------------------------------------------------------------------

/// 子集闸双臂：超子集工具（grep）= ToolFailed **不回落**（VM 与宿主两套
/// 文件视图混用 = 正确性事故）；子集内工具（exec）在 config 关 = Unavailable
/// 且 host_fallback=false（R1：vm 档 guest 异文件视图，宿主回落被错误位
/// 禁止——「可安全回落」只对 process 档成立）。两臂都无需 QEMU/镜像——闸
/// 先于一切 IO。
#[test]
fn persistent_execution_subset_gate_and_config_off() {
    use nemesis_agent::remote_executor_tool::{PersistentCallError, PersistentExecutor};

    let rt = tokio::runtime::Runtime::new().expect("rt");
    let tmp = tempfile::tempdir().expect("tempdir");
    write_config(&tmp.path().join("config.json"), &full_vm_json(false));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), tmp.path().join("config.json"));
    let ctx = nemesis_agent::context::RequestContext::new("web", "c1", "u", "s1");
    let exec_root = tmp.path().join("exec");

    // 子集闸：grep 不在 guest 子集 → ToolFailed（绝不回落）。
    let err = rt
        .block_on(async {
            sup.execute_persistent("agent-1", &exec_root, "grep", "{}", &ctx)
                .await
        })
        .unwrap_err();
    assert!(
        matches!(err, PersistentCallError::ToolFailed(ref m) if m.contains("子集")),
        "grep 必须被子集闸拦成 ToolFailed: {err:?}"
    );
    // git 同表（8 工具之外的最常见高危混用项）。
    let err = rt
        .block_on(async {
            sup.execute_persistent("agent-1", &exec_root, "git", "{}", &ctx)
                .await
        })
        .unwrap_err();
    assert!(matches!(err, PersistentCallError::ToolFailed(_)));

    // 子集内 + config 关 → Unavailable 且 host_fallback=false（vm 档禁宿主回落）。
    let err = rt
        .block_on(async {
            sup.execute_persistent("agent-1", &exec_root, "exec", "{}", &ctx)
                .await
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            PersistentCallError::Unavailable {
                ref message,
                host_fallback: false,
            } if message.contains("enabled=false")
        ),
        "config 关必须 Unavailable 且 host_fallback=false: {err:?}"
    );
}

// R1 回归锁（原 review 探针反转，2026-10-20）：VM enabled 但镜像缺席时，
// vm 档不可用错误携带 host_fallback=false——RemoteExecutorTool 必须上抛诚实
// 错误（含回落禁用说明），绝不静默落宿主 per-call 通道（宿主标记文件不可被
// 读出）。通道 exe 指向不存在路径：正确实现下永不被触碰；若回落（回归），
// spawn 失败错误形态立即可辨。
#[cfg(windows)]
#[tokio::test]
async fn unavailable_vm_does_not_fallback_to_host() {
    use nemesis_agent::r#loop::Tool;
    use nemesis_agent::remote_executor_tool::{
        ExecutorChannel, PersistentRoute, RemoteExecutorTool,
    };
    // 全隔离（tempdir home/config，无进程级全局态操纵——翻转后不再需要
    // GLOBAL_STATE_LOCK，探针时代的 env 依赖已随 NEMESISBOT_TEST_BIN 退场）。
    let tmp = tempfile::tempdir().unwrap();
    write_config(&tmp.path().join("config.json"), &full_vm_json(true));
    std::fs::write(
        tmp.path().join("host-only-marker.txt"),
        "REVIEW_HOST_ONLY_MARKER",
    )
    .unwrap();
    let sup = std::sync::Arc::new(VmSupervisor::new(
        tmp.path().to_path_buf(),
        tmp.path().join("config.json"),
    ));
    let channel = std::sync::Arc::new(ExecutorChannel::new(
        tmp.path().join("no-such-executor.exe"),
        tmp.path().to_string_lossy().to_string(),
        std::sync::Arc::new(|| false),
    ));
    let root = tmp.path().to_path_buf();
    let tool = RemoteExecutorTool::new(
        "read_file".into(),
        Box::new(nemesis_agent::loop_tools::ExecTool::new(
            &root.to_string_lossy(),
            true,
        )),
        channel,
    )
    .with_persistent(PersistentRoute {
        resolve: std::sync::Arc::new(move || Some(("review-vm".into(), root.clone()))),
        executor: sup,
    });
    let ctx = nemesis_agent::context::RequestContext::new("web", "review", "tester", "review");
    let err = tool
        .execute(r#"{"path":"host-only-marker.txt"}"#, &ctx)
        .await
        .expect_err("vm 档不可用必须上抛诚实错误，不得回落宿主读出标记");
    assert!(
        err.contains("host fallback forbidden") && err.contains("不回落宿主"),
        "错误须含回落禁用契约文案: {err}"
    );
    assert!(
        !err.contains("REVIEW_HOST_ONLY_MARKER"),
        "宿主文件内容绝不可出现在结果中: {err}"
    );
    assert!(
        !err.contains("failed to spawn"),
        "per-call 通道必须一次都未被触碰: {err}"
    );
}

/// B9-1 内部方法通道双钉（gateway 同步引擎的闸位契约）：
/// ① `internal_call` **不走** agent 工具面子集闸——bulk_write 等内部方法
///   未就绪环境的失败必须是诚实 Unavailable（可回落），错误里绝不出现
///   「子集」措辞（闸位置钉死：子集闸永远拦不到同步引擎）；
/// ② `execute_persistent`（agent 工具面）**必须拦**内部方法——agent 拿到
///   bulk_write 名字也进不了同步通道。
#[test]
fn internal_call_gate_position_pins() {
    use nemesis_agent::remote_executor_tool::{PersistentCallError, PersistentExecutor};

    let rt = tokio::runtime::Runtime::new().expect("rt");
    let tmp = tempfile::tempdir().expect("tempdir");
    write_config(&tmp.path().join("config.json"), &full_vm_json(false));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), tmp.path().join("config.json"));
    let ctx = nemesis_agent::context::RequestContext::new("web", "c1", "u", "s1");
    let exec_root = tmp.path().join("exec");

    // ① internal_call：config 关 → ensure_running Unavailable（未发出请求）。
    let err = rt
        .block_on(sup.internal_call(
            "agent-1",
            "bulk_write",
            serde_json::json!({"transfer": "t", "seq": 0, "total": 1, "data": ""}),
        ))
        .unwrap_err();
    let msg = match &err {
        VmSupervisorError::Unavailable(m) => m.clone(),
        other => panic!("internal_call 必须走 Unavailable 臂: {other:?}"),
    };
    assert!(msg.contains("enabled=false"), "{msg}");
    assert!(
        !msg.contains("子集"),
        "internal_call 不得被子集闸拦截: {msg}"
    );
    assert!(!msg.contains("VM_GUEST_TOOLS"), "{msg}");

    // ② agent 工具面对内部方法恒关：execute_persistent("bulk_write") →
    //    ToolFailed（不回落），文案点名不在 guest 子集。
    let err = rt
        .block_on(async {
            sup.execute_persistent("agent-1", &exec_root, "bulk_write", "{}", &ctx)
                .await
        })
        .unwrap_err();
    assert!(
        matches!(err, PersistentCallError::ToolFailed(ref m) if m.contains("子集")),
        "bulk_write 必须被子集闸拦成 ToolFailed: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// B6 快速试玩族（VmBridge trait 面；真机链路仍归 vm_smoke——这里钉闸）
// ---------------------------------------------------------------------------

#[test]
fn try_family_off_state_gates() {
    use nemesis_web::handlers::vm::VmBridge;

    let rt = tokio::runtime::Runtime::new().expect("rt");
    let tmp = tempfile::tempdir().expect("tempdir");
    write_config(&tmp.path().join("config.json"), &full_vm_json(false));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), tmp.path().join("config.json"));

    // status 只读探针：关闭态如实可见（对齐 bootstrap.status 纪律——查询不是动作）。
    let st = sup.try_status();
    assert_eq!(st["enabled"], false);
    assert_eq!(st["running"], false);
    assert_eq!(st["agent_id"], "try-dashboard");
    assert_eq!(st["image_ready"], false, "无镜像目录 = 未就绪");

    // start / exec：enabled 闸诚实拒绝（off-state 零激活铁律）。
    let err = rt.block_on(sup.try_start()).unwrap_err();
    assert!(err.contains("总开关"), "start 必须被 enabled 闸拦: {err}");
    let err = rt.block_on(sup.try_exec("uname -a", None)).unwrap_err();
    assert!(err.contains("总开关"), "exec 必须被 enabled 闸拦: {err}");

    // stop = 停机方向：无闸，未运行 = 幂等 Ok（试玩卡停止按钮不报错）。
    let res = rt.block_on(sup.try_stop()).expect("stop 幂等");
    assert_eq!(res["running"], false);
    assert!(res["note"].as_str().unwrap().contains("未在运行"));
}

#[test]
fn try_family_enabled_gates_arch_then_image() {
    use nemesis_web::handlers::vm::VmBridge;

    let rt = tokio::runtime::Runtime::new().expect("rt");
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");

    // aarch64 档：诚实不可用（接口预留——镜像管线尚无产物）。
    let aarch64 = full_vm_json(true).replace("\"x86_64\"", "\"aarch64\"");
    write_config(&cfg, &aarch64);
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg.clone());
    let st = sup.try_status();
    assert_eq!(st["arch"], "aarch64");
    assert_eq!(st["arch_supported"], false);
    let err = rt.block_on(sup.try_start()).unwrap_err();
    assert!(
        err.contains("x86_64") && err.contains("预留"),
        "arch 闸: {err}"
    );

    // x86_64 档但镜像缺席：指路环境安装卡。
    write_config(&cfg, &full_vm_json(true));
    let err = rt.block_on(sup.try_start()).unwrap_err();
    assert!(
        err.contains("base.qcow2") && err.contains("环境安装"),
        "镜像闸指路: {err}"
    );
    let err = rt.block_on(sup.try_exec("uname -a", None)).unwrap_err();
    assert!(err.contains("base.qcow2"), "exec 同过镜像闸: {err}");

    // 镜像齐（占位文件）→ status 就绪转绿（真启动仍归 vm_smoke 真机面）。
    // seed 已烧进 base（无独立介质），image_ready 只看 base.qcow2。
    let images = tmp.path().join("tools").join("vm").join("images");
    std::fs::create_dir_all(&images).expect("images dir");
    std::fs::write(images.join("base.qcow2"), b"stub").expect("base");
    let st = sup.try_status();
    assert_eq!(st["image_ready"], true);
    assert_eq!(st["arch_supported"], true);
}

// ---------------------------------------------------------------------------
// B6 热修（try.start 发起即返回）：启动在途状态机 + 在途闸
// （真机教训 2026-10-19：冷启动同步等 30-60s 撞前端 30s WSAPI 超时）
// ---------------------------------------------------------------------------

#[test]
fn try_launch_claim_state_machine() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_config(&tmp.path().join("config.json"), &full_vm_json(false));

    // 无句柄（未 set_self_arc）：认领 = None + 状态回滚（同步回落语义）。
    let plain = VmSupervisor::new(tmp.path().to_path_buf(), tmp.path().join("config.json"));
    assert!(plain.try_launch_claim().is_none());
    let (starting, err) = plain.try_launch_snapshot();
    assert!(!starting, "回滚后不得残留在途位");
    assert!(err.is_none());

    // 有句柄：认领返回自引用；在途期间二次认领幂等 None。
    let sup = std::sync::Arc::new(VmSupervisor::new(
        tmp.path().to_path_buf(),
        tmp.path().join("config.json"),
    ));
    sup.set_self_arc();
    let owned = sup.try_launch_claim().expect("首次认领成功");
    assert!(std::sync::Arc::ptr_eq(&owned, &sup));
    assert!(sup.try_launch_claim().is_none(), "在途幂等");

    // 在途停止登记（try.stop 落点）→ 收尾翻牌带错 → 消费（读后清）。
    assert!(sup.try_launch_stop_if_starting());
    sup.try_launch_finish(Some("炸了".to_string()));
    let (starting, err) = sup.try_launch_snapshot();
    assert!(!starting);
    assert_eq!(err.as_deref(), Some("炸了"));
    assert!(
        sup.try_launch_take_stop_request(),
        "在途期登记的停止请求可消费"
    );
    assert!(!sup.try_launch_take_stop_request(), "读后清");
    assert!(!sup.try_launch_stop_if_starting(), "收尾后不再接受停止登记");

    // 成功收尾清错。
    sup.try_launch_finish(None);
    assert!(sup.try_launch_snapshot().1.is_none());
}

#[test]
fn try_family_starting_gates_exec_and_stop_lands_request() {
    use nemesis_web::handlers::vm::VmBridge;

    let rt = tokio::runtime::Runtime::new().expect("rt");
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    write_config(&cfg, &full_vm_json(true));
    let images = tmp.path().join("tools").join("vm").join("images");
    std::fs::create_dir_all(&images).expect("images dir");
    std::fs::write(images.join("base.qcow2"), b"stub").expect("base");
    let sup = std::sync::Arc::new(VmSupervisor::new(tmp.path().to_path_buf(), cfg));
    sup.set_self_arc();

    // 模拟在途（真启动归 vm_smoke 真机面）：认领置位。
    assert!(sup.try_launch_claim().is_some());

    // status 投影在途可见（前端轮询契约：starting + running 两字段）。
    let st = sup.try_status();
    assert_eq!(st["running"], false);
    assert_eq!(st["starting"], true);

    // exec 在途诚实拒绝（并发 ensure 双开 QEMU 抢同一 overlay 的 qcow2 锁）。
    let err = rt.block_on(sup.try_exec("uname -a", None)).unwrap_err();
    assert!(err.contains("启动中"), "exec 在途闸: {err}");

    // stop 在途 = 登记请求（Ok + 注记），不报错。
    let res = rt.block_on(sup.try_stop()).expect("stop 在途登记");
    assert_eq!(res["starting"], true);
    assert!(res["note"].as_str().unwrap().contains("自动停机"));
    assert!(sup.try_launch_take_stop_request(), "请求已登记");

    // 收尾（失败形态）：starting 翻 false + last_error 透出（轮询侧展示）。
    sup.try_launch_finish(Some("不可用: 加速器缺席".to_string()));
    let st = sup.try_status();
    assert_eq!(st["starting"], false);
    assert_eq!(st["last_error"], "不可用: 加速器缺席");
}

// ---------------------------------------------------------------------------
// 启动在途闸（starting 集）：同 agent 并发 ensure 互斥 + 失败路径 Drop 释放
// + sweep 跳过在途 agent（2026-10-20 复查批回归锁）
// ---------------------------------------------------------------------------

/// 在途拒绝 + 失败路径自动释放：hypervisor 非法值让 start_or_wake 在 spawn
/// 前确定性失败——第一次拿到的是根因错误（非「启动中」）；守卫 Drop 后
/// starting 集清空，第二次调用重新拿闸、报同样的根因错误（而非闸拒绝）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_gate_released_on_failure_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    // full_vm_json 的 hypervisor=auto 换成非法值：parse_hypervisor 在 spawn
    // 前诚实失败（registry 已落、QEMU 未起）——守卫释放路径的确定性载体。
    let bogus = full_vm_json(true).replace(r#""hypervisor": "auto""#, r#""hypervisor": "bogus""#);
    write_config(&cfg, &bogus);
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg);

    let err1 = match sup.ensure_running("agent-1").await {
        Err(e) => e,
        Ok(_) => panic!("hypervisor 非法时启动必须失败"),
    };
    assert!(
        matches!(&err1, VmSupervisorError::Failed(m) if m.contains("hypervisor")),
        "第一次 = 根因错误而非闸拒绝: {err1:?}"
    );
    assert!(
        sup.starting.lock().expect("starting").is_empty(),
        "失败路径守卫必须已释放 starting 集"
    );

    let err2 = match sup.ensure_running("agent-1").await {
        Err(e) => e,
        Ok(_) => panic!("第二次同样必须失败"),
    };
    assert!(
        matches!(&err2, VmSupervisorError::Failed(m) if m.contains("hypervisor")),
        "第二次应重新拿闸报同一根因（而非「正在启动中」）: {err2:?}"
    );
}

/// 同 agent 在途（并发调用方已持闸）= 诚实 Unavailable 拒绝；异 agent 不受
/// 影响（闸是 per-agent 的，不是全局）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_gate_blocks_same_agent_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = tmp.path().join("config.json");
    write_config(&cfg, &full_vm_json(true));
    let sup = VmSupervisor::new(tmp.path().to_path_buf(), cfg);

    // 模拟并发在途（真启动归 vm_smoke 真机面）。
    sup.starting
        .lock()
        .expect("starting")
        .insert("agent-1".to_string());

    let err = match sup.ensure_running("agent-1").await {
        Err(e) => e,
        Ok(_) => panic!("同 agent 在途时 ensure 必须被闸拒绝"),
    };
    assert!(
        matches!(&err, VmSupervisorError::Unavailable(m) if m.contains("正在启动中")),
        "同 agent 在途 = Unavailable 闸拒绝: {err:?}"
    );
    // 清闸后异 agent 不被 agent-1 的在途闸连坐（走到自己后续的真实失败点：
    // 加速裁决/镜像资产缺失——按宿主环境而定，但绝不是「正在启动中」）。
    sup.starting.lock().expect("starting").clear();
    let err2 = match sup.ensure_running("agent-2").await {
        Err(e) => e,
        Ok(_) => panic!("无镜像/无加速的空 workspace 启动必须失败"),
    };
    assert!(
        !matches!(&err2, VmSupervisorError::Unavailable(m) if m.contains("正在启动中")),
        "异 agent 不得被同 agent 在途闸连坐: {err2:?}"
    );
}

/// sweep 跳过在途 agent：记录「应在运行」+ mtime 拨老过保护窗（排除 mtime
/// 宽限的干扰）+ 在途集持有 → 记录原样保留（不被误转冷停/误杀）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_skips_inflight_starting_agent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let mut r = sample_record("agent-1");
    r.asleep_at = None; // 「应在运行」——无在途闸时 sweep 会动它
    r.overlay = overlay_path(ws, "agent-1");
    save_record(ws, "agent-1", &r).expect("save record");
    backdate(&registry_path(ws, "agent-1"), Duration::from_secs(300));

    let sup = VmSupervisor::new(ws.to_path_buf(), ws.join("config.json"));
    sup.starting
        .lock()
        .expect("starting")
        .insert("agent-1".to_string());

    let swept = sup.sweep_orphans().await;
    assert_eq!(swept, 0, "在途 agent 不是孤儿");
    let after = load_record(ws, "agent-1").expect("record");
    assert_eq!(after, r, "在途启动的记录必须原样保留（未被转冷停）");
}

// ---------------------------------------------------------------------------
// 第六轮复查（2026-10-20）：ensure 路由顺序（活槽短路拒新闸）+ registry
// 原子写（spawn 前锚如实报错）+ spawn 前活 QEMU 防线
// ---------------------------------------------------------------------------

/// 路由顺序表：活槽短路一切「拒新」闸（GC stop/满员都不该拦已在跑的
/// 实例——拒它 = Unavailable → 宿主回落混两套文件视图）；无槽时 GC stop
/// 闸先于满员闸（磁盘语义优先）；warn 档超预算只告警不拒。
#[test]
fn ensure_route_order_table() {
    let deny = gc_cfg(2, "stop");

    // 活槽：预算爆 + 满员，仍然 Reuse（修前 GC 闸在槽检查之前，会先把
    // 活实例拒成 Unavailable）。
    assert_eq!(
        ensure_route(true, GIB * 99, &deny, 4, 2),
        EnsureRoute::Reuse,
        "活槽短路 GC stop 闸与满员闸"
    );

    // 无槽 + 超预算 stop = GC 拒新（先于满员闸）。
    assert_eq!(
        ensure_route(false, GIB * 3, &deny, 4, 2),
        EnsureRoute::GcDeny {
            used_gb: 3,
            budget_gb: 2
        }
    );

    // 无槽 + warn 档超预算：不拒（只告警）→ 满员闸接住。
    assert_eq!(
        ensure_route(false, GIB * 3, &gc_cfg(2, "warn"), 4, 2),
        EnsureRoute::CapFull { max_instances: 2 }
    );

    // 无槽 + 预算内 + 满员 = CapFull；未满 = Proceed。
    assert_eq!(
        ensure_route(false, GIB, &deny, 2, 2),
        EnsureRoute::CapFull { max_instances: 2 }
    );
    assert_eq!(ensure_route(false, GIB, &deny, 1, 2), EnsureRoute::Proceed);

    // 无预算闸（max=0）恒放行/短路形态。
    let off = gc_cfg(0, "stop");
    assert_eq!(
        ensure_route(false, GIB * 999, &off, 0, 1),
        EnsureRoute::Proceed
    );
    assert_eq!(
        ensure_route(false, GIB * 999, &off, 1, 1),
        EnsureRoute::CapFull { max_instances: 1 }
    );
}

/// registry 原子写：roundtrip + 成功路径 run 目录只剩 instance.json（无
/// 孤儿 .tmp——临时文件已被 rename 消费；修前裸 write 失败会留半截 JSON）。
#[test]
fn registry_save_roundtrip_leaves_no_tmp() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let r = sample_record("agent-1");
    save_record(ws, "agent-1", &r).expect("save");
    let mut r2 = r.clone();
    r2.asleep_at = Some(42);
    save_record(ws, "agent-1", &r2).expect("save again（tmp 名唯一，连续写不互踩）");
    assert_eq!(load_record(ws, "agent-1"), Some(r2));

    let entries: Vec<String> = std::fs::read_dir(vm_run_dir(ws, "agent-1"))
        .expect("run dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries,
        vec!["instance.json".to_string()],
        "成功路径不留孤儿 tmp: {entries:?}"
    );
}

/// registry 写失败必须如实报 Err（spawn 前锚 `?` 消费的结构前提）：
/// run/<agent> 被同名文件占位 → create_dir_all 失败 = Err（修前吞错只
/// warn，锚位形同虚设——spawn 照跑，崩溃一致性保证为空）。
#[test]
fn registry_save_error_is_reported_not_swallowed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let run_dir = vm_run_dir(ws, "agent-1");
    std::fs::create_dir_all(run_dir.parent().expect("run root")).expect("run root");
    std::fs::write(&run_dir, b"occupied").expect("占位 run/<agent>");
    let err = save_record(ws, "agent-1", &sample_record("agent-1")).unwrap_err();
    assert!(err.contains("registry"), "报错要点名 registry: {err}");
}

/// spawn 前活 QEMU 防线（[`ensure_no_live_qemu`]）：pidfile 指向身份核验
/// 通过的活进程（本测试进程自身 = 平凡活体，镜像比对用 current_exe 平凡
/// 匹配）→ 拒绝；pid 已死 / 身份不匹配 / pidfile 缺席 → 放行（不误拦正常
/// 冷启/唤醒——QEMU 退出后 pidfile 残留是常态）。
#[test]
fn ensure_no_live_qemu_gate_table() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pidfile = tmp.path().join("qemu.pid");
    let me = std::env::current_exe().expect("current_exe");

    // 活体 + 镜像匹配（本进程）= 拒绝（双开防线主臂）。
    std::fs::write(&pidfile, format!("{}\n", std::process::id())).expect("write pid");
    let err = ensure_no_live_qemu(&pidfile, &me).unwrap_err();
    assert!(err.contains("禁止双开"), "{err}");

    // 身份不匹配（同活 pid、异镜像）= 放行（pid 复用保守 doctrine）。
    let other = me.with_file_name("definitely-not-this-binary");
    ensure_no_live_qemu(&pidfile, &other).expect("身份不匹配不拦");

    // pid 已死 = 放行（正常冷启/唤醒形态）。
    std::fs::write(&pidfile, format!("{}\n", u32::MAX - 1)).expect("write dead pid");
    ensure_no_live_qemu(&pidfile, &me).expect("死 pid 不拦");

    // pidfile 缺席 = 放行。
    let missing = tmp.path().join("no-such.pid");
    ensure_no_live_qemu(&missing, &me).expect("无 pidfile 不拦");
}

/// 全 workspace 活 QEMU 扫描核（[`any_live_qemu_scan`] 可注入形态四态表，
/// 镜像换装闸 promote 侧的判定核）：run 根缺席 / pid 已死 / 身份不匹配 /
/// run 根下的散文件 = 无活；任一 agent 目录身份命中 = 有活（跨进程哨兵
/// ——CLI 前台臂 / 另一 gateway 进程的活实例同样可见）。镜像解析用
/// current_exe 注入（本测试进程自身 = 平凡活体；自然解析在 Windows 上
/// 解析出的是裸名，与测试 exe 镜像路径永不相等，故必须注入）。
#[test]
fn any_live_qemu_scan_table() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path();
    let me = std::env::current_exe().expect("current_exe");
    let other = me.with_file_name("definitely-not-this-binary");

    // run 根缺席 = 无活（空 workspace 形态）。
    assert!(!any_live_qemu_scan(ws, &|_| me.clone()));

    // 死 pid = 无活（正常冷停后 pidfile 残留形态）。
    let run1 = vm_run_dir(ws, "agent-1");
    std::fs::create_dir_all(&run1).expect("run dir 1");
    std::fs::write(run1.join("qemu.pid"), format!("{}\n", u32::MAX - 1)).expect("dead pid");
    assert!(!any_live_qemu_scan(ws, &|_| me.clone()), "死 pid 不算活");

    // 活体 + 镜像不匹配 = 无活（pid 复用保守 doctrine，与
    // ensure_no_live_qemu 同一口径）。
    std::fs::write(run1.join("qemu.pid"), format!("{}\n", std::process::id())).expect("live pid");
    assert!(
        !any_live_qemu_scan(ws, &|_| other.clone()),
        "身份不匹配不算活"
    );

    // 活体 + 镜像匹配 = 有活。
    assert!(any_live_qemu_scan(ws, &|_| me.clone()), "身份命中必须有活");

    // 多目录扫描不得早退：agent-1 改回死 pid，agent-2 身份命中——无论
    // read_dir 顺序如何（平台实现细节），唯一命中必须被报真。
    std::fs::write(run1.join("qemu.pid"), format!("{}\n", u32::MAX - 1)).expect("dead pid 1");
    let run2 = vm_run_dir(ws, "agent-2");
    std::fs::create_dir_all(&run2).expect("run dir 2");
    std::fs::write(run2.join("qemu.pid"), format!("{}\n", std::process::id())).expect("live pid 2");
    assert!(
        any_live_qemu_scan(ws, &|_| me.clone()),
        "第二目录命中必须报真"
    );

    // run 根下的散文件（非 agent 目录）被跳过，不误判不崩。
    std::fs::write(vm_run_root(ws).join("stray.txt"), b"junk").expect("stray file");
    std::fs::write(run2.join("qemu.pid"), format!("{}\n", u32::MAX - 1)).expect("dead pid 2");
    assert!(!any_live_qemu_scan(ws, &|_| me.clone()), "散文件不误判");

    // spawn 在途标记（新鲜）= 有活——pidfile 未写的进程创建窗跨进程可见
    // （修前纯 pidfile 扫描看不见 spawn 在途，换装可能撞上正在制备的实例）。
    let run3 = vm_run_dir(ws, "agent-3");
    std::fs::create_dir_all(&run3).expect("run dir 3");
    std::fs::write(run3.join("starting"), std::process::id().to_string()).expect("marker");
    assert!(any_live_qemu_scan(ws, &|_| me.clone()), "新鲜在途标记算活");

    // 陈旧标记（进程死在 spawn 相位的残留）= 无活——陈旧界（600s）外
    // 不拦换装。
    backdate(&run3.join("starting"), Duration::from_secs(601));
    assert!(!any_live_qemu_scan(ws, &|_| me.clone()), "陈旧标记不算活");
}

/// 在途标记守卫终态纪律：Drop 移除标记（成功/失败各路径的 `?` 早退全
/// 覆盖——守卫语义即「活着才算在途」）。
#[test]
fn starting_marker_guard_drop_removes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let run = tmp.path().join("run").join("agent-1");
    std::fs::create_dir_all(&run).expect("run dir");
    let marker = run.join("starting");
    {
        let g = StartingMarkerGuard::arm(&run).expect("arm");
        assert_eq!(g.path, marker);
        assert!(marker.exists(), "arm 即落盘");
        assert!(starting_marker_fresh(&marker), "刚落的标记新鲜");
    }
    assert!(!marker.exists(), "Drop 即移除");

    // 陈旧界内的判定边界：缺席 = 不新鲜（不在途）。
    assert!(!starting_marker_fresh(&run.join("no-such-marker")));
}
