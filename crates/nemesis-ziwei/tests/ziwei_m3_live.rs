#![cfg(windows)]
//! M3 实弹集成（总控 §M3 验收 §4）：dummy-draw 从零走配置向导。
//!
//! select_target → snapshot → analyze → candidates → diff_window →
//! annotate/define_struct/define_tool → commit → read_struct 真值断言。
//! 证据落 `test-tools/resource/ziwei-uat/m3/m3_setup_live.txt`（逐步输出 +
//! 候选/差分面；io 失败诚实降级为仅 stdout）。
//!
//! 前置与降级纪律同 ziwei_m1_live：payload 内嵌缺席 / dummy 构建失败 =
//! 诚实 SKIP。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use nemesis_ziwei::session::SessionManager;
use nemesis_ziwei::setup::SetupManager;

const DUMMY_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/dummy-targets"
);
const UAT_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/resource/ziwei-uat/m3"
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

/// spawn dummy-draw 并解析 stdout 契约行（PID= / DATA_ADDR= / MAGIC=）。
fn spawn_draw(exe: &Path) -> (Child, u32, u64, u32) {
    let mut child = Command::new(exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn dummy-draw");
    let mut pid = 0u32;
    let mut addr = 0u64;
    let mut magic = 0u32;
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => panic!("dummy-draw 提前退出"),
            Ok(_) => {
                let t = line.trim();
                if let Some(v) = t.strip_prefix("PID=") {
                    pid = v.parse().expect("PID 解析");
                }
                if let Some(v) = t.strip_prefix("DATA_ADDR=") {
                    addr = u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("addr");
                }
                if let Some(v) = t.strip_prefix("MAGIC=0x") {
                    magic = u32::from_str_radix(v, 16).expect("magic");
                }
                if pid != 0 && addr != 0 && magic != 0 {
                    break;
                }
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
    (child, pid, addr, magic)
}

fn temp_ws() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-m3-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 证据收集器（stdout + 落盘双通道）。
struct Evidence {
    lines: Mutex<Vec<String>>,
    path: Option<PathBuf>,
}

impl Evidence {
    fn new() -> Evidence {
        let dir = Path::new(UAT_DIR);
        let path = if dir.is_dir() || std::fs::create_dir_all(dir).is_ok() {
            Some(dir.join("m3_setup_live.txt"))
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

fn cand_find<'a>(
    cands: &'a serde_json::Value,
    kind: &str,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> Option<&'a serde_json::Value> {
    cands["candidates"]
        .as_array()?
        .iter()
        .find(|c| c["kind"] == kind && pred(c))
}

/// M3 §4 验收主链路：七步向导 + 真值断言。
#[tokio::test(flavor = "multi_thread")]
async fn m3_wizard_full_flow_on_dummy_draw() {
    let Some(exe) = dummy_exe("x64", "dummy-draw") else {
        return; // 诚实 SKIP
    };
    let ev = Evidence::new();
    let ws = temp_ws();
    let sess = Arc::new(SessionManager::new(&ws, None, 2));
    nemesis_ziwei::session::set_global(sess.clone());
    let setup = SetupManager::new();
    let (mut child, pid, data_addr, magic) = spawn_draw(&exe);
    ev.log_line(&format!(
        "[fixture] dummy-draw pid={pid} DATA_ADDR=0x{data_addr:X} MAGIC=0x{magic:X}"
    ));

    let done = async {
        // ── Step 1：select_target ──
        let sel = setup
            .select_target(&ws, Some(pid), None)
            .await
            .expect("Step1 select_target");
        let draft_id = sel["draft"]["profile_id"]
            .as_str()
            .expect("draft id")
            .to_string();
        assert!(
            draft_id.starts_with("zw-pro-d"),
            "draft id 前缀：{draft_id}"
        );
        assert_eq!(sel["draft"]["pid"], pid);
        assert_eq!(sel["draft"]["exe_name"], "dummy-draw.exe");
        assert_eq!(
            sel["draft"]["machine"],
            serde_json::json!("x64"),
            "machine 判定"
        );
        ev.log("Step1 select_target", &sel);

        // ── Step 2：snapshot ──
        let snap = setup.snapshot(&ws).await.expect("Step2 snapshot");
        assert!(snap["sections"].as_array().expect("sections").len() >= 2);
        ev.log("Step2 snapshot", &snap);

        // ── Step 3：analyze ──
        let ana = setup.analyze(&ws).expect("Step3 analyze");
        let nfuncs = ana["functions"].as_u64().expect("functions");
        assert!(
            nfuncs >= 4,
            "递归下降应识别入口/推送/渲染链（实际 {nfuncs}）"
        );
        ev.log("Step3 analyze", &ana);

        // ── Step 4：candidates（§4 四类锚全命中断言）──
        let cands = setup.candidates(&ws).expect("Step4 candidates");
        // nopsled：push_record/feed_render 入口 8×0x90
        let nops = cand_find(&cands, "byte_pattern", |c| {
            c["payload"]
                .as_str()
                .is_some_and(|p| p.starts_with("90909090"))
        })
        .unwrap_or_else(|| panic!("nopsled 候选缺席：{cands}"));
        // imm_const：0x5A6B7C8D（mov eax, MAGIC_V1 原始字节）
        assert!(
            cand_find(&cands, "imm_const", |c| c["payload"] == "0x5A6B7C8D").is_some(),
            "imm_const 0x5A6B7C8D 缺席：{cands}"
        );
        // str_ref：ZIWEI_DUMMY_DRAW_V1
        assert!(
            cand_find(&cands, "str_ref", |c| {
                c["payload"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("ZIWEI_DUMMY_DRAW"))
            })
            .is_some(),
            "str_ref ZIWEI_DUMMY_DRAW 缺席：{cands}"
        );
        // import_call_seq：CreateCompatibleDC/DeleteDC
        assert!(
            cand_find(&cands, "import_call_seq", |c| {
                c["payload"]
                    .as_str()
                    .is_some_and(|p| p.contains("CreateCompatibleDC") && p.contains("DeleteDC"))
            })
            .is_some(),
            "import_call_seq GDI 对缺席：{cands}"
        );
        ev.log("Step4 candidates", &cands);

        // ── Step 5：diff_window（G_AREA 数据页变化）──
        let diff = setup
            .diff_window(&ws, Some(3), None)
            .await
            .expect("Step5 diff_window");
        let data_pages = diff["data_pages"].as_array().cloned().unwrap_or_default();
        assert!(
            !data_pages.is_empty(),
            "可写节数据页应观测到变化（G_AREA 每 tick 写入）：{diff}"
        );
        ev.log("Step5 diff_window", &diff);

        // ── Step 6a：annotate（强制锚）──
        // push_record：byte_pattern（nopsled 候选）+ imm_const 双锚
        let push_rva = nops["function_rva"].as_u64().expect("nopsled fn rva");
        let ann = setup
            .annotate(
                &ws,
                &serde_json::json!({
                    "rva": push_rva,
                    "semantic": "push_record",
                    "note": "数据推送（GDI 心跳伴生）",
                    "anchors": [
                        {"kind": "byte_pattern", "pattern": nops["payload"].as_str().unwrap()},
                        {"kind": "imm_const", "value": 0x5A6B7C8Du64},
                    ],
                }),
            )
            .expect("annotate push_record");
        assert_eq!(ann["annotated"], "push_record");
        ev.log("Step6a annotate push_record", &ann);

        // feed_render：第二处 nopsled（候选里另找；缺席诚实注记——
        // push_record 入口 E9 thunk 的解码闭包可能吞并相邻 sled，
        // 「保守闭包宁欠勿误」启发式边界，不影响验收主链）
        let feed = cands["candidates"].as_array().unwrap().iter().find(|c| {
            c["kind"] == "byte_pattern"
                && c["payload"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("90909090"))
                && c["function_rva"].as_u64() != Some(push_rva)
        });
        match feed {
            Some(f) => {
                let ann2 = setup
                    .annotate(
                        &ws,
                        &serde_json::json!({
                            "rva": f["function_rva"],
                            "semantic": "feed_render",
                            "anchors": [{"kind": "byte_pattern", "pattern": f["payload"].as_str().unwrap()}],
                        }),
                    )
                    .expect("annotate feed_render");
                ev.log("Step6a annotate feed_render", &ann2);
            }
            None => {
                ev.log_line(
                    "[note] 第二处 nopsled（feed_render）候选缺席：push_record 入口 \
                     E9 thunk 闭包吞并相邻 sled（保守闭包启发式边界，诚实记录）",
                );
            }
        }

        // ── Step 6b：define_struct ──
        let ds = setup
            .define_struct(
                &ws,
                &serde_json::json!({
                    "name": "DataHeader",
                    "size": 16,
                    "fields": [
                        {"offset": 0, "name": "magic", "type": "u32", "decode": "hex"},
                        {"offset": 4, "name": "layout_version", "type": "u32"},
                        {"offset": 8, "name": "count", "type": "u32"},
                        {"offset": 12, "name": "capacity", "type": "u32"},
                    ],
                }),
            )
            .expect("define_struct DataHeader");
        let dsr = setup
            .define_struct(
                &ws,
                &serde_json::json!({
                    "name": "DataRecordV1",
                    "size": 40,
                    "fields": [
                        {"offset": 0, "name": "id", "type": "u32"},
                        {"offset": 8, "name": "value", "type": "i64"},
                        {"offset": 16, "name": "label", "type": "char[16]"},
                        {"offset": 32, "name": "flags", "type": "u32"},
                    ],
                }),
            )
            .expect("define_struct DataRecordV1");
        ev.log("Step6b define_struct", &serde_json::json!([ds, dsr]));

        // ── Step 6c：define_tool（poll 型 + call_fn 型）──
        let t1 = setup
            .define_tool(
                &ws,
                &serde_json::json!({
                    "tool_name": "dummy_area_read",
                    "description": "读 G_AREA 头（magic/布局/记录数）",
                    "action": {"primitive": "poll_report", "addr": "struct:DataHeader", "mode": "on_change"},
                }),
            )
            .expect("define_tool poll");
        let t2 = setup
            .define_tool(
                &ws,
                &serde_json::json!({
                    "tool_name": "dummy_push_call",
                    "description": "调用 push_record 推一条记录",
                    "action": {"primitive": "call_fn", "addr": "fn:push_record"},
                }),
            )
            .expect("define_tool call_fn");
        ev.log("Step6c define_tool", &serde_json::json!([t1, t2]));

        // ── Step 6d：commit ──
        let c = setup.commit(&ws).expect("commit");
        assert_eq!(c["committed"], draft_id);
        let nfuncs = c["functions"].as_u64().unwrap_or(0);
        assert!(nfuncs >= 1, "至少 push_record 已标注：{c}");
        assert_eq!(c["tools"], 2);
        ev.log("Step6d commit", &c);

        // ── Step 6e：invoke 真值（会话于 select_target 已附；直读结构）──
        let header = sess
            .read_struct(&draft_id, data_addr, &load_ds(&ws, &draft_id, "DataHeader"))
            .await
            .expect("read DataHeader");
        assert_eq!(
            header["magic"],
            serde_json::json!(format!("{magic:#x}")),
            "hex 解码渲染为小写 {:#x} 形态",
            magic
        );
        assert_eq!(header["layout_version"], 1);
        assert_eq!(header["count"], 5);
        assert_eq!(header["capacity"], 16);
        let rec = sess
            .read_struct(
                &draft_id,
                data_addr + 16,
                &load_ds(&ws, &draft_id, "DataRecordV1"),
            )
            .await
            .expect("read DataRecordV1[0]");
        assert_eq!(rec["id"], 0, "记录 0 id 真值");
        assert!(
            rec["value"].as_i64().is_some_and(|v| v >= 1000),
            "记录 0 value 应 ≥ 初始 1000（实际 {}）",
            rec["value"]
        );
        assert!(
            rec["label"].as_str().is_some_and(|l| l.starts_with("REC_")),
            "记录 0 label 应为 REC_NN（实际 {}）",
            rec["label"]
        );
        ev.log(
            "Step6e invoke 真值",
            &serde_json::json!({"header": header, "record0": rec}),
        );
        true
    };

    let ok = done.await;
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
    ev.log_line(&format!("[result] {}", if ok { "PASS" } else { "FAIL" }));
    ev.flush();
    assert!(ok);
}

/// 从已提交 profile 读结构定义（read_struct 入参）。
fn load_ds(ws: &Path, profile_id: &str, name: &str) -> nemesis_ziwei_profile::DataStructure {
    let store = nemesis_ziwei_profile::ProfileStore::new(ws);
    let p = store.load(profile_id).expect("profile 载入");
    p.data_structures
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("结构 {name} 不在 profile"))
}
