//! 查询面（B1/B2）集成测试：纯文件种子（PE 夹具 + draft.json + 快照落盘），
//! 无活进程——`require_info` 磁盘重建 + `with_analysis` 自动 analyze 让
//! `SetupManager::new()` 直调查询方法即可走全链路。
//!
//! 夹具契约（手构最小 x64 PE，布局与断言一一对应）。image_base =
//! 0x140000000（标准 x64 默认基址，B2 的 VA→RVA 换算依赖它）。.text
//! @rva 0x1000（raw 0x400）：func0@0x1000 为 8 个 nop 加 mov eax,
//! 0xDEADBEEF 加对 func2@0x1030 的 call 加 ret；func2 为 mov rax,rcx 加对
//! func1@0x1020 的 call 加 ret；func1 为 lea rax,[rip+0xFE5]（指向 0x200C
//! 串）加 ret；tls_func@0x1060 为 mov eax,1 加 ret（由 TLS/CFG 种子发现，
//! 递归下降够不到）。递归下降经 call 链发现三函数，TLS/CFG 种子补出第四。
//! .rdata @rva 0x2000（raw 0x600）：RUNTIME_FUNCTION[0x1000,0x1013]、
//! "HELLO_ZIWEI\0"@0x200C（func1 rip-rel 引用）、TLS 目录@0x2020（回调数组
//! @0x2100 指向 0x1060）、LoadConfig@0x2048（GuardCF 表@0x20E0 四条目）、
//! 延迟导入描述符@0x2110（FAKE_DELAY.dll / delay_read）、debug 目录@0x21A0
//! （RSDS@0x21C0 → C:\fake\query.pdb）、重定位块@0x21F0。DOS stub 带
//! Rich 头（key=0x11112222，一条 {prod 1, build 24, count 1} 指纹）。
//! .data @rva 0x3000（raw 0x800）：UTF-16LE 宽串 L"query_wide_str"（B3）。

use std::path::PathBuf;

use nemesis_ziwei::setup::{SetupManager, parse_hex_u32, parse_imm_u64};
use nemesis_ziwei_profile::model::{
    Conv, FunctionDef, LocateKind, LocateSpec, Machine, Profile, ProfileStatus, SCHEMA_VERSION,
    TargetFingerprint, Verification, VerifyPolicy,
};
use nemesis_ziwei_rev::pe::SectionInfo;
use nemesis_ziwei_rev::snapshot::{SnapshotMeta, SnapshotStore};

const PROFILE_ID: &str = "zw-pro-d9001";

/// 隔离工作区（测试尾 Drop 清理；fsutil 韧性删除防 Windows 句柄挂起）。
struct Fixture {
    dir: PathBuf,
    #[allow(dead_code)] // mgr 字段仅为所有权；查询方法经 mgr 引用调用
    mgr: SetupManager,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = nemesis_utils::fsutil::remove_dir_all_retry(&self.dir);
    }
}

