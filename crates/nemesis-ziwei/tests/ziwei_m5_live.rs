#![cfg(windows)]
//! M5 实弹演练（总控 §M5 验收）：验证链 / 一次性降级注记 / stale 通知 /
//! 授权找回 全链路，真 exe 文件面（三变体 dummy-draw）。
//!
//! 腿 A：首次验证锚定 hash 基准 → Active；复验 hash 一致 → Active（不重扫）。
//! 腿 B：V2（bake-mutate，锚同 hash 异）→ 锚投票 3≥2 → ActiveDegraded +
//!       一次性报告/事件（str_ref V1 产品串被常量折叠消除——真实版本升级
//!       语义：3/4 锚存活）；复验不再重复打扰（notice_emitted=false）。
//! 腿 C：noanchor（编译期剥锚）→ 锚投票 1<2 → Stale + profile 持久化 +
//!       pre_attach_guard 拒附加 + notify.jsonl + ziwei.stale 事件。
//! 腿 D：recover_start（noanchor）→ partially_recoverable + 报告 md 落盘 +
//!       无 LLM 钩子诚实注记；recover_confirm 全接受 → 仅 str_ref 函数找回，
//!       其余诚实拒绝 → 保持 Stale（无部分恢复态）。
//! 腿 E：recover_confirm（V1 全锚命中，「误报恢复」：noanchor 是错误构建，
//!       真目标仍是 V1）→ all_recovered + Active + hash 基准回锚 →
//!       verify(V1) hash 一致 → Active；guard 放行。
//!       （V2 缺 feed_render 锚，找回只能保命中函数 → 对 V2 全找回不可达，
//!       只能降级（B 腿）或重配——设计语义，见 README 诚实边界。）
//! 腿 F：吊销 → verify loud 拒 + guard 拒（不可自动复活）。
//!
//! 证据落 `test-tools/resource/ziwei-uat/m5/m5_verify_recover.txt`。
//! 夹具纪律同 m1/m3/m4：变体缺席 = 现场构建；构建失败 = 诚实 SKIP。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use nemesis_ziwei::profile::{
    Anchor, AnchorSet, AnchorType, ArgSpec, Conv, FunctionDef, LocateKind, LocateSpec, Machine,
    Profile, ProfileStatus, ProfileStore, SCHEMA_VERSION, TargetFingerprint, Verification,
    VerifyPolicy,
};
use nemesis_ziwei::verify::{
    Verdict, hit_functions, pre_attach_guard, recover_confirm, recover_start, set_event_hook,
    sha256_file, verify_profile,
};

const DUMMY_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/dummy-targets"
);
const UAT_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/resource/ziwei-uat/m5"
);

/// 三变体 exe（缺席则现场构建对应 feature 变体；失败 = 诚实 SKIP）。
fn variant_exe(bin_name: &str, features: &[&str]) -> Option<PathBuf> {
    let p = Path::new(DUMMY_ROOT).join("bin").join("x64").join(bin_name);
    if p.is_file() {
        return Some(p);
    }
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "--target", "x86_64-pc-windows-msvc"])
        .arg("--manifest-path")
        .arg(Path::new(DUMMY_ROOT).join("dummy-draw").join("Cargo.toml"));
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    let built = cmd.status();
    let src = Path::new(DUMMY_ROOT)
        .join("dummy-draw")
        .join("target")
        .join("x86_64-pc-windows-msvc")
        .join("release")
        .join("dummy-draw.exe");
    match built {
        Ok(s) if s.success() && src.is_file() => {
            if std::fs::copy(&src, &p).is_ok() {
                Some(p)
            } else {
                None
            }
        }
        _ => {
            eprintln!("SKIP: {bin_name} 现场构建失败——集成前置不满足");
            None
        }
    }
}

fn temp_ws() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-m5-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 证据收集器（stdout + 落盘双通道；io 失败诚实降级为仅 stdout）。
struct Evidence {
    lines: Mutex<Vec<String>>,
    path: Option<PathBuf>,
}

impl Evidence {
    fn new() -> Evidence {
        let dir = Path::new(UAT_DIR);
        let path = if dir.is_dir() || std::fs::create_dir_all(dir).is_ok() {
            Some(dir.join("m5_verify_recover.txt"))
        } else {
            eprintln!("WARN: 证据目录 {UAT_DIR} 不可建——仅 stdout");
            None
        };
        Evidence {
            lines: Mutex::new(Vec::new()),
            path,
        }
    }

