//! 真实二进制分析场景（本机 + 目标机双端同文件跑；用户验收的「两个真实
//! 场景」：本机分析 PE/ELF + 目标机分析 PE/ELF）。
//!
//! 与既有测试的分工：pe_dirs_tests 的真 PE 钉只咬 CFG 表解析器（解析器级）；
//! M4/M5 live 是自编 dummy 契约进程（attach 机制级）。本文件补上缺失的
//! 「真实系统二进制 → 完整分析链 → 查询面」端到端场景：
//!
//! - 场景 1 `real_pe_file_domain_full_chain`（跨平台）：真实 PE 文件域全链——
//!   BinHeaders → ImageMap（文件节字节合成快照形态，镜像 snapshot 的常驻
//!   三节选择）→ 六源种子 → 递归下降 → imports/xref/反查/CRCT 折叠。快照域
//!   语义纯文件跨主机：同一 PE 字节在 Windows 本机与 Linux 目标机必须产出
//!   同一摘要（`ZIWEI_REAL_PE_OUT` 落 JSON 对质）。
//! - 场景 2 `real_elf_honest_dispatch`（跨平台）：真实 ELF 的 P1 诚实分派——
//!   BinHeaders::Elf 变体 + e_machine 归一（容器解析器未实装，分派先到且
//!   上层 analyze 对该变体显式报错，不炸不静默）。
//! - 场景 3 `real_pe_live_setup_chain`（Windows-only）：真实系统进程走
//!   select_target → snapshot（活内存读）→ analyze → 查询 actions 全链，
//!   用系统 cmd.exe 而非自编 dummy。
//!
//! 样本定位：env 优先，缺席回落平台默认（PE=System32\notepad.exe，
//! ELF=/usr/bin/ls），再缺席 = 诚实 SKIP。样本预期为真实系统二进制
//! （函数数断言 ≥50 按此校准；指向上自编小 PE 会被数量断言拒绝）。

use std::path::PathBuf;

use nemesis_ziwei_rev::container::{self, BinHeaders};
use nemesis_ziwei_rev::disasm::Arch;
use nemesis_ziwei_rev::funcs;
use nemesis_ziwei_rev::pe::{self, ImageMap};
use nemesis_ziwei_rev::xref;

// ── 样本定位（env 优先 → 平台默认 → 诚实 SKIP）──

fn sample_pe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ZIWEI_REAL_PE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if cfg!(windows) {
        let p = PathBuf::from(r"C:\Windows\System32\notepad.exe");
        return p.is_file().then_some(p);
    }
    None
}

fn sample_elf() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ZIWEI_REAL_ELF") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if cfg!(target_os = "linux") {
        let p = PathBuf::from("/usr/bin/ls");
        return p.is_file().then_some(p);
    }
    None
}

// ── 场景 1：真实 PE 文件域全链（跨主机确定性）──

