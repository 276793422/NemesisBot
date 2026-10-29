//! B2 目录解析面 + P0 容器分派单元回归钉。
//!
//! 查询面不暴露 relocs / CFG meta 变体——解析器本身在此直接断言。
//! PeInfo 字段全 pub，测试直构最小结构（不走完整 PE 夹具）。

use nemesis_ziwei_rev::container::BinHeaders;
use nemesis_ziwei_rev::disasm::Arch;
use nemesis_ziwei_rev::pe::{
    self, DIRECTORY_DEBUG, DIRECTORY_LOAD_CONFIG, DIRECTORY_RELOC, DIRECTORY_TLS, PeInfo,
    SectionInfo,
};

/// 最小 PeInfo：一节 .rdata（rva 0x2000 / raw 0x600 / 0x300B）+ 指定目录。
fn mini_pe(dirs: &[(usize, u32, u32)]) -> PeInfo {
    let mut directories = vec![(0u32, 0u32); 16];
    for (idx, rva, size) in dirs {
        directories[*idx] = (*rva, *size);
    }
    PeInfo {
        machine: 0x8664,
        image_base: 0x1400_0000,
        entry_rva: 0x1000,
        timestamp: 0,
        checksum: 0,
        sections: vec![SectionInfo {
            name: ".rdata".to_string(),
            rva: 0x2000,
            vsize: 0x300,
            raw_size: 0x300,
            raw_ptr: 0x600,
            characteristics: 0x4000_0040,
        }],
        directories,
    }
}

/// 文件容器：0x600..0x900 为 .rdata 原始字节。
fn mini_file() -> Vec<u8> {
    let mut f = vec![0u8; 0x900];
    f[0..2].copy_from_slice(b"MZ");
    f
}

#[test]
fn relocations_blocks_and_padding() {
    // 目录 @0x21F0 尺寸 22：块 1（14B，含 padding）+ 块 2（8B 纯头零条目）
    let pe = mini_pe(&[(DIRECTORY_RELOC, 0x21F0, 22)]);
    let mut f = mini_file();
    let w32 =
        |f: &mut Vec<u8>, off: usize, v: u32| f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let w16 =
        |f: &mut Vec<u8>, off: usize, v: u16| f[off..off + 2].copy_from_slice(&v.to_le_bytes());
    // 块 1 @rva 0x21F0（raw 0x7F0）：page 0x3000、两条目 + 一条 padding
    w32(&mut f, 0x7F0, 0x3000); // PageRVA
    w32(&mut f, 0x7F4, 14); // BlockSize = 8 + 3*2
    w16(&mut f, 0x7F8, (10 << 12) | 8); // DIR64 offset 8
    w16(&mut f, 0x7FA, 0); // ABSOLUTE padding（不计入）
    w16(&mut f, 0x7FC, (10 << 12) | 0x10); // DIR64 offset 0x10
    // 块 2 @rva 0x21FE（raw 0x7FE）：仅头（BlockSize 8，零条目）
    w32(&mut f, 0x7FE, 0x4000);
    w32(&mut f, 0x802, 8);
    let table = pe::parse_relocations_file(&f, &pe).expect("表应可解析");
    assert_eq!(table.blocks.len(), 2);
    // 块 1 只收 2 条非 padding
    assert_eq!(table.blocks[0].page_rva, 0x3000);
    assert_eq!(table.blocks[0].offsets, vec![8, 0x10]);
    // 块 2 零条目但块仍在册
    assert_eq!(table.blocks[1].page_rva, 0x4000);
    assert!(table.blocks[1].offsets.is_empty());
    assert_eq!(table.total_entries, 2);
}

#[test]
fn relocations_absent_directory_none() {
    let pe = mini_pe(&[]);
    let f = mini_file();
    assert!(pe::parse_relocations_file(&f, &pe).is_none());
}

#[test]
fn tls_callbacks_va_to_rva_and_termination() {
    let pe = mini_pe(&[(DIRECTORY_TLS, 0x2020, 40)]);
    let mut f = mini_file();
    let w64 =
        |f: &mut Vec<u8>, off: usize, v: u64| f[off..off + 8].copy_from_slice(&v.to_le_bytes());
    // TLS 目录 @0x2020：AddressOfCallBacks = VA(0x2100)
    w64(&mut f, 0x638, 0x1400_0000 + 0x2100);
    // 回调数组 @0x2100（raw 0x700）：两个回调 + NULL
    w64(&mut f, 0x700, 0x1400_0000 + 0x1060);
    w64(&mut f, 0x708, 0x1400_0000 + 0x1020);
    w64(&mut f, 0x710, 0);
    let cbs = pe::parse_tls_callbacks(&f, &pe).expect("回调应可解析");
    assert_eq!(cbs, vec![0x1060, 0x1020]);
}

