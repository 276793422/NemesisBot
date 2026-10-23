#![cfg(windows)]
//! M4 实弹集成（总控 §M4 验收）：dummy-draw 场景 A 闭环。
//!
//! 覆盖：resolve 锚扫描真值（唯一命中 + 锚点回读逐字等于 pattern + 距入口
//! ≤64B）、call_fn 真值/超时看门狗/单飞拒绝/SEH 救援（宿主存活）、hook
//! push_record ≥1000 条逐条真值（regs 域 + seq 单调 + decode 魔法值 +
//! dropped=0）、2 级链收敛（feed_render::chain0 与顶层 1:1）、hook 退订即停。
//! 证据落 `test-tools/resource/ziwei-uat/m4/m4_runtime_live.txt`。
//!
//! 前置与降级纪律同 m1/m3：payload 内嵌缺席 / dummy 构建失败 = 诚实 SKIP。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use nemesis_ziwei::profile::{
    ArgSpec, CaptureSpec, ChainStep, Conv, DataStructure, FieldDecode, FieldDef, FieldType,
    FunctionDef, LocateKind, LocateSpec, Machine, Primitive, Profile, ProfileStatus, ProfileStore,
    SCHEMA_VERSION, TargetFingerprint, ToolAction, ToolDef, Verification, VerifyPolicy,
};
use nemesis_ziwei::session::SessionManager;

const DUMMY_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/dummy-targets"
);
const UAT_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/resource/ziwei-uat/m4"
);

fn dummy_exe(arch: &str, name: &str) -> Option<PathBuf> {
    let p = Path::new(DUMMY_ROOT)
        .join("bin")
        .join(arch)
        .join(format!("{name}.exe"));
    if p.is_file() {
        return Some(p);
    }
    let manifest = Path::new(DUMMY_ROOT).join(name).join("Cargo.toml");
    let built = Command::new("cargo")
        .args(["build", "--release"])
        .arg("--manifest-path")
        .arg(&manifest)
        .status();
    match built {
        Ok(s) if s.success() && p.is_file() => Some(p),
        other => {
            eprintln!("SKIP: {name} 现场构建失败（{other:?}）——集成前置不满足");
            None
        }
    }
}

/// panic 时也杀掉 dummy-draw（测试失败不留孤儿 tick 进程）。
struct KillGuard(Child);

impl Drop for KillGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// dummy-draw stdout 契约（fixture 直报运行时真值）。
#[derive(Default, Debug, Clone)]
struct DrawContract {
    pid: u32,
    magic: u32,
    data_addr: u64,
    fn_push_record: u64,
    fn_feed_render: u64,
    fn_render_record: u64,
    fn_slow_op: u64,
    fn_crash_op: u64,
}

impl DrawContract {
    fn complete(&self) -> bool {
        self.pid != 0
            && self.magic != 0
            && self.data_addr != 0
            && self.fn_push_record != 0
            && self.fn_feed_render != 0
            && self.fn_render_record != 0
            && self.fn_slow_op != 0
            && self.fn_crash_op != 0
    }
}