#[test]
fn real_pe_file_domain_full_chain() {
    let Some(path) = sample_pe() else {
        eprintln!("SKIP: 无真实 PE 样本（ZIWEI_REAL_PE 未设且平台默认缺席）");
        return;
    };
    let file = std::fs::read(&path).expect("样本可读");
    let hdrs = BinHeaders::parse(&file).expect("真实 PE 头应可解析");
    let BinHeaders::Pe(info) = hdrs else {
        panic!("PE 样本分派错误（落到非 Pe 变体）");
    };
    let arch = Arch::from_machine(info.machine).expect("machine 应归一");
    assert!(arch.disassembler_available(), "x64/x86 样本反汇编器应可用");

    // 文件域快照形态：常驻三节 raw 字节（镜像 snapshot 的节选择——attach
    // 路径从活内存读同三节；文件域从盘上取，非 packed 二进制字节一致）。
    let blobs: Vec<(String, Vec<u8>)> = info
        .sections
        .iter()
        .filter(|s| matches!(s.name.as_str(), ".text" | ".rdata" | ".data") && s.raw_size > 0)
        .filter_map(|s| {
            let start = s.raw_ptr as usize;
            let end = start + s.raw_size as usize;
            (end <= file.len()).then(|| (s.name.clone(), file[start..end].to_vec()))
        })
        .collect();
    assert!(!blobs.is_empty(), "真实 PE 应含常驻节");
    let img = ImageMap::new(info.clone(), blobs).expect("ImageMap 组装");
    let bounds = container::pe_text_bounds(&img).expect("应有 .text 可执行节界");

    // 六源种子装配（entry/export/pdata/cfg/tls/nopsled，缺席来源诚实缺席）
    let contribs = container::pe_seed_contribs(&img, &file);
    let mut seeds: Vec<u32> = Vec::new();
    for c in &contribs {
        seeds.extend_from_slice(&c.rvas);
    }
    seeds.sort_unstable();
    seeds.dedup();
    assert!(!seeds.is_empty(), "真实系统 PE 不应零种子");

    let set = funcs::recover(arch, &img, &seeds, bounds);
    // 递归下降从 entry 出发必收 entry 自身
    assert!(
        set.find_by_rva(info.entry_rva).is_some(),
        "entry 0x{:X} 应在函数集",
        info.entry_rva
    );
    // 全部函数起点落在 .text 界内（递归下降不出界）
    assert!(
        set.functions
            .iter()
            .all(|f| f.rva >= bounds.0 && f.rva < bounds.1),
        "函数起点应全部落在 .text 界内"
    );
    assert!(
        set.functions.len() >= 50,
        "真实系统 PE 函数数应健康（实得 {}）",
        set.functions.len()
    );

    let mut imports = pe::parse_imports_file(&file, &info).unwrap_or_default();
    imports.extend(pe::parse_delay_imports_file(&file, &info).unwrap_or_default());
    let idx = xref::build(&img, &set, &imports);
    let reverse = xref::build_reverse(&idx, &set);
    let crt = nemesis_ziwei_rev::crtsigs::fold(&img, &set, &imports);
    assert!(!imports.is_empty(), "系统 PE 应有导入表");
    assert!(!idx.import_calls.is_empty(), "call [iat] 归因应非空");
    assert!(
        !reverse.callers_of.is_empty(),
        "callers 反查索引应非空（真实二进制必有被调函数）"
    );

    // 跨主机摘要（同字节数据同结论）：ZIWEI_REAL_PE_OUT 指定时落盘，
    // 两端跑完 diff 对质。
    if let Ok(out) = std::env::var("ZIWEI_REAL_PE_OUT") {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&file);
        let by_source: std::collections::BTreeMap<&str, usize> =
            contribs.iter().map(|c| (c.source, c.rvas.len())).collect();
        let summary = serde_json::json!({
            "file_sha256": hex::encode(hasher.finalize()),
            "machine": format!("0x{:04X}", info.machine),
            "entry_rva": format!("0x{:X}", info.entry_rva),
            "seeds": seeds.len(),
            "seeds_by_source": by_source,
            "functions": set.functions.len(),
            "crt_folded": crt.len(),
            "import_dlls": imports.len(),
            "import_calls": idx.import_calls.len(),
            "str_refs": idx.str_refs.len(),
            "imm_consts": idx.imm_consts.len(),
            "data_refs": idx.data_refs.len(),
            "callers_keys": reverse.callers_of.len(),
        });
        std::fs::write(&out, serde_json::to_string_pretty(&summary).unwrap())
            .expect("跨主机摘要落盘");
        eprintln!("跨主机摘要 → {out}");
    }

    // CFG 种子来源对账（软断言：/guard:cf 系统二进制应有大清单；样本若
    // 指向非 CFG 的 PE 则诚实降级为注记，不做数量硬断言）。
    let cfg_count = contribs
        .iter()
        .find(|c| c.source == "cfg")
        .map(|c| c.rvas.len())
        .unwrap_or(0);
    if cfg_count < 40 {
        eprintln!(
            "NOTE: cfg 种子仅 {cfg_count} 条（样本非 /guard:cf 二进制属正常；notepad/cmd 应百级）"
        );
    }
}

// ── 场景 2：真实 ELF 诚实分派（P1 占位行为在真实 ELF 上实测）──

#[test]
fn real_elf_honest_dispatch() {
    let Some(path) = sample_elf() else {
        eprintln!("SKIP: 无真实 ELF 样本（ZIWEI_REAL_ELF 未设且平台默认缺席）");
        return;
    };
    let file = std::fs::read(&path).expect("样本可读");
    // 真实 ELF：魔数分派到 Elf 变体（P1 占位）——上层 analyze 对该变体
    // 显式 Err「ELF 容器解析器未启用（P1 批次实装）」，不炸不静默；此处
    // 钉的是分派层契约：真实 ELF 必然落到该诚实报错臂。
    let hdrs = BinHeaders::parse(&file).expect("真实 ELF 应分派成功");
    assert!(
        matches!(hdrs, BinHeaders::Elf),
        "真实 ELF 应分派 Elf 变体（诚实分派先到）"
    );
    // e_machine 归一：P1 占位 Elf 变体不带字段——arch() 诚实 None（不伪造
    // 机器类型；上层 analyze 在 arch 闸即报「machine 0x0000 无法归一」，
    // ELF 专属报错臂随 P1 字段化实装启用）。真实 ELF 全链今天的行为 =
    // 分派成功 + 上层诚实拒绝，不炸不静默。
    assert!(
        hdrs.arch().is_none(),
        "P1 占位 Elf 变体 arch() 应诚实 None（不带 e_machine 字段）"
    );
    eprintln!("ELF 样本 {path:?}: 分派 Elf ✓ + arch 诚实 None ✓（P1 占位契约）");
}

// ── 场景 3：真实 PE 活进程 setup 全链（Windows-only）──

// 进程 spawn/KillGuard 仅活进程腿使用（Linux 编译期消失，零 unused 警告）。
#[cfg(windows)]
use std::process::{Child, Command, Stdio};

/// panic 时也杀掉目标进程（测试失败不留孤儿 cmd）。
#[cfg(windows)]
struct KillGuard(Child);