    fn log(&self, step: &str, v: &serde_json::Value) {
        let line = format!(
            "== {step} ==\n{}",
            serde_json::to_string_pretty(v).unwrap_or_default()
        );
        println!("{line}");
        self.lines.lock().unwrap().push(line);
    }

    fn log_line(&self, s: &str) {
        println!("{s}");
        self.lines.lock().unwrap().push(s.to_string());
    }

    fn flush(&self) {
        if let Some(p) = &self.path {
            let body = self.lines.lock().unwrap().join("\n");
            if let Err(e) = std::fs::write(p, body + "\n") {
                eprintln!("WARN: 证据落盘失败（{}）：{e}", p.display());
            } else {
                println!("[evidence] {}", p.display());
            }
        }
    }
}

fn pattern_loc(p: &str) -> LocateSpec {
    LocateSpec {
        kind: LocateKind::BytePattern,
        module: None,
        pattern: Some(p.to_string()),
        rva: None,
    }
}

/// M5 profile：四个函数（语义名即锚关联键）；阈值 2；hash 基准待首验锚定。
fn m5_profile(profile_id: &str) -> Profile {
    let f = |semantic: &str, pattern: &str| FunctionDef {
        semantic: semantic.into(),
        locate: pattern_loc(pattern),
        conv: Conv::MsX64,
        args: vec![ArgSpec {
            reg: "rcx".into(),
            ty: None,
            desc: None,
        }],
        capture: None,
    };
    Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: profile_id.to_string(),
        name: "m5 verify/recover live".into(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: ProfileStatus::Active,
        target: TargetFingerprint {
            exe_name: "dummy-draw.exe".into(),
            exe_path_hint: None,
            machine: Machine::X64,
            file_sha256: None,
            version_resource: Default::default(),
            verify_policy: VerifyPolicy::HashOrVersion,
        },
        attach: Default::default(),
        functions: vec![
            f("push_record", "90 90 90 90 90 90 90 90 B8 8D 7C 6B 5A"),
            f("feed_render", "90 90 90 90 90 90 90 90"),
            f("slow_op", "90 90 90 90 90 90 90 90 B8 33 22 11 00"),
            f("crash_op", "90 90 90 90 90 90 90 90 B8 0D F0 AD 0B"),
        ],
        data_structures: vec![],
        tools: vec![],
        verification: Verification {
            on_attach: vec!["file_sha256".into(), "anchors_vote>=2".into()],
            stale_action: Some("notify_user".into()),
        },
    }
}