/// spawn dummy-draw（--tick-ms=3 加速）并解析全部契约行；tick 行由防堵
/// 线程吞掉。
fn spawn_draw(exe: &Path) -> (KillGuard, DrawContract) {
    let mut cmd = Command::new(exe);
    let mut child = cmd
        .arg("--tick-ms=3")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn dummy-draw");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut c = DrawContract::default();
    let grab = |t: &str, prefix: &str, slot: &mut u64| {
        if let Some(v) = t.strip_prefix(prefix) {
            *slot = u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("契约地址");
        }
    };
    let mut line = String::new();
    while !c.complete() {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => panic!("dummy-draw 提前退出"),
            Ok(_) => {
                let t = line.trim();
                if let Some(v) = t.strip_prefix("PID=") {
                    c.pid = v.parse().expect("PID");
                }
                if let Some(v) = t.strip_prefix("MAGIC=0x") {
                    c.magic = u32::from_str_radix(v, 16).expect("magic");
                }
                grab(t, "DATA_ADDR=", &mut c.data_addr);
                grab(t, "FN_PUSH_RECORD=", &mut c.fn_push_record);
                grab(t, "FN_FEED_RENDER=", &mut c.fn_feed_render);
                grab(t, "FN_RENDER_RECORD=", &mut c.fn_render_record);
                grab(t, "FN_SLOW_OP=", &mut c.fn_slow_op);
                grab(t, "FN_CRASH_OP=", &mut c.fn_crash_op);
            }
            Err(e) => panic!("读 stdout 失败: {e}"),
        }
    }
    // 剩余 tick 行防管道堵
    std::thread::spawn(move || {
        let mut rest = String::new();
        loop {
            rest.clear();
            match reader.read_line(&mut rest) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    (KillGuard(child), c)
}

fn temp_ws() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-m4-{}-{}",
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
            Some(dir.join("m4_runtime_live.txt"))
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

fn ft(spec: &str) -> FieldType {
    FieldType::parse(spec).expect("字段类型")
}

/// M4 profile：push_record/slow_op/crash_op 用 13B 唯一锚；feed_render 带
/// 1 级链（运行期地址由 FN_ 契约直供——8×0x90 前缀不唯一，锚扫描只作
/// 形态合法位）。
fn m4_profile(profile_id: &str) -> Profile {
    Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: profile_id.to_string(),
        name: "m4 live dummy-draw".into(),
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
            FunctionDef {
                semantic: "push_record".into(),
                locate: pattern_loc("90 90 90 90 90 90 90 90 B8 8D 7C 6B 5A"),
                conv: Conv::MsX64,
                args: vec![
                    ArgSpec {
                        reg: "rcx".into(),
                        desc: Some("DataArea*（regs[0]，解码锚）".into()),
                    },
                    ArgSpec {
                        reg: "rdx".into(),
                        desc: Some("idx（0..5）".into()),
                    },
                    ArgSpec {
                        reg: "r8".into(),
                        desc: Some("delta（1..=3）".into()),
                    },
                ],
                capture: Some(CaptureSpec {
                    decode: Some("struct:DataHeader".into()),
                    emit: None,
                    chain: vec![],
                }),
            },
            FunctionDef {
                semantic: "slow_op".into(),
                locate: pattern_loc("90 90 90 90 90 90 90 90 B8 33 22 11 00"),
                conv: Conv::MsX64,
                args: vec![ArgSpec {
                    reg: "rcx".into(),
                    desc: Some("spin_ms（u32）".into()),
                }],
                capture: None,
            },
            FunctionDef {
                semantic: "crash_op".into(),
                locate: pattern_loc("90 90 90 90 90 90 90 90 B8 0D F0 AD 0B"),
                conv: Conv::MsX64,
                args: vec![],
                capture: None,
            },
            FunctionDef {
                semantic: "feed_render".into(),
                locate: pattern_loc("90 90 90 90 90 90 90 90"),
                conv: Conv::MsX64,
                args: vec![],
                capture: Some(CaptureSpec {
                    decode: None,
                    emit: None,
                    chain: vec![ChainStep {
                        // render_record 自带 sled+RENDER_ANCHOR（0x0C0FFEE0）——
                        // v1 hook 只挂 nop 窗口，裸 magic pattern 既歧义（命中
                        // push_record 的同值 magic）又不可挂（magic 非 nop）
                        locate: pattern_loc("90 90 90 90 90 90 90 90 B8 E0 FE 0F 0C"),
                        decode: None,
                        note: Some("第二级 render_record（sled+imm_const 双锚，唯一命中）".into()),
                    }],
                }),
            },
        ],
        data_structures: vec![DataStructure {
            name: "DataHeader".into(),
            size: 16,
            fields: vec![
                FieldDef {
                    offset: 0,
                    name: "magic".into(),
                    type_: ft("u32"),
                    decode: Some(FieldDecode::Hex),
                },
                FieldDef {
                    offset: 4,
                    name: "layout_version".into(),
                    type_: ft("u32"),
                    decode: None,
                },
                FieldDef {
                    offset: 8,
                    name: "count".into(),
                    type_: ft("u32"),
                    decode: None,
                },
                FieldDef {
                    offset: 12,
                    name: "capacity".into(),
                    type_: ft("u32"),
                    decode: None,
                },
            ],
        }],
        tools: vec![
            ToolDef {
                tool_name: "m4_slow_call".into(),
                description: "调用 slow_op 自旋（call_fn 真值面）".into(),
                input_schema: serde_json::Value::Null,
                action: ToolAction {
                    primitive: Primitive::CallFn,
                    addr: "fn:slow_op".into(),
                    mode: None,
                    interval_secs: None,
                    max_records: None,
                    timeout_ms: Some(5_000),
                    notify: None,
                },
                output: None,
                run_policy: nemesis_ziwei::profile::RunPolicy::Auto,
            },
            ToolDef {
                tool_name: "m4_push_hook".into(),
                description: "hook push_record 捕获推送流（hook_capture 面）".into(),
                input_schema: serde_json::Value::Null,
                action: ToolAction {
                    primitive: Primitive::HookCapture,
                    addr: "fn:push_record".into(),
                    mode: None,
                    interval_secs: None,
                    max_records: Some(2000),
                    timeout_ms: None,
                    notify: None,
                },
                output: Some(nemesis_ziwei::profile::OutputSpec {
                    dest: "m4_hook_report.jsonl".into(),
                }),
                run_policy: nemesis_ziwei::profile::RunPolicy::Auto,
            },
        ],
        verification: Verification {
            on_attach: vec!["file_sha256".into()],
            stale_action: None,
        },
    }
}