#[cfg(windows)]
impl Drop for KillGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
fn temp_ws() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-real-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 真实系统进程走 select_target → snapshot → analyze → 查询全链。M4/M5
/// 证明 attach 机制用的是自编 dummy；本腿证明同一机制在真实二进制上成立：
/// 缺省用系统 cmd.exe（CFG/ASLR/大 .text）；`ZIWEI_REAL_PE_LIVE` 可指自编
/// 新编译样本（`--serve` 保活模式，见 ziwei_sample.rs）。
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn real_pe_live_setup_chain() {
    use std::sync::Arc;

    use nemesis_ziwei::session::SessionManager;
    use nemesis_ziwei::setup::SetupManager;

    let target = std::env::var("ZIWEI_REAL_PE_LIVE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\System32\cmd.exe"));
    if !target.is_file() {
        eprintln!("SKIP: {target:?} 缺席");
        return;
    }
    // cmd.exe 用 /K 保活；自编样本用内建 --serve 保活模式。
    let is_cmd = target
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("cmd.exe"));

    // 盘上样本独立真值先取（CFG 对账条件 + entry 断言都消费它）。
    let exe_file = std::fs::read(&target).expect("盘上样本可读");
    let pe_info = nemesis_ziwei_rev::pe::PeInfo::parse(&exe_file).expect("样本头解析");

    let ws = temp_ws();
    let sess = Arc::new(SessionManager::new(&ws, None, 2));
    nemesis_ziwei::session::set_global(sess);

    // /K：跑完 echo 后保活（stdin 管道挂起等待），窗口继承测试控制台。
    let child = Command::new(&target)
        .args(if is_cmd {
            &["/K", "echo ziwei-real-live"][..]
        } else {
            &["--serve"][..]
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn 目标进程");
    let pid = child.id();
    let _guard = KillGuard(child);

    let mgr = SetupManager::new();
    // 新进程模块/PEB 就绪窗口：select_target 带重试轮询。
    let mut selected = None;
    let mut last_err = String::new();
    for _ in 0..20 {
        match mgr.select_target(&ws, Some(pid), None).await {
            Ok(v) => {
                selected = Some(v);
                break;
            }
            Err(e) => {
                last_err = e;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    }
    if selected.is_none() {
        panic!("select_target 重试耗尽（pid {pid}）：{last_err}");
    }

    let snap = mgr.snapshot(&ws).await.expect("活内存快照");
    assert!(
        snap["sections"].as_array().is_some_and(|a| !a.is_empty()),
        "快照应含常驻节"
    );

    let ana = mgr.analyze(&ws).expect("analyze 全链");
    let functions = ana["functions"].as_u64().expect("functions 字段");
    assert!(
        functions >= 50,
        "递归下降应恢复健康函数数（实得 {functions}）"
    );
    // CFG 对账按样本真实 GFIDS 表条件化：盘上 exe 解析得出 ≥40 条表才硬
    // 断言（系统 CFG 二进制如 cmd.exe 百级）；自编样本（Rust 缺省无 CFG）
    // 诚实软注记，不做硬断言。
    let gfid_count = pe::parse_cfg_function_table(&exe_file, &pe_info)
        .map(|v| v.len())
        .unwrap_or(0);
    let cfg = ana["cfg_funcs"].as_u64().unwrap_or(0);
    if gfid_count >= 40 {
        assert!(
            cfg >= 40,
            "CFG 样本对账应健康（GFIDS {gfid_count} vs 实得 {cfg}）"
        );
    } else {
        eprintln!("NOTE: 样本无 GFIDS 表（{gfid_count} 条），cfg_funcs={cfg} 仅注记");
    }
    let import_dlls = ana["import_dlls"].as_u64().unwrap_or(0);
    assert!(
        import_dlls >= 2,
        "样本导入 DLL 应非零（实得 {import_dlls}）"
    );

    // 查询面 actions：functions → disasm（entry）→ callers（top 函数）。
    let funcs_top = mgr
        .query_functions(&ws, "refs", 5, None)
        .expect("query_functions");
    assert!(
        funcs_top["total"].as_u64().unwrap_or(0) > 0,
        "query_functions 应非空"
    );

    // entry 断言：与开头盘上头解析的独立真值对质（不依赖分析缓存）。
    let dis = mgr
        .query_disasm(&ws, pe_info.entry_rva, 8)
        .expect("query_disasm(entry)");
    assert!(
        dis["count"].as_u64().unwrap_or(0) >= 1,
        "entry 反汇编应非空"
    );
    let expect_entry = format!("0x{:X}", pe_info.entry_rva);
    assert_eq!(
        dis["func_rva"].as_str(),
        Some(expect_entry.as_str()),
        "entry 应归属 entry 函数自身"
    );

    let top_rva_s = funcs_top["functions"][0]["rva"]
        .as_str()
        .expect("top 行 rva");
    let top_rva = nemesis_ziwei::setup::parse_hex_u32(top_rva_s).expect("top rva 可解析");
    let callers = mgr.query_callers(&ws, top_rva).expect("query_callers");
    eprintln!(
        "live 全链 ✓ pid={pid} functions={functions} cfg={cfg} top={top_rva_s} callers_total={}",
        callers["total"].as_u64().unwrap_or(0)
    );

    let _ = mgr.abort(&ws).await;
}