/// 手构最小 x64 PE → (完整文件, 头页, 节 blobs)。
fn build_pe() -> (Vec<u8>, Vec<u8>, Vec<(String, Vec<u8>)>) {
    const IMAGE_BASE: u64 = 0x1400_0000;
    let va = |rva: u64| IMAGE_BASE + rva;
    let mut f = vec![0u8; 0xA00];
    // DOS 头：MZ + e_lfanew=0x80
    f[0..2].copy_from_slice(b"MZ");
    f[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    // DOS stub Rich 头（B2 工具链指纹）：DanS 块@0x40 + 一条指纹@0x50 +
    // "Rich"+key@0x58 + 16B 对齐垫@0x60（e_lfanew=0x80 之前）
    let key: u32 = 0x1111_2222;
    f[0x40..0x44].copy_from_slice(&(0x536E_6144u32 ^ key).to_le_bytes()); // DanS^key
    f[0x50..0x54].copy_from_slice(&(((24u32 << 16) | 1) ^ key).to_le_bytes()); // id=build24/prod1
    f[0x54..0x58].copy_from_slice(&(1u32 ^ key).to_le_bytes()); // count=1
    f[0x58..0x5C].copy_from_slice(&0x6863_6952u32.to_le_bytes()); // "Rich"
    f[0x5C..0x60].copy_from_slice(&key.to_le_bytes());
    // PE 签名 + COFF（coff = e_lfanew+4；machine@+0 / num_sections@+2 / optional_size@+16）
    f[0x80..0x84].copy_from_slice(b"PE\0\0");
    f[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes()); // machine=x64
    f[0x86..0x88].copy_from_slice(&3u16.to_le_bytes()); // num_sections
    f[0x94..0x96].copy_from_slice(&240u16.to_le_bytes()); // optional_size
    f[0x96..0x98].copy_from_slice(&0x0022u16.to_le_bytes()); // characteristics
    // Optional header（PE32+）
    let opt = 0x98usize;
    f[opt..opt + 2].copy_from_slice(&0x020Bu16.to_le_bytes()); // PE32+ magic
    f[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry
    f[opt + 24..opt + 32].copy_from_slice(&IMAGE_BASE.to_le_bytes()); // image_base
    f[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // section_align
    f[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes()); // file_align
    f[opt + 56..opt + 60].copy_from_slice(&0x5000u32.to_le_bytes()); // size_image
    f[opt + 60..opt + 64].copy_from_slice(&0x400u32.to_le_bytes()); // size_headers
    f[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes()); // 数据目录数
    // 数据目录（dd[i] @ opt+112+i*8）
    let dd = |i: usize| opt + 112 + i * 8;
    f[dd(3)..dd(3) + 4].copy_from_slice(&0x2000u32.to_le_bytes()); // 异常表
    f[dd(3) + 4..dd(3) + 8].copy_from_slice(&12u32.to_le_bytes());
    f[dd(5)..dd(5) + 4].copy_from_slice(&0x21F0u32.to_le_bytes()); // 重定位
    f[dd(5) + 4..dd(5) + 8].copy_from_slice(&12u32.to_le_bytes());
    f[dd(6)..dd(6) + 4].copy_from_slice(&0x21A0u32.to_le_bytes()); // debug
    f[dd(6) + 4..dd(6) + 8].copy_from_slice(&28u32.to_le_bytes());
    f[dd(9)..dd(9) + 4].copy_from_slice(&0x2020u32.to_le_bytes()); // TLS
    f[dd(9) + 4..dd(9) + 8].copy_from_slice(&40u32.to_le_bytes());
    f[dd(10)..dd(10) + 4].copy_from_slice(&0x2048u32.to_le_bytes()); // LoadConfig
    f[dd(10) + 4..dd(10) + 8].copy_from_slice(&152u32.to_le_bytes());
    f[dd(13)..dd(13) + 4].copy_from_slice(&0x2110u32.to_le_bytes()); // 延迟导入
    f[dd(13) + 4..dd(13) + 8].copy_from_slice(&64u32.to_le_bytes());
    // 节表 @ opt+240
    let st = opt + 240;
    let secs = [
        (
            ".text",
            0x200u32,
            0x1000u32,
            0x200u32,
            0x400u32,
            0x6000_0020u32,
        ),
        (".rdata", 0x200, 0x2000, 0x200, 0x600, 0x4000_0040),
        (".data", 0x200, 0x3000, 0x200, 0x800, 0xC000_0040),
    ];
    for (i, (name, vsize, rva, raw_size, raw_ptr, chars)) in secs.iter().enumerate() {
        let e = st + i * 40;
        f[e..e + name.len()].copy_from_slice(name.as_bytes());
        f[e + 8..e + 12].copy_from_slice(&vsize.to_le_bytes());
        f[e + 12..e + 16].copy_from_slice(&rva.to_le_bytes());
        f[e + 16..e + 20].copy_from_slice(&raw_size.to_le_bytes());
        f[e + 20..e + 24].copy_from_slice(&raw_ptr.to_le_bytes());
        f[e + 36..e + 40].copy_from_slice(&chars.to_le_bytes());
    }
    // .text @raw 0x400
    let t = 0x400;
    for i in 0..8 {
        f[t + i] = 0x90; // func0 前导 nop 0x1000-0x1007
    }
    f[t + 0x08..t + 0x0D].copy_from_slice(&[0xB8, 0xEF, 0xBE, 0xAD, 0xDE]); // mov eax,0xDEADBEEF @0x1008
    f[t + 0x0D..t + 0x12].copy_from_slice(&[0xE8, 0x1E, 0x00, 0x00, 0x00]); // call 0x1030 @0x100D
    f[t + 0x12] = 0xC3; // ret @0x1012 → func0=[0x1000,0x1013)
    f[t + 0x20..t + 0x27].copy_from_slice(&[0x48, 0x8D, 0x05, 0xE5, 0x0F, 0x00, 0x00]); // lea rax,[rip+0xFE5]→0x200C @0x1020
    f[t + 0x27] = 0xC3; // ret @0x1027 → func1=[0x1020,0x1028)
    f[t + 0x30..t + 0x33].copy_from_slice(&[0x48, 0x8B, 0xC1]); // mov rax,rcx @0x1030
    f[t + 0x33..t + 0x38].copy_from_slice(&[0xE8, 0xE8, 0xFF, 0xFF, 0xFF]); // call 0x1020 @0x1033
    f[t + 0x38] = 0xC3; // ret @0x1038 → func2=[0x1030,0x1039)
    f[t + 0x60..t + 0x66].copy_from_slice(&[0xB8, 0x01, 0x00, 0x00, 0x00, 0xC3]); // tls_func=[0x1060,0x1066)
    // .rdata @raw 0x600：pdata RUNTIME_FUNCTION + C 串
    let r = 0x600;
    f[r..r + 4].copy_from_slice(&0x1000u32.to_le_bytes()); // .StartAddress
    f[r + 4..r + 8].copy_from_slice(&0x1013u32.to_le_bytes()); // .EndAddress
    f[r + 0x0C..r + 0x17].copy_from_slice(b"HELLO_ZIWEI"); // @0x200C
    f[r + 0x17] = 0x00; // NUL @0x2017
    // B2 数据目录内容（全部落在 .rdata 0x2000..0x2200 界内）。
    // w32/w64 以参数形式传 f（零捕获闭包——可变捕获会与下方直接写冲突）。
    let w32 =
        |f: &mut Vec<u8>, off: usize, v: u32| f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    let w64 =
        |f: &mut Vec<u8>, off: usize, v: u64| f[off..off + 8].copy_from_slice(&v.to_le_bytes());
    // TLS 目录 @0x2020（IMAGE_TLS_DIRECTORY64；AddressOfCallBacks=VA→0x2100）
    w64(&mut f, r + 0x20, va(0x3000)); // StartAddressOfRawData
    w64(&mut f, r + 0x28, va(0x3010)); // EndAddressOfRawData
    w64(&mut f, r + 0x30, va(0x3010)); // AddressOfIndex
    w64(&mut f, r + 0x38, va(0x2100)); // AddressOfCallBacks
    // LoadConfig @0x2048（winnt.h 字段对齐：GuardCF 表 VA=+128/Count=+136/
    // Flags=+144；152B 恰好到 CFG 表@0x20E0 起点，与 TLS 目录零重叠）
    w32(&mut f, r + 0x48, 152); // Size
    w64(&mut f, r + 0x48 + 128, va(0x20E0)); // GuardCFFunctionTable
    w64(&mut f, r + 0x48 + 136, 4); // GuardCFFunctionCount
    w32(&mut f, r + 0x48 + 144, 0); // GuardFlags（无 per-entry metadata）
    // CFG 表 @0x20E0：0x1000 / 0x1020 / 0x1030 / 0x1060（GFIDS 条目 = 4B RVA，
    // GuardFlags=0 → stride 4；真 PE 对质形态）
    w32(&mut f, r + 0xE0, 0x1000);
    w32(&mut f, r + 0xE4, 0x1020);
    w32(&mut f, r + 0xE8, 0x1030);
    w32(&mut f, r + 0xEC, 0x1060);
    // TLS 回调数组 @0x2100：[0x1060, NULL]
    w64(&mut f, r + 0x100, va(0x1060));
    w64(&mut f, r + 0x108, 0);
    // 延迟导入描述符 @0x2110（grAttrs=1 → RVA 形态）+ 全零终止项
    w32(&mut f, r + 0x110, 1); // grAttrs = dlattrRva
    w32(&mut f, r + 0x114, 0x2150); // szName
    w32(&mut f, r + 0x11C, 0x2170); // pIAT
    w32(&mut f, r + 0x120, 0x2160); // pINT
    // INT @0x2160 / IAT @0x2170：thunk→0x2180（RVA 形态）+ NULL
    w64(&mut f, r + 0x160, 0x2180);
    w64(&mut f, r + 0x168, 0);
    w64(&mut f, r + 0x170, 0x2180);
    w64(&mut f, r + 0x178, 0);
    // Debug 目录 @0x21A0：一条 CODEVIEW，载荷 RSDS @0x21C0（raw 0x7C0）
    let dbgr = r + 0x1A0;
    w32(&mut f, dbgr + 12, 2); // Type = IMAGE_DEBUG_TYPE_CODEVIEW
    w32(&mut f, dbgr + 16, 42); // SizeOfData
    w32(&mut f, dbgr + 20, 0x21C0); // AddressOfRawData
    w32(&mut f, dbgr + 24, 0x7C0); // PointerToRawData
    w32(&mut f, r + 0x1C0, 0x5344_5352); // "RSDS" magic（LE 字节序原样）
    w32(&mut f, r + 0x1D4, 1); // age（GUID 16B 保持全零）
    // 重定位块 @0x21F0：page 0x3000、BlockSize 12、一条 DIR64@offset 8
    w32(&mut f, r + 0x1F0, 0x3000); // PageRVA
    w32(&mut f, r + 0x1F4, 12); // BlockSize
    // 以下非常量字段直接写字节（闭包借用已在最后一次调用后释放）
    // DLL 名 "FAKE_DELAY.dll\0" @0x2150
    f[r + 0x150..r + 0x15E].copy_from_slice(b"FAKE_DELAY.dll");
    f[r + 0x15E] = 0;
    // IMAGE_IMPORT_BY_NAME @0x2180：hint 0 + "delay_read\0"
    f[r + 0x180..r + 0x182].copy_from_slice(&0u16.to_le_bytes());
    f[r + 0x182..r + 0x18C].copy_from_slice(b"delay_read");
    f[r + 0x18C] = 0;
    // RSDS 路径 "C:\fake\query.pdb\0" @0x21D8
    f[r + 0x1D8..r + 0x1E9].copy_from_slice(b"C:\\fake\\query.pdb"); // 18B
    f[r + 0x1E9] = 0;
    // DIR64 重定位项（type 10 offset 8）
    f[r + 0x1F8..r + 0x1FA].copy_from_slice(&((10u16 << 12) | 8).to_le_bytes());

    // .data @raw 0x800：UTF-16LE 宽串 L"query_wide_str"（B3；偶数位
    // 0x00 已是零初值，只写可打印字节）
    for (k, ch) in b"query_wide_str".iter().enumerate() {
        f[0x800 + k * 2] = *ch;
    }

    let headers = f[0..0x400].to_vec();
    let blobs = vec![
        (".text".to_string(), f[0x400..0x600].to_vec()),
        (".rdata".to_string(), f[0x600..0x800].to_vec()),
        (".data".to_string(), f[0x800..0xA00].to_vec()),
    ];
    (f, headers, blobs)
}

fn fixture(tag: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "ziwei-query-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let (file, headers, blobs) = build_pe();
    let exe_path = dir.join("query_fixture.exe");
    std::fs::write(&exe_path, &file).unwrap();

    // draft.json（含语义标注 func0 = push_data）
    let profile = Profile {
        schema_version: SCHEMA_VERSION,
        profile_id: PROFILE_ID.to_string(),
        name: "query-test".to_string(),
        created_at: chrono::Utc::now(),
        last_verified_at: chrono::Utc::now(),
        status: ProfileStatus::Active,
        target: TargetFingerprint {
            exe_name: "query_fixture.exe".to_string(),
            exe_path_hint: Some(exe_path.to_string_lossy().to_string()),
            machine: Machine::X64,
            file_sha256: None,
            version_resource: Default::default(),
            verify_policy: VerifyPolicy::HashOrVersion,
        },
        attach: Default::default(),
        functions: vec![FunctionDef {
            semantic: "push_data".to_string(),
            locate: LocateSpec {
                kind: LocateKind::Rva,
                module: None,
                pattern: None,
                rva: Some(0x1000),
            },
            conv: Conv::MsX64,
            args: vec![],
            capture: None,
        }],
        data_structures: vec![],
        tools: vec![],
        verification: Verification {
            on_attach: vec!["file_sha256".to_string()],
            stale_action: None,
        },
    };
    let draft_dir = dir.join("ziwei").join("drafts").join(PROFILE_ID);
    std::fs::create_dir_all(&draft_dir).unwrap();
    std::fs::write(
        draft_dir.join("draft.json"),
        serde_json::to_vec_pretty(&profile).unwrap(),
    )
    .unwrap();
    std::fs::write(
        draft_dir.join("anchors.json"),
        serde_json::to_vec_pretty(&nemesis_ziwei_profile::anchors::AnchorSet {
            profile_id: PROFILE_ID.to_string(),
            anchors: vec![],
        })
        .unwrap(),
    )
    .unwrap();

    // 快照落盘
    let store = SnapshotStore::new(&dir, PROFILE_ID);
    let meta = SnapshotMeta {
        profile_id: PROFILE_ID.to_string(),
        ts_rfc3339: chrono::Utc::now().to_rfc3339(),
        pid: 0,
        module_base: 0x1400_0000,
        machine: 0x8664,
        exe_path: exe_path.to_string_lossy().to_string(),
        sections: vec![
            SectionInfo {
                name: ".text".to_string(),
                rva: 0x1000,
                vsize: 0x200,
                raw_size: 0x200,
                raw_ptr: 0x400,
                characteristics: 0x6000_0020,
            },
            SectionInfo {
                name: ".rdata".to_string(),
                rva: 0x2000,
                vsize: 0x200,
                raw_size: 0x200,
                raw_ptr: 0x600,
                characteristics: 0x4000_0040,
            },
            SectionInfo {
                name: ".data".to_string(),
                rva: 0x3000,
                vsize: 0x200,
                raw_size: 0x200,
                raw_ptr: 0x800,
                characteristics: 0xC000_0040,
            },
        ],
    };
    store
        .save(&meta, &headers, &blobs, 1_700_000_000_000)
        .unwrap();

    Fixture {
        dir,
        mgr: SetupManager::new(),
    }
}

// ── 入参解析辅助 ────────────────────────────────────────────────────────

#[test]
fn parse_helpers_hex_and_imm() {
    assert_eq!(parse_hex_u32("0x1000"), Some(0x1000));
    assert_eq!(parse_hex_u32("0X1000"), Some(0x1000));
    assert_eq!(parse_hex_u32(" 1000 "), Some(0x1000));
    assert_eq!(parse_hex_u32("XYZ"), None);
    assert_eq!(parse_hex_u32(""), None);
    assert_eq!(parse_imm_u64("3735928559"), Some(0xDEAD_BEEF)); // 十进制优先
    assert_eq!(parse_imm_u64("0xDEADBEEF"), Some(0xDEAD_BEEF)); // hex 显式
    assert_eq!(parse_imm_u64("zz"), None);
}

// ── disasm ──────────────────────────────────────────────────────────────

#[test]
fn disasm_at_func_start() {
    let fx = fixture("disasm-start");
    let out = fx.mgr.query_disasm(&fx.dir, 0x1000, 512).unwrap();
    assert_eq!(out["func_rva"], "0x1000");
    assert_eq!(out["func_name"], "push_data"); // 语义名装饰
    assert_eq!(out["total_insns"], 11); // 8 nop + mov + call + ret
    assert_eq!(out["truncated"], false);
    let insns = out["instructions"].as_array().unwrap();
    assert_eq!(insns[0]["m"], "nop");
    // mov eax, imm32：操作数投影含立即数
    let mov = &insns[8];
    assert_eq!(mov["m"], "mov");
    assert_eq!(mov["ops"].as_array().unwrap().len(), 2);
    assert_eq!(mov["ops"][1], "0xDEADBEEF");
    // call 直接目标 → func 三态
    let call = &insns[9];
    assert_eq!(call["m"], "call");
    assert_eq!(call["target"]["kind"], "func");
    assert_eq!(call["target"]["rva"], "0x1030");
}

#[test]
fn disasm_body_address_resolves_to_start_with_note() {
    let fx = fixture("disasm-body");
    let out = fx.mgr.query_disasm(&fx.dir, 0x1004, 512).unwrap();
    assert_eq!(out["func_rva"], "0x1000"); // 按起点解析
    assert!(out["note"].as_str().unwrap().contains("体内"));
}

#[test]
fn disasm_unowned_address_honest_error() {
    let fx = fixture("disasm-unowned");
    let err = fx.mgr.query_disasm(&fx.dir, 0x1015, 512).unwrap_err();
    assert!(err.contains("不在任何已识别函数"), "err={err}");
}

// ── xref_to / xref_from / callers ───────────────────────────────────────

#[test]
fn xref_to_imm_hex_and_sites() {
    let fx = fixture("xref-imm");
    let out = fx.mgr.query_xref_to(&fx.dir, "imm", "0xDEADBEEF").unwrap();
    assert_eq!(out["total"], 1);
    assert_eq!(out["hits"][0]["site"], "0x1008");
    assert_eq!(out["hits"][0]["func"], "0x1000");
    assert_eq!(out["hits"][0]["name"], "push_data");
}

#[test]
fn xref_to_str_two_hop_via_data_refs() {
    let fx = fixture("xref-str");
    let out = fx.mgr.query_xref_to(&fx.dir, "str", "HELLO_ZIWEI").unwrap();
    assert_eq!(out["query"]["str_rvas"][0], "0x200C");
    assert_eq!(out["total"], 1);
    assert_eq!(out["hits"][0]["site"], "0x1020");
    assert_eq!(out["hits"][0]["func"], "0x1020");
}

#[test]
fn xref_to_data_rva() {
    let fx = fixture("xref-data");
    let out = fx.mgr.query_xref_to(&fx.dir, "data", "0x200C").unwrap();
    assert_eq!(out["total"], 1);
    assert_eq!(out["hits"][0]["site"], "0x1020");
}

#[test]
fn xref_to_no_hits_note() {
    let fx = fixture("xref-empty");
    let out = fx.mgr.query_xref_to(&fx.dir, "data", "0x3000").unwrap();
    assert_eq!(out["total"], 0);
    assert!(out["note"].as_str().unwrap().contains("无引用站点"));
}

#[test]
fn xref_to_bad_kind_rejected() {
    let fx = fixture("xref-badkind");
    let err = fx.mgr.query_xref_to(&fx.dir, "junk", "0").unwrap_err();
    assert!(err.contains("非法"), "err={err}");
}

#[test]
fn xref_from_func0_lists_imm() {
    let fx = fixture("xref-from");
    let out = fx.mgr.query_xref_from(&fx.dir, 0x1000).unwrap();
    assert_eq!(out["func_rva"], "0x1000");
    assert_eq!(out["imm_consts"][0], "0xDEADBEEF");
    assert_eq!(out["strings"].as_array().unwrap().len(), 0);
}

#[test]
fn xref_from_func1_lists_string_and_data() {
    let fx = fixture("xref-from-1");
    let out = fx.mgr.query_xref_from(&fx.dir, 0x1020).unwrap();
    assert_eq!(out["strings"][0]["str_rva"], "0x200C");
    assert_eq!(out["strings"][0]["text"], "HELLO_ZIWEI");
    assert_eq!(out["strings"][0]["wide"], false);
    assert_eq!(out["data_refs"][0], "0x200C");
}

#[test]
fn callers_and_no_caller_note() {
    let fx = fixture("callers");
    let out = fx.mgr.query_callers(&fx.dir, 0x1020).unwrap();
    assert_eq!(out["total"], 1);
    assert_eq!(out["callers"][0]["rva"], "0x1030");

    let out0 = fx.mgr.query_callers(&fx.dir, 0x1000).unwrap();
    assert_eq!(out0["total"], 0);
    assert!(out0["note"].as_str().unwrap().contains("无直接调用方"));
}

// ── search ──────────────────────────────────────────────────────────────

#[test]
fn search_bytes_with_wildcard() {
    let fx = fixture("search-bytes");
    // 8 连 nop：3 字节模式命中起点 0x1000..0x1005 共 6 处
    let out = fx.mgr.query_search(&fx.dir, "bytes", "909090").unwrap();
    assert_eq!(out["count"], 6);
    assert_eq!(out["hits"][0]["site"], "0x1000");
    assert_eq!(out["hits"][0]["func"], "0x1000");
    // ?? 通配：90 ?? 90 同样 6 处
    let out2 = fx.mgr.query_search(&fx.dir, "bytes", "90 ?? 90").unwrap();
    assert_eq!(out2["count"], 6);
}

#[test]
fn search_imm_decimal_and_hex() {
    let fx = fixture("search-imm");
    let dec = fx.mgr.query_search(&fx.dir, "imm", "3735928559").unwrap();
    assert_eq!(dec["hits"][0]["site"], "0x1008");
    let hex = fx.mgr.query_search(&fx.dir, "imm", "0xDEADBEEF").unwrap();
    assert_eq!(hex["hits"][0]["site"], "0x1008");
}

#[test]
fn search_string_exact() {
    let fx = fixture("search-str");
    let out = fx
        .mgr
        .query_search(&fx.dir, "string", "HELLO_ZIWEI")
        .unwrap();
    assert_eq!(out["count"], 1);
    assert_eq!(out["hits"][0]["str_rva"], "0x200C");
    assert_eq!(out["hits"][0]["wide"], false);
    assert_eq!(out["hits"][0]["refs"][0]["site"], "0x1020");
}

#[test]
fn search_string_wide_and_summary_count() {
    let fx = fixture("search-wide");
    // 宽串文本直接命中（UTF-16LE 扫描进同一 StrRef 索引，B3）
    let out = fx
        .mgr
        .query_search(&fx.dir, "string", "query_wide_str")
        .unwrap();
    assert_eq!(out["count"], 1);
    assert_eq!(out["hits"][0]["str_rva"], "0x3000");
    assert_eq!(out["hits"][0]["wide"], true);
    // analyze summary：总含宽串；宽串计数单独可见
    // 5 ASCII = HELLO_ZIWEI / FAKE_DELAY.dll / delay_read / C:\fake\query.pdb
    // + 'RSDS'（debug magic 4 字节 printable run 也入索引——扫描器对 magic
    // 字节无豁免，真实 PE 同形，无害）
    let sum = fx.mgr.analyze(&fx.dir).unwrap();
    assert_eq!(sum["strings"], 6);
    assert_eq!(sum["strings_wide"], 1);
}

#[test]
fn search_name_substring_case_insensitive() {
    let fx = fixture("search-name");
    let out = fx.mgr.query_search(&fx.dir, "name", "PUSH").unwrap();
    assert_eq!(out["count"], 1);
    assert_eq!(out["hits"][0]["source"], "annotated");
    assert_eq!(out["hits"][0]["rva"], "0x1000");
}

#[test]
fn search_insn_mnemonic() {
    let fx = fixture("search-insn");
    let out = fx.mgr.query_search(&fx.dir, "insn", "nop").unwrap();
    assert_eq!(out["count"], 8); // func0 的 8 个 nop 站点
    assert_eq!(out["hits"][0]["func"], "0x1000");
}

#[test]
fn search_bad_kind_rejected() {
    let fx = fixture("search-badkind");
    let err = fx.mgr.query_search(&fx.dir, "regex", "x").unwrap_err();
    assert!(err.contains("非法"), "err={err}");
}

// ── read_data ───────────────────────────────────────────────────────────

#[test]
fn read_data_hex_and_utf8() {
    let fx = fixture("read-data");
    let hex_out = fx.mgr.query_read_data(&fx.dir, 0x200C, 12, false).unwrap();
    assert_eq!(hex_out["data"], "48454c4c4f5f5a4957454900"); // "HELLO_ZIWEI\0"
    assert_eq!(hex_out["section"], ".rdata");
    assert_eq!(hex_out["requested"], 12);
    let utf8_out = fx.mgr.query_read_data(&fx.dir, 0x200C, 12, true).unwrap();
    assert_eq!(utf8_out["data"], "HELLO_ZIWEI\u{0}");
}

#[test]
fn read_data_outside_sections_honest_error() {
    let fx = fixture("read-data-oob");
    let err = fx
        .mgr
        .query_read_data(&fx.dir, 0x9000, 16, false)
        .unwrap_err();
    assert!(err.contains("不在快照节内"), "err={err}");
}

// ── pe ──────────────────────────────────────────────────────────────────

#[test]
fn pe_sections_pdata_exports_imports() {
    let fx = fixture("pe-four");
    let secs = fx.mgr.query_pe(&fx.dir, "sections").unwrap();
    assert_eq!(secs["count"], 3);
    assert_eq!(secs["sections"][0]["name"], ".text");
    assert_eq!(secs["sections"][0]["exec"], true);

    let pdata = fx.mgr.query_pe(&fx.dir, "pdata").unwrap();
    assert_eq!(pdata["total"], 1);
    assert_eq!(pdata["pdata"][0]["start"], "0x1000");
    assert_eq!(pdata["pdata"][0]["end"], "0x1013");

    let exports = fx.mgr.query_pe(&fx.dir, "exports").unwrap();
    assert_eq!(exports["total"], 0);
    // imports 合并了延迟导入（B2）：主表空，唯一 DLL = FAKE_DELAY.dll
    let imports = fx.mgr.query_pe(&fx.dir, "imports").unwrap();
    assert_eq!(imports["total"], 1);
    assert_eq!(imports["imports"][0]["dll"], "FAKE_DELAY.dll");
    assert_eq!(imports["imports"][0]["funcs"][0], "delay_read");
}

#[test]
fn pe_tls_debug_rich_actions() {
    let fx = fixture("pe-b2");
    // TLS：回调数组 [0x1060]，未标注 → name null；已进函数集（TLS 种子）
    let tls = fx.mgr.query_pe(&fx.dir, "tls").unwrap();
    assert_eq!(tls["total"], 1);
    assert_eq!(tls["callbacks"][0]["rva"], "0x1060");
    assert_eq!(tls["callbacks"][0]["discovered"], true);
    assert!(tls["callbacks"][0]["name"].is_null());

    // debug：一条 CODEVIEW + RSDS PDB 路径（白送语义）
    let dbg = fx.mgr.query_pe(&fx.dir, "debug").unwrap();
    assert_eq!(dbg["total"], 1);
    assert_eq!(dbg["entries"][0]["type"], 2);
    assert_eq!(dbg["entries"][0]["rva"], "0x21C0");
    assert_eq!(dbg["pdb_path"], "C:\\fake\\query.pdb");

    // rich：XOR key + 工具链指纹一条（prod 1 / build 24 / count 1）
    let rich = fx.mgr.query_pe(&fx.dir, "rich").unwrap();
    assert_eq!(rich["key"], "0x11112222");
    assert_eq!(rich["total"], 1);
    assert_eq!(rich["prods"][0]["prod_id"], 1);
    assert_eq!(rich["prods"][0]["build"], 24);
    assert_eq!(rich["prods"][0]["count"], 1);

    let err = fx.mgr.query_pe(&fx.dir, "junk").unwrap_err();
    assert!(err.contains("非法"), "err={err}");
}

// ── locate ──────────────────────────────────────────────────────────────

#[test]
fn locate_four_states() {
    let fx = fixture("locate");

    let func = fx.mgr.query_locate(&fx.dir, 0x1000).unwrap();
    assert_eq!(func["status"], "func");
    assert_eq!(func["func"]["name"], "push_data");

    let body = fx.mgr.query_locate(&fx.dir, 0x1004).unwrap();
    assert_eq!(body["status"], "body");
    assert_eq!(body["func"]["rva"], "0x1000");

    let unowned = fx.mgr.query_locate(&fx.dir, 0x1015).unwrap();
    assert_eq!(unowned["status"], "unowned");
    assert_eq!(unowned["prev_func"]["rva"], "0x1000");
    assert_eq!(unowned["next_func"]["rva"], "0x1020");

    let outside = fx.mgr.query_locate(&fx.dir, 0x9000).unwrap();
    assert_eq!(outside["status"], "outside");
}

// ── functions / gaps ────────────────────────────────────────────────────

#[test]
fn functions_sort_filter_and_rows() {
    let fx = fixture("functions");
    let out = fx.mgr.query_functions(&fx.dir, "size", 10, None).unwrap();
    assert_eq!(out["total"], 4);
    let rows = out["functions"].as_array().unwrap();
    assert_eq!(rows[0]["rva"], "0x1000"); // 19B 最大
    assert_eq!(rows[1]["rva"], "0x1030"); // 9B
    assert_eq!(rows[2]["rva"], "0x1020"); // 8B
    assert_eq!(rows[3]["rva"], "0x1060"); // 6B（TLS/CFG 种子发现）
    assert_eq!(rows[0]["name"], "push_data");
    assert_eq!(rows[0]["crt_folded"], false);

    let filtered = fx
        .mgr
        .query_functions(&fx.dir, "size", 10, Some("push"))
        .unwrap();
    assert_eq!(filtered["total"], 1);

    let by_refs = fx.mgr.query_functions(&fx.dir, "refs", 10, None).unwrap();
    assert_eq!(by_refs["functions"][0]["refs"], 1); // func1/func2 各有一个调用方

    let err = fx
        .mgr
        .query_functions(&fx.dir, "junk", 10, None)
        .unwrap_err();
    assert!(err.contains("非法"), "err={err}");
}

#[test]
fn gaps_four_holes_in_text() {
    let fx = fixture("gaps");
    let out = fx.mgr.query_gaps(&fx.dir).unwrap();
    assert_eq!(out["total_gaps"], 4); // [0x1013,0x1020) [0x1028,0x1030) [0x1039,0x1060) [0x1066,0x1200)
    assert_eq!(out["total_gap_bytes"], 13 + 8 + 39 + 410);
    assert_eq!(out["gaps"][0]["start"], "0x1013");
    assert_eq!(out["gaps"][0]["end"], "0x1020");
}

// ── analyze 对账面（pdata/cfg vs discovered + 种子按源计数）────────────

#[test]
fn analyze_summary_pdata_reconciliation() {
    let fx = fixture("reconcile");
    let summary = fx.mgr.analyze(&fx.dir).unwrap();
    assert_eq!(summary["pdata_funcs"], 1); // .pdata 只登记 func0
    assert_eq!(summary["functions"], 4); // 递归下降三函数 + TLS/CFG 种子补 tls_func
    assert_eq!(summary["cfg_funcs"], 4); // CFG 表四条目全在 .text 界内 = discovered 对齐
    let by_src = &summary["seeds_by_source"];
    assert_eq!(by_src["entry"], 1);
    assert_eq!(by_src["pdata"], 1);
    assert_eq!(by_src["cfg"], 4);
    assert_eq!(by_src["tls"], 1);
    assert_eq!(by_src["nopsled"], 1);
    assert!(by_src.get("export").is_none()); // 无导出 → 来源诚实缺席
    // 再次查询走缓存（analyze 幂等）
    let out = fx.mgr.query_disasm(&fx.dir, 0x1000, 8).unwrap();
    assert_eq!(out["truncated"], true); // max_insns=8 < 11 条
    assert_eq!(out["count"], 8);
}

// ── annotate params 签名（B4：LLM 推断的参数落 draft.functions[].args）──

#[test]
fn annotate_params_signature_persisted() {
    let fx = fixture("annotate-params");
    // 带 params 标注 tls_func@0x1060（byte_pattern 锚指向真实字节）
    let out = fx
        .mgr
        .annotate(
            &fx.dir,
            &serde_json::json!({
                "rva": 0x1060,
                "semantic": "tls_callback",
                "anchors": [{"kind": "byte_pattern", "pattern": "B8 01 00 00 00 C3"}],
                "params": [
                    {"reg": "rcx", "ty": "u64", "desc": "handle"},
                    {"reg": "rdx", "ty": "char[32]"}
                ]
            }),
        )
        .unwrap();
    assert_eq!(out["params"], 2);
    assert_eq!(out["functions"], 2); // push_data + tls_callback
    // draft.json 落库验证：args 带 ty/desc
    let draft: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fx.dir
                .join("ziwei")
                .join("drafts")
                .join(PROFILE_ID)
                .join("draft.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let args = draft["functions"][1]["args"].as_array().unwrap();
    assert_eq!(args[0]["reg"], "rcx");
    assert_eq!(args[0]["ty"], "u64");
    assert_eq!(args[0]["desc"], "handle");
    assert_eq!(args[1]["reg"], "rdx");
    assert_eq!(args[1]["ty"], "char[32]");
    assert!(args[1]["desc"].is_null()); // 缺省 desc 不落值
    // 上限 16 loud 拒
    let mut cap_payload = serde_json::json!({
        "rva": 0x1030,
        "semantic": "cap_probe",
        "anchors": [{"kind": "imm_const", "value": 0xDEADBEEFu64}]
    });
    cap_payload["params"] = (0..17)
        .map(|_| serde_json::json!({"reg": "rcx"}))
        .collect::<Vec<_>>()
        .into();
    let err = fx.mgr.annotate(&fx.dir, &cap_payload).unwrap_err();
    assert!(err.contains("超上限 16"), "err={err}");
    // ty 非法 loud 拒（FieldType 字面量之外形态）
    let err = fx
        .mgr
        .annotate(
            &fx.dir,
            &serde_json::json!({
                "rva": 0x1030,
                "semantic": "bad_ty_probe",
                "anchors": [{"kind": "imm_const", "value": 0xDEADBEEFu64}],
                "params": [{"reg": "rcx", "ty": "int*"}]
            }),
        )
        .unwrap_err();
    assert!(err.contains("非法"), "err={err}");
}
