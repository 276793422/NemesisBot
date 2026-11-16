//!  回归锁：损坏 PE / 手改 profile 的
//! u32 算术溢出与空载荷扫描必须诚实降级，绝不 panic。
//!
//! 紫微系统的核心用途是分析任意第三方 exe（可能损坏/构造）——全部节值来自
//! 文件字节，saturating/checked/u64 算术是「损坏输入诚实降级」契约的一部分。
//! str_ref 空 text 走 load 侧不校验的手改/旧版 profile 路径（save 期校验
//! 只保护本方写入）。

use nemesis_ziwei_rev::diff::page_diff;
use nemesis_ziwei_rev::disasm::{Arch, leading_nop_bytes};
use nemesis_ziwei_rev::pe::{PeInfo, parse_relocations_file};
use nemesis_ziwei_rev::recover::{AnchorLite, rescan};

/// 最小 + 文件：头页 + 单个.rdata 节（PeInfo::parse 全路径可解）。
/// `sec_rva` 可注入损坏值（如 0xFFFF_FFFF）。
fn tiny_pe(sec_rva: u32, payload: &[u8]) -> Vec<u8> {
    let raw_ptr: usize = 0x400;
    let raw_size: usize = 0x20;
    let mut f = vec![0u8; raw_ptr + raw_size];
    f[0..2].copy_from_slice(b"MZ");
    let e_lfanew: usize = 0x80;
    f[0x3C..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
    f[e_lfanew..e_lfanew + 4].copy_from_slice(b"PE\0\0");
    let coff = e_lfanew + 4;
    f[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
    f[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 节
    let opt_size: usize = 240; // + 标准长度
    f[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
    let opt = coff + 20;
    f[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // + magic
    // 数据目录区（16×8）与节表由 parse 从 opt 布局推导，零值安全
    let sh = opt + opt_size;
    f[sh..sh + 6].copy_from_slice(b".rdata");
    f[sh + 8..sh + 12].copy_from_slice(&(raw_size as u32).to_le_bytes()); // vsize
    f[sh + 12..sh + 16].copy_from_slice(&sec_rva.to_le_bytes());
    f[sh + 16..sh + 20].copy_from_slice(&(raw_size as u32).to_le_bytes()); // raw_size
    f[sh + 20..sh + 24].copy_from_slice(&(raw_ptr as u32).to_le_bytes());
    f[sh + 36..sh + 40].copy_from_slice(&0x4000_0040u32.to_le_bytes()); // 只读初始化数据
    f[raw_ptr..raw_ptr + payload.len()].copy_from_slice(payload);
    f
}

fn anchor(kind: &str, text: Option<&str>) -> AnchorLite {
    AnchorLite {
        function: "f".to_string(),
        kind: kind.to_string(),
        pattern: None,
        text: text.map(str::to_string),
        value: None,
        imports: None,
    }
}

/// str_ref 空 text（load 侧不校验的手改/旧版 profile）
/// 必须诚实注记，绝不 windows(0) panic；且不毒化同批其他锚的投票。
#[test]
fn str_ref_empty_text_honest_note_no_panic() {
    let file = tiny_pe(0x1000, b"ABABABAB\0");
    // 单空锚：supported=1（注记非 bb_hash）、hits=0 → unrecoverable，无 panic
    let v = rescan(&file, &[anchor("str_ref", Some(""))]).expect("空 text 应诚实降级");
    assert_eq!(v.per_anchor[0].note, "str_ref 锚 text 为空");
    assert!(!v.per_anchor[0].hit);
    assert_eq!(v.verdict, "unrecoverable");
    // 混批：空锚不 panic、正常锚照常命中投票
    let v = rescan(
        &file,
        &[
            anchor("str_ref", Some("")),
            anchor("str_ref", Some("ABABABAB")),
        ],
    )
    .expect("混批应正常");
    assert_eq!(v.anchors_supported, 2);
    assert_eq!(v.anchors_hit, 1);
    assert_eq!(v.verdict, "partially_recoverable");
}

/// 同族：节基址顶到 u32::MAX + 节内多命中——命中 RVA
/// saturating 防溢出（旧代码 debug panic）。
#[test]
fn str_ref_hits_rva_saturating_near_max() {
    // "ABABABAB"：windows(6) 在 off 0 与 2 两处命中
    let file = tiny_pe(0xFFFF_FFFF, b"ABABABAB\0");
    let v = rescan(&file, &[anchor("str_ref", Some("ABABABAB"))]).expect("应可扫");
    let r = &v.per_anchor[0];
    assert!(r.hit);
    // 两处命中 saturate 到同一 RVA → dedup 后 1 处（诚实计数，不回绕成小值）
    assert_eq!(r.hit_count, 1);
    assert_eq!(r.found_rva, Some(u32::MAX));
    assert_eq!(v.verdict, "recoverable");
}

/// 同族：页 diff 基址近顶——页基 RVA 边界不回绕。
/// （saturating 防溢出与 str_ref 命中同型；页基 = base+页首偏移，i>1 的
/// 溢出路径需 2^31 级数据不可测，此处钉边界语义正确性。）
#[test]
fn page_diff_base_rva_saturating() {
    let before = [1u8, 2, 3];
    let after = [1u8, 2, 9];
    let pages = page_diff(&before, &after, 0xFFFF_FFFE, 0x1000);
    assert_eq!(pages.len(), 1);
    // 页基 = base_rva + 页首偏移（0）= 0xFFFF_FFFE，页内变化区段页相对
    assert_eq!(pages[0].rva, 0xFFFF_FFFE);
    assert_eq!(pages[0].ranges, vec![(2, 1)]);
}

/// 同族：前导 nop 扫描 rva 近顶——saturating 防溢出
/// （旧代码 off 推进后 rva+off panic）。
#[test]
fn leading_nops_rva_saturating() {
    let nops = [0x90u8; 8];
    // 旧代码：第二指令 rva = u32::MAX + 1 溢出 panic
    let total = leading_nop_bytes(Arch::X64, &nops, u32::MAX, 16);
    assert_eq!(total, 8);
}

/// 最小 PE 变体：单 .rdata 节（rva=0x1000，raw_ptr=0x400，raw_size 可调）
/// 加 dd[5]（重定位目录）指向节首。块头 8 字节（page/bsize）落 raw_ptr 处，
/// 条目紧随其后（body）。
fn reloc_pe(dir_size: u32, raw_size: usize, page: u32, bsize: u32, body: &[u8]) -> Vec<u8> {
    let raw_ptr: usize = 0x400;
    assert!(raw_size >= 8 && 8 + body.len() <= raw_size);
    let mut f = vec![0u8; raw_ptr + raw_size];
    f[0..2].copy_from_slice(b"MZ");
    let e_lfanew: usize = 0x80;
    f[0x3C..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
    f[e_lfanew..e_lfanew + 4].copy_from_slice(b"PE\0\0");
    let coff = e_lfanew + 4;
    f[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
    f[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 节
    let opt_size: usize = 240;
    f[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
    let opt = coff + 20;
    f[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // + magic
    // dd[5] = 重定位目录（0x20B → dd_off = opt+112；dd[5].rva 在 opt+152）
    f[opt + 152..opt + 156].copy_from_slice(&0x1000u32.to_le_bytes());
    f[opt + 156..opt + 160].copy_from_slice(&dir_size.to_le_bytes());
    let sh = opt + opt_size;
    f[sh..sh + 6].copy_from_slice(b".rdata");
    f[sh + 8..sh + 12].copy_from_slice(&(raw_size as u32).to_le_bytes()); // vsize
    f[sh + 12..sh + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // 节 rva
    f[sh + 16..sh + 20].copy_from_slice(&(raw_size as u32).to_le_bytes()); // raw_size
    f[sh + 20..sh + 24].copy_from_slice(&(raw_ptr as u32).to_le_bytes());
    f[sh + 36..sh + 40].copy_from_slice(&0x4000_0040u32.to_le_bytes());
    f[raw_ptr..raw_ptr + 4].copy_from_slice(&page.to_le_bytes());
    f[raw_ptr + 4..raw_ptr + 8].copy_from_slice(&bsize.to_le_bytes());
    f[raw_ptr + 8..raw_ptr + 8 + body.len()].copy_from_slice(body);
    f
}

/// 同族（BUG-20261116-023 红证）：BlockSize 声明 0xFFFF_FFF8 且块头后盘上
/// 0 可读（表被截断）——预分配必须按物理可读钳制，不得按声明值 GB 级
/// 分配（失败 = 进程 abort），且 0 可读 = 诚实 None（不再产出空块 Some）。
#[test]
fn reloc_declared_size_truncated_honest_none() {
    let f = reloc_pe(0xFFFF_FFF0, 8, 0x1000, 0xFFFF_FFF8, &[]);
    let pe = PeInfo::parse(&f).expect("PE 头应可解");
    assert!(parse_relocations_file(&f, &pe).is_none());
}

/// 同族（修复行为锁）：声明 n 巨大但盘上仅 12 个 u16 可读——条目数与
/// 预分配都按物理字节钳制，产出全部真实条目。
#[test]
fn reloc_entries_bounded_by_physical_bytes() {
    let mut body = Vec::new();
    for i in 0..12u16 {
        body.extend_from_slice(&(0xA000u16 | i).to_le_bytes());
    }
    let f = reloc_pe(0xFFFF_FFF0, 0x20, 0x1000, 0xFFFF_FFF8, &body);
    let pe = PeInfo::parse(&f).expect("PE 头应可解");
    let t = parse_relocations_file(&f, &pe).expect("物理截断应诚实可解");
    assert_eq!(t.blocks.len(), 1);
    assert_eq!(t.total_entries, 12);
    assert_eq!(t.blocks[0].offsets, (0..12).collect::<Vec<u16>>());
}

/// 防过钳：良构小表（bsize=16，4 条目含 1 padding）必须完整解析——
/// 物理钳制只砍声明超出盘面的部分，不伤正常表。
#[test]
fn reloc_well_formed_table_parsed() {
    let mut body = Vec::new();
    for v in [0xA004u16, 0xA008, 0x0000, 0xA00C] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    let f = reloc_pe(16, 16, 0x1000, 16, &body);
    let pe = PeInfo::parse(&f).expect("PE 头应可解");
    let t = parse_relocations_file(&f, &pe).expect("正常表应可解");
    assert_eq!(t.blocks.len(), 1);
    assert_eq!(t.blocks[0].page_rva, 0x1000);
    assert_eq!(t.blocks[0].offsets, vec![4u16, 8, 12]); // padding 已滤
    assert_eq!(t.total_entries, 3);
}
