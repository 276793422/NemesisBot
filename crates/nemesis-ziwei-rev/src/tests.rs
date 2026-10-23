//! nemesis-ziwei-rev 单测（M3）。
//!
//! 夹具 = 手工构造的 PE32+ 最小镜像（x64）：.text 带调用图/立即数/
//! rip 相对引用，.rdata 带导出表/导入表/字符串——pe/disasm/funcs/xref/
//! candidates/crtsigs/package/snapshot/diff 九模块全链路验证。

use crate::candidates;
use crate::crtsigs;
use crate::diff;
use crate::disasm::{self, Arch};
use crate::funcs;
use crate::package;
use crate::pe::{self, ImageMap};
use crate::snapshot;
use crate::xref;

// ── 夹具构造 ──────────────────────────────────────────────────────────

/// .rdata 内容（0x200 字节；偏移 = rva - 0x2000）。文件与内存同内容。
pub(crate) fn rdata_blob() -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    let w32 =
        |buf: &mut [u8], off: usize, v: u32| buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    // 导出目录 @0x2000
    w32(&mut b, 0x00, 0); // flags
    w32(&mut b, 0x04, 0x67000000); // timestamp
    b[0x08] = 1; // major
    b[0x0A] = 0; // minor
    w32(&mut b, 0x0C, 0x2040); // name rva（"test.dll"）
    w32(&mut b, 0x10, 1); // ordinal base
    w32(&mut b, 0x14, 1); // num funcs
    w32(&mut b, 0x18, 1); // num names
    w32(&mut b, 0x1C, 0x2060); // addr funcs
    w32(&mut b, 0x20, 0x2068); // addr names
    w32(&mut b, 0x24, 0x2070); // addr ords
    // "test.dll\0" @0x2040
    b[0x40..0x40 + 9].copy_from_slice(b"test.dll\0");
    // "ziwei_test_fn\0" @0x2050
    b[0x50..0x50 + 14].copy_from_slice(b"ziwei_test_fn\0");
    // 函数地址表 @0x2060
    w32(&mut b, 0x60, 0x1008);
    // 名字指针表 @0x2068
    w32(&mut b, 0x68, 0x2050);
    // 序号表 @0x2070
    w32(&mut b, 0x70, 0);
    // 导入描述符 @0x2080：OFT=0x20CC, name=0x20C0, IAT=0x20B0；@0x2094 终止项
    w32(&mut b, 0x80, 0x20CC);
    w32(&mut b, 0x8C, 0x20C0);
    w32(&mut b, 0x90, 0x20B0);
    // OFT 数组槽 @0x20CC（8B = hint/name rva 0x20DC），终止槽 @0x20D4 零
    b[0xCC..0xD4].copy_from_slice(&0x20DCu64.to_le_bytes());
    // IAT 数组槽 @0x20B0（同值），终止槽 @0x20B8 零（零填充天然满足）
    b[0xB0..0xB8].copy_from_slice(&0x20DCu64.to_le_bytes());
    // "USER32.dll" @0x20C0（null 由零填充满足）
    b[0xC0..0xC0 + 10].copy_from_slice(b"USER32.dll");
    // hint(2B)=0 @0x20DC + "MessageBoxA\0" @0x20DE（0xDE..0xEA）
    b[0xDE..0xDE + 12].copy_from_slice(b"MessageBoxA\0");
    // "ZIWEI_TEST_STRING\0" @0x20F0
    b[0xF0..0xF0 + 18].copy_from_slice(b"ZIWEI_TEST_STRING\0");
    b
}