#[test]
fn cfg_table_with_per_entry_metadata() {
    // GFIDS 条目 = 4 字节 RVA + GuardFlags 高 4 位字节元数据（此处 2 →
    // stride 6）——meta 处理钉。字段偏移对质 winnt.h：
    // GuardCFFunctionTable=+128/Count=+136/Flags=+144。
    let pe = mini_pe(&[(DIRECTORY_LOAD_CONFIG, 0x2050, 152)]);
    let mut f = mini_file();
    let w64 =
        |f: &mut Vec<u8>, off: usize, v: u64| f[off..off + 8].copy_from_slice(&v.to_le_bytes());
    let w32 =
        |f: &mut Vec<u8>, off: usize, v: u32| f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let lc = 0x650; // LoadConfig @rva 0x2050（152B → rva 0x20E8 止）
    w64(&mut f, lc + 128, 0x1400_0000 + 0x20F0); // GuardCFFunctionTable
    w64(&mut f, lc + 136, 3); // Count
    w32(&mut f, lc + 144, 2 << 28); // GuardFlags：stride = 4+2
    // 表 @0x20F0（raw 0x6F0，结构体界外零重叠——0x20E0 会撞 GuardFlags）：
    // 三条目，各 4B RVA + 2B 元数据
    w32(&mut f, 0x6F0, 0x1000);
    w32(&mut f, 0x6F6, 0x1020);
    w32(&mut f, 0x6FC, 0x1060);
    let cfg = pe::parse_cfg_function_table(&f, &pe).expect("CFG 表应可解析");
    assert_eq!(cfg, vec![0x1000, 0x1020, 0x1060]);
}

#[test]
fn debug_dir_rsds_pdb_path() {
    let pe = mini_pe(&[(DIRECTORY_DEBUG, 0x21A0, 28)]);
    let mut f = mini_file();
    let w32 =
        |f: &mut Vec<u8>, off: usize, v: u32| f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    // Debug 目录 @0x21A0（raw 0x7A0）：一条 CODEVIEW → RSDS @0x21C0（raw 0x7C0）
    w32(&mut f, 0x7AC, 2); // Type
    w32(&mut f, 0x7B0, 42); // SizeOfData
    w32(&mut f, 0x7B4, 0x21C0); // AddressOfRawData
    w32(&mut f, 0x7B8, 0x7C0); // PointerToRawData
    f[0x7C0..0x7C4].copy_from_slice(b"RSDS");
    let name = b"C:\\fake\\x.pdb";
    f[0x7D8..0x7D8 + name.len()].copy_from_slice(name); // 跳过 GUID16+age4
    f[0x7D8 + name.len()] = 0;
    let info = pe::parse_debug_file(&f, &pe).expect("debug 目录应可解析");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].ty, 2);
    assert_eq!(info.pdb_path.as_deref(), Some("C:\\fake\\x.pdb"));
}

#[test]
fn rich_header_key_and_prods() {
    let key: u32 = 0xAABB_CCDD;
    let mut hdr = vec![0u8; 0x100];
    hdr[0..2].copy_from_slice(b"MZ");
    // DanS 块 @0x40 + 两条指纹 @0x50/@0x58 + Rich+key @0x60
    hdr[0x40..0x44].copy_from_slice(&(0x536E_6144u32 ^ key).to_le_bytes());
    hdr[0x50..0x54].copy_from_slice(&(((24u32 << 16) | 1) ^ key).to_le_bytes());
    hdr[0x54..0x58].copy_from_slice(&(3u32 ^ key).to_le_bytes());
    hdr[0x58..0x5C].copy_from_slice(&(((64170u32 << 16) | 261) ^ key).to_le_bytes());
    hdr[0x5C..0x60].copy_from_slice(&(1u32 ^ key).to_le_bytes());
    hdr[0x60..0x64].copy_from_slice(&0x6863_6952u32.to_le_bytes()); // "Rich"
    hdr[0x64..0x68].copy_from_slice(&key.to_le_bytes());
    let info = pe::parse_rich(&hdr).expect("Rich 头应可解析");
    assert_eq!(info.key, key);
    assert_eq!(info.prods.len(), 2);
    assert_eq!(info.prods[0].prod_id, 1);
    assert_eq!(info.prods[0].build, 24);
    assert_eq!(info.prods[0].count, 3);
    assert_eq!(info.prods[1].prod_id, 261);
    assert_eq!(info.prods[1].build, 64170);
    // 无 Rich（全零 stub）= 诚实 None
    assert!(pe::parse_rich(&[0u8; 0x100]).is_none());
}