/// M4 验收主链路：resolve 真值 → call_fn 三态 → hook 1000 逐条 → 2 级链 → 退订。
#[tokio::test(flavor = "multi_thread")]
async fn m4_runtime_live_on_dummy_draw() {
    let Some(exe) = dummy_exe("x64", "dummy-draw") else {
        return; // 诚实 SKIP
    };
    let ev = Evidence::new();
    let ws = temp_ws();
    let sess = Arc::new(SessionManager::new(&ws, None, 2));
    nemesis_ziwei::session::set_global(sess.clone());
    let (mut guard, contract) = spawn_draw(&exe);
    ev.log_line(&format!(
        "[fixture] dummy-draw pid={} MAGIC=0x{:X} DATA_ADDR=0x{:X} \
         FN_PUSH_RECORD=0x{:X} FN_FEED_RENDER=0x{:X} FN_RENDER_RECORD=0x{:X} \
         FN_SLOW_OP=0x{:X} FN_CRASH_OP=0x{:X}",
        contract.pid,
        contract.magic,
        contract.data_addr,
        contract.fn_push_record,
        contract.fn_feed_render,
        contract.fn_render_record,
        contract.fn_slow_op,
        contract.fn_crash_op
    ));

    // ── profile 落盘 + attach ──
    let id = "zw-pro-m4live";
    let profile = m4_profile(id);
    ProfileStore::new(&ws)
        .save(&profile, None)
        .expect("profile save");
    let attached = nemesis_ziwei::scheduler::attach_and_schedule(
        &sess,
        &profile,
        contract.pid,
        std::collections::HashMap::from([("DataHeader".to_string(), contract.data_addr)]),
    )
    .await
    .expect("attach");
    ev.log("attach", &attached.snapshot());

    // ── ① resolve：锚扫描真值三重断言 ──
    // 夹具字节布局事实：asm! 前有编译器 prologue，pattern 锚在函数入口 +N
    // 字节（N 随函数而变）——**锚点地址 ≠ 入口**，所以调用/hook 地址一律用
    // FN_ stdout 契约直供（夹具设计如此）。resolve 的真值面改为：
    //  a. 三 pattern 全部唯一命中（notes 空）；
    //  b. 锚点回读 13 字节 == pattern 本身（逐字真值）；
    //  c. 锚点距 FN_ 入口 ≤64B（近邻合理性）。
    let rep = sess.resolve_functions(id).await.expect("resolve_functions");
    let resolved = rep["resolved"].clone();
    ev.log("① resolve", &rep);
    assert!(
        rep["notes"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "三 pattern 应全部唯一命中（notes 空）：{rep}"
    );
    let addr_of = |k: &str| -> u64 {
        u64::from_str_radix(
            resolved[k]
                .as_str()
                .unwrap_or_else(|| panic!("resolved 缺 {k}"))
                .trim_start_matches("0x"),
            16,
        )
        .expect("addr")
    };
    for (name, pat, fn_entry) in [
        (
            "push_record",
            [0xB8u8, 0x8D, 0x7C, 0x6B, 0x5A],
            contract.fn_push_record,
        ),
        (
            "slow_op",
            [0xB8u8, 0x33, 0x22, 0x11, 0x00],
            contract.fn_slow_op,
        ),
        (
            "crash_op",
            [0xB8u8, 0x0D, 0xF0, 0xAD, 0x0B],
            contract.fn_crash_op,
        ),
    ] {
        let expect: Vec<u8> = [0x90u8; 8].into_iter().chain(pat).collect();
        let bytes = read_bytes(&sess, id, addr_of(name), 13).await;
        assert_eq!(
            bytes, expect,
            "{name} 锚点回读必须逐字等于 pattern（锚点即锚字节所在）"
        );
        let dist = addr_of(name).abs_diff(fn_entry);
        assert!(dist <= 64, "{name} 锚点距入口 {dist}B 越界（应 ≤64B）");
    }
    ev.log_line("[truth] push_record/slow_op/crash_op 三锚点：唯一命中 + 回读逐字等于 pattern + 距入口 ≤64B");

    // 调用/hook 地址表：FN_ stdout 契约直供（pattern 锚在编译器 prologue
    // 之后、非入口——用锚点地址调函数会跳过 prologue 造成栈失衡）。
    let mut addrs = std::collections::HashMap::new();
    addrs.insert("push_record".to_string(), contract.fn_push_record);
    addrs.insert("slow_op".to_string(), contract.fn_slow_op);
    addrs.insert("crash_op".to_string(), contract.fn_crash_op);
    addrs.insert("feed_render".to_string(), contract.fn_feed_render);
    addrs.insert("feed_render::chain0".to_string(), contract.fn_render_record);
    attached.set_fn_addrs(addrs);

    // ── ② call_fn 三态 ──
    // ②a 真值：slow_op(50) == 50（直地址路径）
    let out = sess
        .call_fn_addr(
            id,
            contract.fn_slow_op,
            nemesis_ziwei::dispatch::CONV_MSX64,
            [50, 0, 0, 0],
            5_000,
        )
        .await
        .expect("call slow_op(50)");
    assert_eq!(out["ret"], 50, "slow_op 返回真值：{out}");
    assert!(
        out["elapsed_ms"].as_u64().unwrap_or(0) >= 45,
        "自旋耗时下界：{out}"
    );
    // ②b runtime 统一入口（查表路径 + fn 装饰）
    let out2 =
        nemesis_ziwei::runtime::invoke_call_fn(&sess, &ws, id, "slow_op", [80, 0, 0, 0], 5_000)
            .await
            .expect("invoke_call_fn slow_op(80)");
    assert_eq!(out2["ret"], 80);
    assert_eq!(out2["fn"], "slow_op");
    ev.log("② call_fn 真值", &serde_json::json!([out, out2]));

    // ②c 超时看门狗：slow_op(1500) timeout=300 → Timeout 应答
    let to = sess
        .call_fn_addr(
            id,
            contract.fn_slow_op,
            nemesis_ziwei::dispatch::CONV_MSX64,
            [1500, 0, 0, 0],
            300,
        )
        .await
        .expect("call slow_op(1500) 超时窗");
    assert_eq!(to["timeout"], true, "看门狗超时：{to}");
    assert!(
        to["waited_ms"].as_u64().unwrap_or(0) >= 280,
        "超时等待下界：{to}"
    );
    ev.log("② call_fn 超时", &to);

    // ②d 单飞拒绝：看门狗线程仍在途（1500ms 自旋）→ 新调用被拒
    let busy = sess
        .call_fn_addr(
            id,
            contract.fn_slow_op,
            nemesis_ziwei::dispatch::CONV_MSX64,
            [0, 0, 0, 0],
            5_000,
        )
        .await;
    let busy_err = busy.expect_err("在途期间新调用应被单飞拒绝");
    assert!(busy_err.contains("单飞"), "单飞拒绝文案：{busy_err}");
    ev.log_line(&format!("[单飞拒绝] {busy_err}"));
    // 等在途线程自然结束（1500ms 自旋 + 余量）后恢复
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    let out3 = sess
        .call_fn_addr(
            id,
            contract.fn_slow_op,
            nemesis_ziwei::dispatch::CONV_MSX64,
            [0, 0, 0, 0],
            5_000,
        )
        .await
        .expect("在途结束后恢复");
    assert_eq!(out3["ret"], 0, "恢复后 slow_op(0)==0：{out3}");

    // ②e SEH：crash_op(0) → VEH 救援 0xC0000005，宿主存活
    let seh = sess
        .call_fn_addr(
            id,
            contract.fn_crash_op,
            nemesis_ziwei::dispatch::CONV_MSX64,
            [0, 0, 0, 0],
            5_000,
        )
        .await
        .expect("call crash_op");
    assert_eq!(seh["seh"], "0xC0000005", "SEH 码真值：{seh}");
    ev.log("② call_fn SEH", &seh);
    let header = sess
        .read_struct(id, contract.data_addr, &load_ds(&ws, id, "DataHeader"))
        .await
        .expect("SEH 后 read_struct（宿主存活）");
    assert_eq!(
        header["magic"],
        serde_json::json!(format!("{:#x}", contract.magic))
    );
    assert!(
        guard.0.try_wait().expect("try_wait").is_none(),
        "SEH 后目标进程必须存活"
    );
    ev.log("② SEH 后宿主存活", &header);

    // ── ③ hook push_record ≥1000 条逐条真值 ──
    let hs = sess
        .hook_start(id, "push_record")
        .await
        .expect("hook_start");
    ev.log("③ hook_start", &hs);
    let hook_id = hs["id"].as_u64().expect("hook id") as u32;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut all: Vec<serde_json::Value> = Vec::new();
    let mut total_dropped = 0u64;
    while all.len() < 1000 && std::time::Instant::now() < deadline {
        let pull = sess.poll_captures(id).await.expect("poll_captures");
        total_dropped += pull["dropped"].as_u64().unwrap_or(0);
        if let Some(evs) = pull["events"].as_array() {
            all.extend(
                evs.iter()
                    .map(|e| {
                        serde_json::json!({
                            "id": e["id"], "seq": e["seq"], "tid": e["tid"],
                            "regs": e["regs"], "decoded": e["decoded"],
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        }
        if all.len() < 1000 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    assert!(
        all.len() >= 1000,
        "60s 窗内应捕获 ≥1000 条（实际 {}）",
        all.len()
    );
    assert_eq!(total_dropped, 0, "3ms tick 下环不应溢出：{total_dropped}");
    // 逐条真值：id 恒定 / idx∈0..5 / delta∈1..=3 / seq 严格递增 / decode 魔法+计数
    let magic_str = format!("{:#x}", contract.magic);
    let mut prev_seq: u64 = 0;
    for (i, e) in all.iter().enumerate() {
        assert_eq!(e["id"], hook_id, "第 {i} 条 id");
        let regs = e["regs"].as_array().expect("regs");
        let idx = regs[1].as_u64().expect("idx");
        let delta = regs[2].as_u64().expect("delta");
        assert!(idx < 5, "第 {i} 条 idx 越域：{e}");
        assert!((1..=3).contains(&delta), "第 {i} 条 delta 越域：{e}");
        assert_eq!(
            e["decoded"]["magic"], magic_str,
            "第 {i} 条 decode 魔法：{e}"
        );
        assert_eq!(e["decoded"]["count"], 5, "第 {i} 条 decode count：{e}");
        let seq = e["seq"].as_u64().expect("seq");
        if i > 0 {
            assert!(
                seq > prev_seq,
                "第 {i} 条 seq 必须严格递增（{prev_seq}→{seq}）"
            );
        }
        prev_seq = seq;
    }
    ev.log(
        "③ hook 1000 逐条真值",
        &serde_json::json!({
            "total": all.len(),
            "dropped": total_dropped,
            "first": all[0],
            "last": all[all.len() - 1],
            "note": "逐条断言 id/idx/delta/seq 单调/decode 全过（全文太大只录首尾）",
        }),
    );
    // capture.jsonl 回流面
    let stored = sess.capture().read_recent(id, 5000);
    assert!(
        stored.len() >= 1000,
        "capture.jsonl 回流应 ≥1000（实际 {}）",
        stored.len()
    );
    ev.log_line(&format!("[capture.jsonl] 回流 {} 条", stored.len()));

    // ── ④ 2 级链收敛：feed_render::chain0 与顶层 1:1 ──
    let chain_out =
        nemesis_ziwei::runtime::invoke_hook_capture(&sess, &ws, id, "feed_render", None, 200)
            .await
            .expect("invoke_hook_capture feed_render");
    assert_eq!(chain_out["hooks_active"], 2, "顶层+链点双武装：{chain_out}");
    assert!(
        chain_out["anchors_skipped"].as_array().unwrap().is_empty(),
        "链锚不应被跳过：{chain_out}"
    );
    let hooks_now = attached.hooks();
    let id_top = hooks_now
        .iter()
        .find(|h| h.semantic == "feed_render")
        .expect("顶层绑定")
        .id as u64;
    let id_chain = hooks_now
        .iter()
        .find(|h| h.semantic == "feed_render::chain0")
        .expect("链点绑定")
        .id as u64;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    // 排空共享环（防两批之间残留导致计数不齐），统计两 id 事件数
    let mut cnt_top = 0u64;
    let mut cnt_chain = 0u64;
    for _ in 0..10 {
        let pull = sess.poll_captures(id).await.expect("chain poll");
        let evs = pull["events"].as_array().cloned().unwrap_or_default();
        let empty = evs.is_empty();
        for e in &evs {
            match e["id"].as_u64() {
                Some(v) if v == id_top => cnt_top += 1,
                Some(v) if v == id_chain => cnt_chain += 1,
                _ => {}
            }
        }
        if empty {
            break;
        }
    }
    assert!(cnt_top >= 50, "顶层 feed_render 应批量命中：{cnt_top}");
    assert_eq!(
        cnt_top, cnt_chain,
        "2 级链 1:1 收敛（每顶层调用恰好一次链点）：top={cnt_top} chain={cnt_chain}"
    );
    ev.log(
        "④ 2 级链收敛",
        &serde_json::json!({
            "hooks_active": chain_out["hooks_active"],
            "id_top": id_top, "id_chain": id_chain,
            "cnt_top": cnt_top, "cnt_chain": cnt_chain,
        }),
    );
    sess.hook_stop(id, "feed_render").await.expect("stop top");
    sess.hook_stop(id, "feed_render::chain0")
        .await
        .expect("stop chain");

    // ── ⑤ 退订即停：hook_stop(push_record) 后无新事件 ──
    sess.hook_stop(id, "push_record").await.expect("hook_stop");
    // 退订前在途积压的排空：三个 hook 累计的 pre-stop 事件经管道批量回流，
    // 慢机上固定窗+单次 poll 清不尽深环（实测间歇假红：满环 1024 条在
    // 600ms 固定窗后仍在上屏，快跑 12s 形态绿、慢跑 22s 形态红）。改循环
    // poll 至安静（5s 预算）再断言——「退订即停」语义不变，积压排净从
    // 祈祷式固定等待改成有界实证；真退订失效时 hook 持续产事件，循环
    // 永不见安静，断言照常红。
    let mut quiet = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let batch = sess.poll_captures(id).await.expect("drain");
        if batch["events"].as_array().map(|a| a.len()).unwrap_or(0) == 0 {
            quiet = true;
            break;
        }
    }
    assert!(
        quiet,
        "退订后在途积压 5s 内未排净（疑似退订未生效，hook 仍在产事件）"
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let after = sess.poll_captures(id).await.expect("post-stop poll");
    let n_after = after["events"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(n_after, 0, "退订后不得再有新事件：{after}");
    ev.log("⑤ 退订即停", &after);

    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
    ev.log_line("[result] PASS");
    ev.flush();
}

/// 从 profile 读结构定义（read_struct 入参）。
fn load_ds(ws: &Path, profile_id: &str, name: &str) -> DataStructure {
    ProfileStore::new(ws)
        .load(profile_id)
        .expect("profile 载入")
        .data_structures
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("结构 {name} 不在 profile"))
}

/// read_mem 裸字节读取（借 read_struct + u8[n] 临时结构）。
async fn read_bytes(sess: &SessionManager, profile_id: &str, addr: u64, n: usize) -> Vec<u8> {
    let ds = DataStructure {
        name: "RawBytes".into(),
        size: n as u32,
        fields: vec![FieldDef {
            offset: 0,
            name: "b".into(),
            type_: ft(&format!("u8[{n}]")),
            decode: None,
        }],
    };
    let v = sess
        .read_struct(profile_id, addr, &ds)
        .await
        .expect("read_bytes");
    v["b"]
        .as_array()
        .expect("u8 数组")
        .iter()
        .map(|x| x.as_u64().expect("u8") as u8)
        .collect()
}