/// 真锚集（与夹具字节布局一一对应；4 锚，阈值 2）：
/// - push_record: byte_pattern（sled+imm 13B，全变体唯一）
/// - feed_render: str_ref（V1 产品串；bake-mutate 常量折叠会消除该串——
///   版本升级锚衰减的真实样本，V2 投票 3/4 的来源）
/// - slow_op: imm_const 0x00112233（仅 asm 预埋）
/// - crash_op: imm_const 0x0BADF00D（仅 asm 预埋）
///
/// 故意不用 imm_const 0x5A6B7C8D：MAGIC_V1 经 C7 imm32 写入指令同样出现在
/// .text（init_area），剥锚变体也命中——会污染 stale 腿的投票语义。
fn m5_anchors(profile_id: &str) -> AnchorSet {
    let a = |function: &str, kind: AnchorType, patch: &dyn Fn(&mut Anchor)| -> Anchor {
        let mut anchor = Anchor {
            function: function.into(),
            kind,
            module: None,
            pattern: None,
            text: None,
            distance: None,
            value: None,
            imports: None,
            bb_hash: None,
            note: None,
        };
        patch(&mut anchor);
        anchor
    };
    AnchorSet {
        profile_id: profile_id.into(),
        anchors: vec![
            a("push_record", AnchorType::BytePattern, &|x| {
                x.pattern = Some("90 90 90 90 90 90 90 90 B8 8D 7C 6B 5A".into());
            }),
            a("feed_render", AnchorType::StrRef, &|x| {
                x.text = Some("ZIWEI_DUMMY_DRAW_V1".into());
            }),
            a("slow_op", AnchorType::ImmConst, &|x| {
                x.value = Some(0x0011_2233);
            }),
            a("crash_op", AnchorType::ImmConst, &|x| {
                x.value = Some(0x0BAD_F00D);
            }),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn m5_verify_recover_live_on_dummy_variants() {
    let Some(exe_v1) = variant_exe("dummy-draw.exe", &[]) else {
        return;
    };
    let Some(exe_v2) = variant_exe("dummy-draw-v2.exe", &["bake-mutate"]) else {
        return;
    };
    let Some(exe_noa) = variant_exe("dummy-draw-noanchor.exe", &["no-anchors"]) else {
        return;
    };
    let ev = Evidence::new();
    let ws = temp_ws();
    let store = ProfileStore::new(&ws);
    let id = "zw-pro-m5live";

    // 夹具健全性：三变体 hash 两两互异（V2=更新版、noanchor=重编剥锚）。
    let (h_v1, h_v2, h_noa) = (
        sha256_file(&exe_v1).unwrap(),
        sha256_file(&exe_v2).unwrap(),
        sha256_file(&exe_noa).unwrap(),
    );
    assert_ne!(h_v1, h_v2, "V1/V2 hash 必须互异");
    assert_ne!(h_v1, h_noa, "V1/noanchor hash 必须互异");
    assert_ne!(h_v2, h_noa, "V2/noanchor hash 必须互异");
    ev.log(
        "fixture",
        &serde_json::json!({
            "v1": exe_v1.display().to_string(), "v1_sha256": h_v1,
            "v2": exe_v2.display().to_string(), "v2_sha256": h_v2,
            "noanchor": exe_noa.display().to_string(), "noanchor_sha256": h_noa,
        }),
    );

    // 事件钩子（验证链 T5.2 面）：全程收集 ziwei.* 事件。
    let events: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let events_c = events.clone();
    assert!(
        set_event_hook(Arc::new(move |name: &str, data: &serde_json::Value| {
            events_c
                .lock()
                .unwrap()
                .push((name.to_string(), data.clone()));
        })),
        "事件钩子首次安装应成功"
    );

    // profile + 锚集落盘
    let profile = m5_profile(id);
    let anchors = m5_anchors(id);
    store
        .save(&profile, Some(&anchors))
        .expect("profile+anchors save");

    // ── 腿 A：首次验证 = 锚定 hash 基准 → Active；复验 hash 一致 → Active ──
    let out = verify_profile(&store, &ws, &profile, &exe_v1).expect("A 首验");
    assert_eq!(out.report.verdict, Verdict::Active);
    assert_eq!(out.report.hash_expected, None, "首验无基准");
    assert_eq!(out.report.hash_actual, h_v1);
    assert!(out.report.notes.iter().any(|n| n.contains("锚定")));
    let profile = out.profile_updated.expect("首验必带 profile 更新");
    assert_eq!(profile.target.file_sha256.as_deref(), Some(h_v1.as_str()));
    store.save(&profile, None).expect("A 持久化");

    let out = verify_profile(&store, &ws, &profile, &exe_v1).expect("A 复验");
    assert_eq!(out.report.verdict, Verdict::Active);
    assert!(out.report.hash_match, "复验 hash 一致");
    assert!(out.report.anchors.is_none(), "hash 一致不重扫锚");
    ev.log(
        "A 首验+复验（Active 基准链）",
        &serde_json::json!({
            "first": { "verdict": "active", "baseline_anchored": true },
            "second": { "verdict": "active", "hash_match": true, "rescan": false },
        }),
    );

    // ── 腿 B：V2（锚同 hash 异）→ ActiveDegraded + 一次性注记/事件 ──
    let out = verify_profile(&store, &ws, &profile, &exe_v2).expect("B 降级验证");
    assert_eq!(out.report.verdict, Verdict::ActiveDegraded);
    assert!(!out.report.hash_match);
    let av = out.report.anchors.as_ref().expect("降级必有锚投票");
    assert!(av.passed, "锚投票应过阈值（3≥2）：{av:?}");
    assert_eq!(
        (av.total, av.supported, av.hit),
        (4, 4, 3),
        "V2：str_ref V1 串被常量折叠消除"
    );
    assert_eq!(av.threshold, 2);
    assert!(out.report.notice_emitted, "首次降级应新落报告");
    let degraded_report = out.report_path.clone().expect("降级报告落盘");
    assert!(degraded_report.is_file());
    assert!(out.report.notes.iter().any(|n| n.contains("降级注记")));
    let profile_b = out.profile_updated.expect("B 更新");
    assert_eq!(profile_b.status, ProfileStatus::Active, "降级不改状态");
    // 缺的是 feed_render 的 V1 产品串锚（版本升级真实语义）
    assert_eq!(
        hit_functions(&store, id, &exe_v2).unwrap(),
        vec!["crash_op", "push_record", "slow_op"],
        "V2 存活锚函数集（feed_render 的 V1 串已不在）"
    );

    let out2 = verify_profile(&store, &ws, &profile_b, &exe_v2).expect("B 复验");
    assert_eq!(out2.report.verdict, Verdict::ActiveDegraded);
    assert!(
        !out2.report.notice_emitted,
        "同 (profile,hash) 二验不得重复打扰"
    );
    ev.log(
        "B V2 → ActiveDegraded + 一次性",
        &serde_json::json!({
            "first": { "verdict": "active_degraded", "anchors": av, "notice_emitted": true,
                       "report": degraded_report.display().to_string() },
            "second": { "verdict": "active_degraded", "notice_emitted": false },
            "status_after": "active（降级不改状态）",
        }),
    );

    // ── 腿 C：noanchor（锚全剥）→ Stale + 持久化 + guard 拒 + notify + 事件 ──
    let out = verify_profile(&store, &ws, &profile_b, &exe_noa).expect("C 失效验证");
    assert_eq!(out.report.verdict, Verdict::Stale);
    let av = out.report.anchors.as_ref().expect("stale 必有锚投票");
    assert!(!av.passed, "投票应不过（1<2）：{av:?}");
    assert_eq!((av.hit, av.supported), (1, 4), "仅 str_ref 存活");
    let stale_profile = out.profile_updated.expect("C 更新");
    assert_eq!(stale_profile.status, ProfileStatus::Stale, "状态机落 Stale");
    store.save(&stale_profile, None).expect("C 持久化");
    // pre_attach_guard 拒绝（文案含指引）
    let guard_err = pre_attach_guard(&stale_profile).expect_err("stale 必须拒附加");
    assert!(
        guard_err.contains("已失效") && guard_err.contains("recover"),
        "guard 文案：{guard_err}"
    );
    // notify.jsonl 落一行 stale 通知
    let notify_path = ws.join("logs").join("ziwei").join("notify.jsonl");
    let notify_body = std::fs::read_to_string(&notify_path).expect("notify.jsonl 存在");
    assert!(notify_body.contains("\"kind\":\"stale\"") && notify_body.contains(id));
    // reload 磁盘面：Stale 已持久化
    assert_eq!(store.load(id).unwrap().status, ProfileStatus::Stale);
    ev.log(
        "C noanchor → Stale",
        &serde_json::json!({
            "verdict": "stale", "anchors": av,
            "status_persisted": true,
            "guard": guard_err,
            "notify": notify_path.display().to_string(),
        }),
    );

    // ── 腿 D：找回对 noanchor 诚实失败（仅 1/4 锚存活）──
    let rep = recover_start(&store, &ws, id, &exe_noa).expect("D recover_start");
    let verdict = &rep["report"]["verdict"];
    assert_eq!(
        verdict["verdict"], "partially_recoverable",
        "1/4 应部分可找回：{rep}"
    );
    assert_eq!(
        rep["report"]["llm_review"], "（LLM 语义比对未装配——以确定性证据为准）",
        "无 LLM 钩子 = 诚实注记"
    );
    let md_path = PathBuf::from(rep["report_md"].as_str().expect("report_md"));
    assert!(md_path.is_file(), "《找回报告》md 落盘");
    let llm_note = rep["report"]["llm_review"]
        .as_str()
        .expect("llm_review")
        .to_string();
    let per_anchor = verdict["per_anchor"].as_array().expect("per_anchor");
    assert_eq!(per_anchor.len(), 4);
    let hit_of = |fn_name: &str| {
        per_anchor
            .iter()
            .find(|r| r["function"] == fn_name)
            .expect("锚行")["hit"]
            .as_bool()
            .expect("hit bool")
    };
    assert!(!hit_of("push_record"), "byte_pattern 应 miss");
    assert!(hit_of("feed_render"), "str_ref 应 hit");
    assert!(!hit_of("slow_op") && !hit_of("crash_op"), "imm 锚应 miss");

    // 全接受确认 → 仅 feed_render 找回，其余诚实拒绝 → 保持 Stale
    let accept_all: Vec<String> = vec![
        "push_record".into(),
        "feed_render".into(),
        "slow_op".into(),
        "crash_op".into(),
    ];
    let rep = recover_confirm(&store, &ws, id, &exe_noa, &accept_all).expect("D confirm");
    assert_eq!(rep["all_recovered"], false, "noanchor 不可全找回：{rep}");
    assert_eq!(rep["status"], "stale", "有拒绝 = 保持 Stale");
    let per_function = rep["per_function"].as_array().expect("per_function");
    let recovered_of = |fn_name: &str| {
        per_function
            .iter()
            .find(|r| r["function"] == fn_name)
            .expect("函数行")["recovered"]
            .as_bool()
            .expect("bool")
    };
    assert!(recovered_of("feed_render"));
    assert!(!recovered_of("push_record") && !recovered_of("slow_op") && !recovered_of("crash_op"));
    assert_eq!(store.load(id).unwrap().status, ProfileStatus::Stale);
    ev.log(
        "D noanchor 找回诚实拒绝",
        &serde_json::json!({
            "recover_start": { "verdict": "partially_recoverable", "llm": llm_note,
                "report_md": md_path.display().to_string() },
            "confirm": rep,
        }),
    );

    // ── 腿 E：对 V1 找回确认（全锚命中，「误报恢复」）→ Active + 复验闭环 ──
    let hits = hit_functions(&store, id, &exe_v1).expect("E hit_functions");
    assert_eq!(
        hits,
        vec!["crash_op", "feed_render", "push_record", "slow_op"]
    );
    let rep = recover_confirm(&store, &ws, id, &exe_v1, &hits).expect("E confirm V1");
    assert_eq!(rep["all_recovered"], true, "V1 全锚命中应全找回：{rep}");
    assert_eq!(rep["status"], "active");
    let profile_e = store.load(id).expect("E reload");
    assert_eq!(profile_e.status, ProfileStatus::Active);
    assert_eq!(
        profile_e.target.file_sha256.as_deref(),
        Some(h_v1.as_str()),
        "hash 基准回锚 V1"
    );
    // byte_pattern 载荷在 V1 中逐字相同 → 不需要重生（重生数 0 是正确语义）
    assert_eq!(rep["regenerated_anchors"], 0, "V1 锚字节未变，无需重生");
    // 复验：hash 一致 → Active（闭环）
    let out = verify_profile(&store, &ws, &profile_e, &exe_v1).expect("E 复验");
    assert_eq!(out.report.verdict, Verdict::Active);
    assert!(out.report.hash_match);
    assert_eq!(
        pre_attach_guard(&out.profile_updated.unwrap()),
        Ok(()),
        "Active 放行附加"
    );
    ev.log(
        "E V1 找回确认（误报恢复）→ Active",
        &serde_json::json!({
            "hit_functions": hits,
            "confirm": rep,
            "reverify": { "verdict": "active", "hash_match": true },
            "guard": "pass",
            "note": "V2 缺 feed_render 锚不可全找回（B 腿降级覆盖）；此处演示全找回路径",
        }),
    );

    // ── 腿 F：吊销 → 不可验证 / 不可附加（不可自动复活）──
    let mut revoked = store.load(id).unwrap();
    revoked.status = ProfileStatus::Revoked;
    store.save(&revoked, None).expect("F 持久化吊销");
    let err = verify_profile(&store, &ws, &revoked, &exe_v1).expect_err("吊销必须拒验证");
    assert!(err.contains("已吊销"), "verify 文案：{err}");
    let gerr = pre_attach_guard(&revoked).expect_err("吊销必须拒附加");
    assert!(gerr.contains("已吊销"), "guard 文案：{gerr}");
    ev.log(
        "F 吊销拒绝面",
        &serde_json::json!({ "verify": err, "guard": gerr }),
    );

    // ── 事件钩子总账 ──
    let evs = events.lock().unwrap();
    let names: Vec<&str> = evs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["ziwei.degraded", "ziwei.stale", "ziwei.recovered"],
        "事件序列：{names:?}"
    );
    drop(evs);
    ev.log(
        "事件钩子总账",
        &serde_json::json!({
            "sequence": ["ziwei.degraded(B)", "ziwei.stale(C)", "ziwei.recovered(E)"],
        }),
    );

    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
    ev.log_line("[result] PASS");
    ev.flush();
}