/// .text 内容（0x100 字节；base rva 0x1000）。
/// 0x1000: call 0x1008; 0x1005-7: nop×3; 0x1008: mov eax,0x5A6B7C8D;
/// 0x100D: mov rax,[rip+0x10DC]（→0x20F0 串）; 0x1014: call [rip+0x1096]
/// （→IAT 0x20B0）; 0x101A: ret; 0x1020: nop×8 + ret（nopsled）。
pub(crate) fn text_blob() -> Vec<u8> {
    let mut b = vec![0xCC; 0x100];
    let mut p = 0usize;
    let put = |b: &mut Vec<u8>, p: &mut usize, bytes: &[u8]| {
        b[*p..*p + bytes.len()].copy_from_slice(bytes);
        *p += bytes.len();
    };
    put(&mut b, &mut p, &[0xE8, 0x03, 0x00, 0x00, 0x00]); // 0x1000 call +3
    put(&mut b, &mut p, &[0x90, 0x90, 0x90]); // 0x1005 nop×3
    put(&mut b, &mut p, &[0xB8, 0x8D, 0x7C, 0x6B, 0x5A]); // 0x1008 mov eax,imm32
    put(&mut b, &mut p, &[0x48, 0x8B, 0x05, 0xDC, 0x10, 0x00, 0x00]); // 0x100D → 0x20F0
    put(&mut b, &mut p, &[0xFF, 0x15, 0x96, 0x10, 0x00, 0x00]); // 0x1014
    put(&mut b, &mut p, &[0xC3]); // 0x101A ret
    // 0x1020 nopsled
    b[0x20..0x28].fill(0x90);
    b[0x28] = 0xC3;
    b
}

/// 完整 PE 文件字节（DOS + PE + 2 节头 + .text@0x200 + .rdata@0x400）。
pub(crate) fn test_pe_file() -> Vec<u8> {
    let mut f = vec![0u8; 0x600];
    f[0] = b'M';
    f[1] = b'Z';
    f[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes()); // e_lfanew
    f[0x80..0x84].copy_from_slice(b"PE\0\0");
    // COFF @0x84
    f[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes()); // machine
    f[0x86..0x88].copy_from_slice(&2u16.to_le_bytes()); // num sections
    f[0x88..0x8C].copy_from_slice(&0x67000000u32.to_le_bytes()); // timestamp
    f[0x94..0x96].copy_from_slice(&0xF0u16.to_le_bytes()); // optional size
    f[0x96..0x98].copy_from_slice(&0x0022u16.to_le_bytes()); // characteristics
    // Optional @0x98（PE32+，240 字节）
    let opt = 0x98usize;
    f[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // magic
    f[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry
    f[opt + 24..opt + 32].copy_from_slice(&0x140000000u64.to_le_bytes()); // base
    f[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // sect align
    f[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes()); // file align
    let dd = opt + 112;
    f[dd..dd + 4].copy_from_slice(&0x2000u32.to_le_bytes()); // export dir rva
    f[dd + 4..dd + 8].copy_from_slice(&0x40u32.to_le_bytes()); // export size
    f[dd + 8..dd + 12].copy_from_slice(&0x2080u32.to_le_bytes()); // import dir rva
    f[dd + 12..dd + 16].copy_from_slice(&0x28u32.to_le_bytes()); // import size
    // 节表 @0x188（opt+240）
    let sh = 0x188usize;
    let sect = |f: &mut [u8],
                base: usize,
                name: &[u8; 8],
                vs: u32,
                rva: u32,
                raws: u32,
                ptr: u32,
                ch: u32| {
        f[base..base + 8].copy_from_slice(name);
        f[base + 8..base + 12].copy_from_slice(&vs.to_le_bytes());
        f[base + 12..base + 16].copy_from_slice(&rva.to_le_bytes());
        f[base + 16..base + 20].copy_from_slice(&raws.to_le_bytes());
        f[base + 20..base + 24].copy_from_slice(&ptr.to_le_bytes());
        f[base + 36..base + 40].copy_from_slice(&ch.to_le_bytes());
    };
    sect(
        &mut f,
        sh,
        b".text\0\0\0",
        0x1000,
        0x1000,
        0x200,
        0x200,
        0x6000_0020,
    );
    sect(
        &mut f,
        sh + 40,
        b".rdata\0\0",
        0x1000,
        0x2000,
        0x200,
        0x400,
        0x4000_0040,
    );
    // 节内容
    f[0x200..0x300].copy_from_slice(&text_blob());
    f[0x400..0x600].copy_from_slice(&rdata_blob());
    f
}

/// ImageMap（内存形态）。
fn test_image() -> ImageMap {
    let file = test_pe_file();
    let pe = pe::PeInfo::parse(&file).expect("夹具 PE 头可解析");
    ImageMap::new(
        pe,
        vec![
            (".text".to_string(), text_blob()),
            (".rdata".to_string(), rdata_blob()),
        ],
    )
    .expect("ImageMap 组装")
}

// ── pe ────────────────────────────────────────────────────────────────

#[test]
fn pe_headers_and_mappings() {
    let file = test_pe_file();
    let pe = pe::PeInfo::parse(&file).expect("parse");
    assert_eq!(pe.machine, 0x8664);
    assert_eq!(pe.entry_rva, 0x1000);
    assert_eq!(pe.image_base, 0x140000000);
    assert_eq!(pe.sections.len(), 2);
    assert_eq!(pe.sections[0].name, ".text");
    assert!(pe.sections[0].is_executable());
    assert!(!pe.sections[0].is_writable());
    // 磁盘映射：0x1000 → 0x200；0x2050 → 0x450
    assert_eq!(pe.rva_to_file_offset(0x1000), Some(0x200));
    assert_eq!(pe.rva_to_file_offset(0x2050), Some(0x450));
    assert_eq!(pe.rva_to_file_offset(0x9999), None);
    assert_eq!(pe.bitness(), Some(64));
}

#[test]
fn pe_exports_and_imports_from_file() {
    let file = test_pe_file();
    let pe = pe::PeInfo::parse(&file).unwrap();
    let exports = pe::parse_exports_file(&file, &pe).expect("exports");
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].name, "ziwei_test_fn");
    assert_eq!(exports[0].rva, 0x1008);
    let imports = pe::parse_imports_file(&file, &pe).expect("imports");
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].dll, "USER32.dll");
    assert_eq!(imports[0].funcs.len(), 1);
    assert_eq!(imports[0].funcs[0].0, "MessageBoxA");
    assert_eq!(imports[0].funcs[0].1, 0x20B0);
}