/// 真 PE 回归钉（防「夹具与解析器自洽、双双错误」复发）：CFG 表解析在
/// 真实系统二进制上必须产出健康清单——GFIDS 条目形态（4B RVA + meta）
/// 只有对真二进制才验得出。notepad.exe 缺席（非标准系统）= 诚实 SKIP。
#[cfg(windows)]
#[test]
fn cfg_table_real_pe_ground_truth() {
    let path = r"C:\Windows\System32\notepad.exe";
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("SKIP: {path} 不存在");
        return;
    };
    let Ok(BinHeaders::Pe(pe)) = BinHeaders::parse(&bytes) else {
        panic!("notepad.exe 头解析失败");
    };
    let Some(cfg) = pe::parse_cfg_function_table(&bytes, &pe) else {
        panic!("真 PE CFG 表应可解析（条目形态对质失败）");
    };
    // MSVC /guard:cf 二进制全函数权威清单：notepad 常态百余条；>=50 跨版本稳。
    assert!(
        cfg.len() >= 50,
        "CFG 表应产出健康清单（实得 {} 条）",
        cfg.len()
    );
    // 表本身按规范升序（解析后 sort+dedup 恒有序）；RVA 落在镜像节界内。
    assert!(cfg.windows(2).all(|w| w[0] < w[1]), "RVA 应严格升序");
    let max_rva = pe
        .sections
        .iter()
        .map(|s| s.rva.saturating_add(s.vsize))
        .max()
        .unwrap_or(0);
    assert!(
        cfg.iter().all(|r| *r > 0 && *r < max_rva),
        "RVA 应落镜像界内"
    );
}

#[test]
fn binheaders_magic_dispatch() {
    // ELF 魔数 → Elf 占位（P1 实装，分派先到）
    assert!(matches!(
        BinHeaders::parse(b"\x7fELF\x02\x01\x01rest..."),
        Ok(BinHeaders::Elf)
    ));
    // 未知魔数 → 诚实报错
    assert!(BinHeaders::parse(b"JUNKJUNKJUNK").is_err());
    // MZ 但头残缺 → PE 解析错误透传（Truncated 面，非魔数错）
    assert!(BinHeaders::parse(b"MZ").is_err());
}

#[test]
fn arch_namespaces_and_disassembler_gate() {
    // PE 命名空间：x64/x86/ARM64 machine
    assert_eq!(Arch::from_machine(0x8664), Some(Arch::X64));
    assert_eq!(Arch::from_machine(0x14C), Some(Arch::X86));
    assert_eq!(Arch::from_machine(0xAA64), Some(Arch::Arm64));
    assert_eq!(Arch::from_machine(0x0000), None);
    // ELF 命名空间：e_machine 归一同枚举
    assert_eq!(Arch::from_elf_machine(0x3E), Some(Arch::X64));
    assert_eq!(Arch::from_elf_machine(0xB7), Some(Arch::Arm64));
    // 反汇编器可用性闸：ARM64 诚实缺席
    assert!(Arch::X64.disassembler_available());
    assert!(!Arch::Arm64.disassembler_available());
}

// ── BUG-20261029-154 回归锁：损坏 PE 的 u32 算术溢出必须诚实降级非 panic ──

/// 节 rva 顶到地址空间顶部（rva+size 破 u32 域）：RVA 映射与节归属判定
/// 必须 u64 中间算术给出诚实结果（旧代码 debug panic / release 回绕）。
#[test]
fn rva_map_overflow_bounds_honest() {
    let pe = PeInfo {
        machine: 0x8664,
        image_base: 0x1400_0000,
        entry_rva: 0x1000,
        timestamp: 0,
        checksum: 0,
        sections: vec![SectionInfo {
            name: ".rdata".to_string(),
            rva: 0xFFFF_F000,
            vsize: 0x2000,
            raw_size: 0x2000,
            raw_ptr: 0x1000,
            characteristics: 0x4000_0040,
        }],
        directories: vec![(0u32, 0u32); 16],
    };
    // 旧代码：0xFFFF_F000 + 0x2000 u32 溢出。新代码：命中节内 → 有偏移。
    assert_eq!(pe.rva_to_file_offset(0xFFFF_F800), Some(0x1800));
    // 节界上沿之下（0xFFFF_F000+0x1FFF）仍命中
    assert_eq!(pe.rva_to_file_offset(0xFFFF_FFFF), Some(0x1FFF));
    // 节外（低于节基址 / 越过 u64 节界）→ 诚实 None
    assert_eq!(pe.rva_to_file_offset(0x0000_1000), None);
    assert_eq!(
        pe.section_by_rva(0xFFFF_F800).map(|s| s.name.as_str()),
        Some(".rdata")
    );
    assert!(pe.section_by_rva(0x0000_0800).is_none());
}

