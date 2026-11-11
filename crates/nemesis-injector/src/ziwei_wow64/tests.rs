use super::*;

/// 真文件解析：SysWOW64\kernel32.dll（x64 Windows 在场）→ LoadLibraryW
/// / GetProcAddress RVA 非零且互异。文件缺席（32 位机/ARM）→ 诚实 SKIP。
#[test]
fn syswow64_kernel32_export_rvas() {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let p = std::path::Path::new(&sys)
        .join("SysWOW64")
        .join("kernel32.dll");
    if !p.is_file() {
        eprintln!("SKIP: {} 不在场（非 x64 Windows）", p.display());
        return;
    }
    let rvas = pe_export_rvas(&p, &["LoadLibraryW", "GetProcAddress"]).unwrap();
    assert!(rvas[0] > 0x1000 && rvas[1] > 0x1000);
    assert_ne!(rvas[0], rvas[1]);
    // RVA 落在 32 位空间（< 4GB）——WOW64 kernel32 是
    assert!(rvas[0] < u32::MAX as u64);
}

/// 同位数路径冒烟：x64 自进程解析出的 LoadLibraryW 应落在已加载模块内
/// （非空、非典型栈/堆区段）。
#[cfg(target_arch = "x86_64")]
#[test]
fn x64_remote_api_resolves() {
    let (lo, gp) = resolve_remote_api(std::process::id(), "x64").unwrap();
    assert!(lo > 0x10000 && gp > 0x10000 && lo != gp);
}

/// 损坏 PE 回归（BUG-20261110-061）：节表 raw 声称范围远超实际文件长度时，
/// name_rva 命中超界节 → rva_to_off 返回超文件长度的偏移 → 名字扫描处
/// `data[noff..]` 裸切片 panic（修复前）/ Err「导出名未找齐」（修复后）。
///
/// 合成最小 PE（PE32 0x10B）：节 0 界内（承载导出目录 + 名字表），
/// 节 1 raw = 0x90000（数据仅 0x200）；name_rva 指向节 1 → 超界映射。
#[test]
fn corrupt_pe_section_raw_beyond_file_errs_not_panics() {
    let mut d = vec![0u8; 0x200];
    // DOS header
    d[0..2].copy_from_slice(b"MZ");
    d[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    // PE signature
    d[0x40..0x44].copy_from_slice(b"PE\0\0");
    // COFF @0x44：n_sections@0x46 = 2，opt header size@0x54 = 0xE0
    d[0x46..0x48].copy_from_slice(&2u16.to_le_bytes());
    d[0x54..0x56].copy_from_slice(&0xE0u16.to_le_bytes());
    // Optional header @0x58：magic 0x10B → data dirs @0xB8
    d[0x58..0x5A].copy_from_slice(&0x10Bu16.to_le_bytes());
    // export_rva = 0x1080（节 0 内，文件偏移 0x180）
    d[0xB8..0xBC].copy_from_slice(&0x1080u32.to_le_bytes());
    // 节 0 @0x138：vsize=0x100 vaddr=0x1000 raw=0x100（界内）
    d[0x138 + 8..0x138 + 12].copy_from_slice(&0x100u32.to_le_bytes());
    d[0x138 + 12..0x138 + 16].copy_from_slice(&0x1000u32.to_le_bytes());
    d[0x138 + 20..0x138 + 24].copy_from_slice(&0x100u32.to_le_bytes());
    // 节 1 @0x160：vsize=0x1000 vaddr=0x2000 raw=0x90000（>> 文件 0x200）
    d[0x160 + 8..0x160 + 12].copy_from_slice(&0x1000u32.to_le_bytes());
    d[0x160 + 12..0x160 + 16].copy_from_slice(&0x2000u32.to_le_bytes());
    d[0x160 + 20..0x160 + 24].copy_from_slice(&0x90000u32.to_le_bytes());
    // 导出目录内容 @0x180
    d[0x180 + 20..0x180 + 24].copy_from_slice(&1u32.to_le_bytes()); // n_funcs
    d[0x180 + 24..0x180 + 28].copy_from_slice(&1u32.to_le_bytes()); // n_names
    d[0x180 + 28..0x180 + 32].copy_from_slice(&0x10C0u32.to_le_bytes()); // funcs_rva
    d[0x180 + 32..0x180 + 36].copy_from_slice(&0x1020u32.to_le_bytes()); // names_rva（节 0，偏移 0x120）
    d[0x180 + 36..0x180 + 40].copy_from_slice(&0x1040u32.to_le_bytes()); // ords_rva（节 0，偏移 0x140）
    // @0x120：名字表首项 name_rva = 0x2080 → 命中超界节 1 → noff = 0x90080
    d[0x120..0x124].copy_from_slice(&0x2080u32.to_le_bytes());

    let p = std::env::temp_dir().join(format!("nb061_corrupt_pe_{}.dll", std::process::id()));
    std::fs::write(&p, &d).unwrap();
    let r = pe_export_rvas(&p, &["whatever"]);
    let _ = std::fs::remove_file(&p);
    let err = r.expect_err("损坏 PE 必须报错而非 panic / Ok");
    assert!(
        err.contains("导出名未找齐"),
        "超界名字应按缺失处理（Err 导出名未找齐），实际：{err}"
    );
}