#[test]
fn pe_rejects_garbage() {
    assert!(pe::PeInfo::parse(&[0u8; 16]).is_err());
    assert!(pe::PeInfo::parse(b"MZ short").is_err());
    let mut not_pe = vec![0u8; 0x100];
    not_pe[0] = b'M';
    not_pe[1] = b'Z';
    not_pe[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    assert!(pe::PeInfo::parse(&not_pe).is_err()); // 无 PE 签名
}

#[test]
fn image_map_reads_by_rva() {
    let img = test_image();
    let s = img.read_rva(0x20F0, 20).expect("串 RVA 读取");
    assert!(s.starts_with(b"ZIWEI_TEST_STRING"));
    assert!(img.read_rva(0x9999, 4).is_none()); // 未映射节
}

// ── disasm ────────────────────────────────────────────────────────────

#[test]
fn disasm_decodes_fixture_text() {
    let text = text_blob();
    // decode_block 是线性解码器：上限 8 条即产出 8 条（ret 后不停——
    // 停块语义归 funcs 的递归下降，这里只验指令属性）
    let insns = disasm::decode_block(Arch::X64, &text, 0x1000, 8);
    assert_eq!(insns.len(), 8, "实际解码：{insns:#?}");
    assert!(insns[0].is_call());
    assert_eq!(insns[0].branch_target, Some(0x1008));
    assert_eq!(insns[1].len, 1);
    assert!(insns[1].is_nop());
    assert_eq!(insns[4].imm, Some(0x5A6B7C8D), "mov eax 立即数抽取");
    assert_eq!(insns[5].rip_rel_target, Some(0x20F0), "mov rax,[rip] 目标");
    assert_eq!(insns[6].rip_rel_target, Some(0x20B0), "call [rip] → IAT");
    assert!(insns[7].is_ret());
}

#[test]
fn disasm_nopsled_length() {
    let text = text_blob();
    let n = disasm::leading_nop_bytes(Arch::X64, &text[0x20..], 0x1020, 64);
    assert_eq!(n, 8);
    let n0 = disasm::leading_nop_bytes(Arch::X64, &text, 0x1000, 64);
    assert_eq!(n0, 0, "E8 开头无 nop 前导");
}

// ── funcs ─────────────────────────────────────────────────────────────

#[test]
fn funcs_recursive_descent_finds_call_target_and_nopsled() {
    let img = test_image();
    let set = funcs::recover(Arch::X64, &img, &[0x1000, 0x1020], (0x1000, 0x1100));
    let rvas: Vec<u32> = set.functions.iter().map(|f| f.rva).collect();
    assert!(rvas.contains(&0x1000), "入口函数：{rvas:?}");
    assert!(rvas.contains(&0x1020), "nopsled 种子函数");
    let f0 = set.find_by_rva(0x1000).unwrap();
    assert!(f0.calls.contains(&0x1008), "直接调用目标入 calls");
    assert!(f0.imms.contains(&0x5A6B7C8D), "立即数入 imms");
    assert!(f0.rip_refs.contains(&0x20F0), "rip-rel 数据引用");
    // 0x1008 是 0x1000 的体内地址：不再生成独立函数（重叠让位）
    assert!(set.find_by_rva(0x1008).is_none());
}

// ── xref ──────────────────────────────────────────────────────────────

#[test]
fn xref_attribs_import_call_and_string() {
    let img = test_image();
    let set = funcs::recover(Arch::X64, &img, &[0x1000], (0x1000, 0x1100));
    let file = test_pe_file();
    let pe = pe::PeInfo::parse(&file).unwrap();
    let imports = pe::parse_imports_file(&file, &pe).unwrap();
    let idx = xref::build(&img, &set, &imports);

    let mb = idx
        .import_calls
        .iter()
        .find(|c| c.func == "MessageBoxA")
        .expect("导入调用归因");
    assert_eq!(mb.dll, "USER32.dll");
    assert_eq!(mb.iat_rva, 0x20B0);
    assert_eq!(mb.insn_rva, 0x1014);

    let sr = idx
        .str_refs
        .iter()
        .find(|s| s.text.starts_with("ZIWEI_TEST_STRING"))
        .expect("字符串入索引");
    assert_eq!(sr.str_rva, 0x20F0);
    assert_eq!(sr.refs, vec![0x100D], "串引用归因到 mov rax,[rip] 指令");

    assert!(
        idx.imm_consts
            .iter()
            .any(|c| c.value == 0x5A6B7C8D && c.insn_rva == 0x1008)
    );
    assert!(idx.data_refs.iter().any(|d| d.data_rva == 0x20F0));
}

// ── crtsigs ───────────────────────────────────────────────────────────

#[test]
fn crtsigs_signature_match_and_fold() {
    // 空函数集 → 空折叠（防呆）
    assert!(crtsigs::fold(&test_image(), &funcs::FunctionSet::default(), &[]).is_empty());
    // 造一个函数：insns 空（不走导入路径）、函数体命中 security_check 签名
    let sig_bytes = [0x48u8, 0x3B, 0x0D, 1, 2, 3, 4, 5, 0x75, 0x10, 0xC3];
    let mut img = test_image();
    let mut body = vec![0u8; 0x100];
    body[..sig_bytes.len()].copy_from_slice(&sig_bytes);
    if let Some(p) = img.section_mut_pair(".text") {
        p.1 = body;
    }
    let mut f = funcs::FunctionSet::default();
    f.functions.push(funcs::FunctionInfo {
        rva: 0x1000,
        end_rva: 0x1000 + sig_bytes.len() as u32,
        insns: vec![],
        calls: vec![],
        imms: vec![],
        rip_refs: vec![],
        has_indirect_branch: false,
        truncated: false,
    });
    let folded = crtsigs::fold(&img, &f, &[]);
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].rva, 0x1000);
    assert!(folded[0].reason.starts_with("sig:"));
}