/// 导入描述符基址 di=1 溢出：checked 断走诚实 break（旧代码 debug panic）。
#[test]
fn imports_desc_base_overflow_honest_break() {
    // 节 rva 0xFFFF_F000 raw 0x1000 @raw_ptr 0x600；目录 @0xFFFF_FFF0
    let pe = PeInfo {
        machine: 0x8664,
        image_base: 0x1400_0000,
        entry_rva: 0x1000,
        timestamp: 0,
        checksum: 0,
        sections: vec![SectionInfo {
            name: ".rdata".to_string(),
            rva: 0xFFFF_F000,
            vsize: 0x1000,
            raw_size: 0x1000,
            raw_ptr: 0x600,
            characteristics: 0x4000_0040,
        }],
        directories: {
            let mut d = vec![(0u32, 0u32); 16];
            d[pe::DIRECTORY_IMPORT] = (0xFFFF_FFF0, 0x40);
            d
        },
    };
    let mut f = vec![0u8; 0x1800];
    let w32 =
        |f: &mut Vec<u8>, off: usize, v: u32| f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let w64 =
        |f: &mut Vec<u8>, off: usize, v: u64| f[off..off + 8].copy_from_slice(&v.to_le_bytes());
    // 描述符 di=0 @0xFFFF_FFF0（raw 0x15F0）：OriginalFirstThunk / 名 / FirstThunk
    w32(&mut f, 0x15F0, 0xFFFF_F300); // OriginalFirstThunk（节内）
    w32(&mut f, 0x15FC, 0xFFFF_F200); // Name RVA（raw 0x800）
    w32(&mut f, 0x1600, 0x0000_0000); // FirstThunk = 0 → 走名字表
    f[0x800..0x800 + 6].copy_from_slice(b"a.dll\0");
    w64(&mut f, 0x900, 0); // 名字表首槽 = 终止 → funcs 空
    // di=0 正常解析；di=1 desc 基址 0xFFFF_FFF0+20 溢出 → checked break
    let dlls = pe::parse_imports_file(&f, &pe).expect("损坏基址应诚实降级非 panic");
    assert_eq!(dlls.len(), 1);
    assert_eq!(dlls[0].dll, "a.dll");
    assert!(dlls[0].funcs.is_empty());
}

/// 重定位块游标走到 u32::MAX 边界：余量判定诚实止步（旧代码 cur+8 溢出）。
#[test]
fn relocations_cursor_top_of_space_honest_stop() {
    let pe = PeInfo {
        machine: 0x8664,
        image_base: 0x1400_0000,
        entry_rva: 0x1000,
        timestamp: 0,
        checksum: 0,
        sections: vec![SectionInfo {
            name: ".rdata".to_string(),
            rva: 0xFFFF_F000,
            vsize: 0x1000,
            raw_size: 0x1000,
            raw_ptr: 0x600,
            characteristics: 0x4000_0040,
        }],
        directories: {
            let mut d = vec![(0u32, 0u32); 16];
            d[DIRECTORY_RELOC] = (0xFFFF_FFF0, 16);
            d
        },
    };
    let mut f = vec![0u8; 0x1600];
    let w32 =
        |f: &mut Vec<u8>, off: usize, v: u32| f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    // 块 1 @0xFFFF_FFF0（raw 0x15F0）：仅头（BlockSize 8，零条目）
    w32(&mut f, 0x15F0, 0x3000);
    w32(&mut f, 0x15F4, 8);
    // 块游标推进到 0xFFFF_FFF8：end-cur = 7 < 8 → 诚实停（旧代码 cur+8 溢出 panic）
    let table = pe::parse_relocations_file(&f, &pe).expect("边界内应可解析");
    assert_eq!(table.blocks.len(), 1);
    assert_eq!(table.blocks[0].page_rva, 0x3000);
    assert!(table.blocks[0].offsets.is_empty());
}