// ── candidates ────────────────────────────────────────────────────────

#[test]
fn candidates_nopsled_imm_and_str() {
    let img = test_image();
    let set = funcs::recover(Arch::X64, &img, &[0x1000, 0x1020], (0x1000, 0x1100));
    let file = test_pe_file();
    let pe = pe::PeInfo::parse(&file).unwrap();
    let imports = pe::parse_imports_file(&file, &pe).unwrap();
    let idx = xref::build(&img, &set, &imports);
    let cands = candidates::generate(&img, &set, &idx, &[], 32);

    // nopsled：0x1020 前导 nop×8
    let pat = cands
        .iter()
        .find(|c| c.kind == "byte_pattern" && c.function_rva == Some(0x1020))
        .expect("nopsled 候选");
    assert!(pat.payload.starts_with("90909090"));
    // imm_const：0x5A6B7C8D 独特立即数
    assert!(
        cands
            .iter()
            .any(|c| c.kind == "imm_const" && c.payload == "0x5A6B7C8D")
    );
    // str_ref：ZIWEI_TEST_STRING
    let sr = cands
        .iter()
        .find(|c| c.kind == "str_ref" && c.payload.starts_with("ZIWEI_TEST"))
        .expect("str_ref 候选");
    assert_eq!(sr.function_rva, Some(0x1000));
    // import_call_seq：夹具函数只有 1 个导入（<2 阈值）→ 无该类候选
    assert!(!cands.iter().any(|c| c.kind == "import_call_seq"));
}

// ── diff ──────────────────────────────────────────────────────────────

#[test]
fn diff_pages_and_ranges() {
    let before = vec![0u8; 0x2000];
    let mut after = before.clone();
    after[0x1010] = 0xAA; // 第二页（字节 0x1000..0x2000）
    after[0x1015] = 0xBB;
    after[0x1800] = 1; // 同页不同区段
    let pages = diff::page_diff(&before, &after, 0x1000, 0x1000);
    assert_eq!(pages.len(), 1);
    // 页基 RVA = base_rva + 页起始字节偏移（0x1000 + 0x1000）
    assert_eq!(pages[0].rva, 0x2000);
    assert_eq!(pages[0].bytes_changed, 3);
    // 页内区段：0x1010-0x1015（6 字节）与 0x1800（1 字节），页内相对
    assert_eq!(pages[0].ranges, vec![(0x10, 1), (0x15, 1), (0x800, 1)]);
    // 长度不齐不 panic（前缀对齐比较；长度差由调用方对比 len 诚实自查）
    let _ = diff::page_diff(&[0, 1], &[0, 1, 2], 0, 0x1000);
}

// ── package ───────────────────────────────────────────────────────────

#[test]
fn package_prompt_and_annotations() {
    let img = test_image();
    let set = funcs::recover(Arch::X64, &img, &[0x1000], (0x1000, 0x1100));
    let file = test_pe_file();
    let pe = pe::PeInfo::parse(&file).unwrap();
    let imports = pe::parse_imports_file(&file, &pe).unwrap();
    let idx = xref::build(&img, &set, &imports);
    let table = package::function_table(&set, &idx, &[], 16);
    assert_eq!(table.len(), 1);
    assert!(table[0].imports.iter().any(|s| s.contains("MessageBoxA")));

    let prompt = package::build_prompt("dummy.exe", "x64", &table, &[0x1000], 4096);
    assert!(prompt.contains("dummy.exe"));
    assert!(prompt.contains("RVA=1000"));
    assert!(prompt.contains("USER32.dll!MessageBoxA"));

    let reply = "RVA=1000 NAME=render_frame DESC=渲染一帧\n垃圾行\nRVA=zz NAME=x DESC=y";
    let ann = package::parse_annotations(reply);
    assert_eq!(ann.len(), 1, "解析失败行忽略");
    assert_eq!(ann[0].rva, 0x1000);
    assert_eq!(ann[0].name, "render_frame");

    // snippet 有界
    let snip = package::function_snippet(&set, 0x1000, 3).expect("snippet");
    assert_eq!(snip.lines().count(), 4, "3 条 + 截断注记");
}

// ── 种子补充（.pdata / nopsled，M3 验收驱动）──────────────────────────

#[test]
fn pdata_absent_returns_none_and_nopsled_seeds_found() {
    let img = test_image();
    // 夹具无异常目录 → None（x86 回落路径）
    assert!(pe::parse_pdata_file(&test_pe_file(), &img.pe).is_none());
    // nopsled @0x1020（8×0x90）入册
    let seeds = funcs::nopsled_seed_rvas(&img);
    assert!(seeds.contains(&0x1020), "nopsled 种子：{seeds:?}");
}

#[test]
fn pdata_seeds_function_starts() {
    let mut file = test_pe_file();
    // dd[3] = (0x2140, 24)：指向 .rdata 空闲区的小型 RUNTIME_FUNCTION 表
    let dd = 0x98 + 112;
    file[dd + 24..dd + 28].copy_from_slice(&0x2140u32.to_le_bytes());
    file[dd + 28..dd + 32].copy_from_slice(&24u32.to_le_bytes());
    let mut put = |off: usize, v: u32| file[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put(0x540, 0x1000); // rva 0x2140 @ file 0x540
    put(0x544, 0x1008);
    put(0x548, 0x2100); // unwind info rva（不消费，填合法值）
    put(0x54C, 0x1020);
    put(0x550, 0x1028);
    put(0x554, 0x2100);
    let pe = pe::PeInfo::parse(&file).unwrap();
    let t = pe::parse_pdata_file(&file, &pe).expect("pdata 表");
    assert_eq!(t, vec![(0x1000, 0x1008), (0x1020, 0x1028)]);
}

// ── snapshot ──────────────────────────────────────────────────────────

#[test]
fn snapshot_save_latest_sweep_roundtrip() {
    let root = std::env::temp_dir().join(format!("zwrev-snap-{}", std::process::id()));
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let store = snapshot::SnapshotStore::new(&ws, "zw-pro-t");

    let meta = snapshot::SnapshotMeta {
        profile_id: "zw-pro-t".into(),
        ts_rfc3339: "2026-10-23T00:00:00Z".into(),
        pid: 42,
        module_base: 0x140000000,
        machine: 0x8664,
        exe_path: r"C:\t\dummy.exe".into(),
        sections: vec![crate::pe::SectionInfo {
            name: ".text".into(),
            rva: 0x1000,
            vsize: 0x1000,
            raw_size: 0x200,
            raw_ptr: 0x200,
            characteristics: 0x6000_0020,
        }],
    };
    // 两份快照
    store
        .save(&meta, &[1, 2, 3], &[(".text".into(), vec![4; 16])], 1000)
        .unwrap();
    store
        .save(&meta, &[9, 9, 9], &[(".text".into(), vec![7; 16])], 2000)
        .unwrap();
    let (m, headers, sections) = store.latest().unwrap().expect("latest");
    assert_eq!(m.pid, 42);
    assert_eq!(headers, vec![9, 9, 9], "取最新");
    assert_eq!(sections[0].0, ".text");

    // 节名防御
    let bad = store.save(&meta, &[], &[("../evil".into(), vec![])], 3000);
    assert!(bad.is_err(), "路径注入节名拒绝");

    // 容量清扫：max_kept=1 → 留 2000（最新），1000 被删
    let rep = store.sweep(std::time::Duration::from_secs(3600), 1);
    assert_eq!(rep.kept, 1, "清扫报告：{rep:?}");
    let (m2, _, _) = store.latest().unwrap().unwrap();
    assert_eq!(m2.ts_rfc3339, "2026-10-23T00:00:00Z");

    // 清理（fsutil 韧性删除纪律）
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&root);
}
