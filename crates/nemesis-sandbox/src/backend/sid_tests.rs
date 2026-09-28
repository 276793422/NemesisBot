//! `sid.rs`（DACL 定向档 D1）单测。
//!
//! 全平台跑纯函数面；Windows 专属的 canonicalize 大小写收敛与
//! `ConvertStringSidToSidW` 解析往返逐个挂 `#[cfg(windows)]`（仓库 Windows
//! 测试标记约定——Linux 上编译期消失）。

use super::sid::{derive_workspace_sid, normalize_canonical, sid_from_digest};

/// 同一路径反复派生 = 同一 SID（确定性是 standing ACE 跨会话复用的根基）。
#[test]
fn derive_is_deterministic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = derive_workspace_sid(tmp.path()).expect("derive a");
    let b = derive_workspace_sid(tmp.path()).expect("derive b");
    assert_eq!(a, b, "同一路径两次派生必须一致: {a} vs {b}");
}

/// 形态契约：`S-1-5-21-` 前缀 + 恰好 6 段、子授权无符号 u32 十进制。
#[test]
fn sid_shape_is_domain_form() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sid = derive_workspace_sid(tmp.path()).expect("derive");
    let parts: Vec<&str> = sid.split('-').collect();
    assert_eq!(&parts[0..4], &["S", "1", "5", "21"], "前四节形态: {sid}");
    assert_eq!(parts.len(), 7, "S-1-5-21-a-b-c 共 7 段: {sid}");
    for p in &parts[4..] {
        p.parse::<u32>()
            .unwrap_or_else(|_| panic!("子授权 {p} 应为无符号 u32 十进制: {sid}"));
    }
}

/// 不同路径派生不同 SID（碰撞面天文小概率——这是设计假设的实证抽查）。
#[test]
fn different_paths_derive_different_sids() {
    let t1 = tempfile::tempdir().expect("tempdir 1");
    let t2 = tempfile::tempdir().expect("tempdir 2");
    let a = derive_workspace_sid(t1.path()).expect("derive 1");
    let b = derive_workspace_sid(t2.path()).expect("derive 2");
    assert_ne!(a, b, "不同工作区不应撞 SID");
}

/// 尾斜杠收敛：`path` 与 `path/`（或 `path\`）是同一目录 → 同一 SID。
/// canonicalize 抹掉尾分隔符——派生输入不区分写法。
#[test]
fn trailing_slash_converges() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = derive_workspace_sid(tmp.path()).expect("derive base");
    let sep = std::path::MAIN_SEPARATOR.to_string();
    let with_slash = tmp.path().as_os_str().to_string_lossy().to_string() + &sep;
    let slashed = derive_workspace_sid(std::path::Path::new(&with_slash))
        .expect("derive with trailing slash");
    assert_eq!(base, slashed, "尾斜杠写法应收敛到同一 SID");
}

/// normalize_canonical 纯函数：verbatim 前缀剥离 + lowercase 的字节级约定
/// （D1 派生规则的 hash 输入形态）。
#[test]
fn normalize_strips_verbatim_and_lowercases() {
    assert_eq!(
        normalize_canonical(std::path::Path::new(r"\\?\C:\Work\Ws")),
        r"c:\work\ws",
        "本地盘 verbatim 剥离"
    );
    assert_eq!(
        normalize_canonical(std::path::Path::new(r"\\?\UNC\server\share\ws")),
        r"\\server\share\ws",
        "UNC verbatim 还原为 \\\\ 前缀"
    );
    assert_eq!(
        normalize_canonical(std::path::Path::new("/tmp/WS")),
        "/tmp/ws",
        "非 verbatim 输入原样 lowercase（Unix 形态）"
    );
}

/// digest → SID 的字节序契约（be32 大端拆 3 个子授权）——用全 0/全 0xFF
/// 两个极端向量钉死，防将来误改 little-endian。
#[test]
fn digest_be32_byte_order() {
    let mut d = [0u8; 32];
    assert_eq!(sid_from_digest(&d), "S-1-5-21-0-0-0", "全 0 摘要");
    d[0] = 0xFF; // be32(d[0..4]) = 0xFF000000
    assert_eq!(
        sid_from_digest(&d),
        "S-1-5-21-4278190080-0-0",
        "大端序高字节在前"
    );
}

// ---------------------------------------------------------------------------
// Windows 专属：canonicalize 收敛 + Win32 SID 解析往返
// ---------------------------------------------------------------------------

/// Windows 大小写不敏感：`C:\WS` 与 `c:\ws` canonicalize 后收敛 → 同一 SID
/// （设计文档 §3.1「lowercase 必须」的实证钉）。
#[cfg(windows)]
#[test]
fn case_insensitive_paths_converge_on_windows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let orig = derive_workspace_sid(tmp.path()).expect("derive orig");
    let upper = derive_workspace_sid(std::path::Path::new(
        &tmp.path().to_string_lossy().to_ascii_uppercase(),
    ))
    .expect("derive upper");
    let lower = derive_workspace_sid(std::path::Path::new(
        &tmp.path().to_string_lossy().to_ascii_lowercase(),
    ))
    .expect("derive lower");
    assert_eq!(orig, upper, "大写变体应收敛");
    assert_eq!(orig, lower, "小写变体应收敛");
}

/// Windows 上 canonicalize 产出 verbatim 路径——生产输入的真实形态，派生
/// 链路（含剥前缀）端到端自洽（幂等性在 verbatim 形态下成立）。
#[cfg(windows)]
#[test]
fn canonicalize_produces_verbatim_and_derive_handles_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let canon = tmp.path().canonicalize().expect("canonicalize");
    let s = canon.to_string_lossy();
    assert!(
        s.starts_with(r"\\?\"),
        "Windows canonicalize 应产出 verbatim 形态，实际 {s}"
    );
    let a = derive_workspace_sid(canon.as_path()).expect("derive from verbatim path");
    let b = derive_workspace_sid(tmp.path()).expect("derive from plain path");
    assert_eq!(a, b, "verbatim 与普通形态输入应派生同一 SID");
}

/// 解析往返：派生出的 SID 字符串必须被 `ConvertStringSidToSidW` 接受（D2
/// 打 ACE / D3 组 restricting SIDs 都走这一步），且结构为 revision=1、
/// identifier authority=5、4 个 subauthority、首节 = 21（`S-1-5-21` 形态）。
#[cfg(windows)]
#[test]
fn derived_sid_parses_via_convert_string_sid_to_sidw() {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::{
        GetSidIdentifierAuthority, GetSidSubAuthority, GetSidSubAuthorityCount, PSID,
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let sid_str = derive_workspace_sid(tmp.path()).expect("derive");
    let wide: Vec<u16> = sid_str.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut sid: PSID = std::ptr::null_mut();
        if ConvertStringSidToSidW(wide.as_ptr(), &mut sid) == 0 {
            panic!(
                "ConvertStringSidToSidW 拒绝派生 SID {sid_str}: {}",
                GetLastError()
            );
        }
        // 结构断言（ LocalFree 前完成）。
        let count = *GetSidSubAuthorityCount(sid) as usize;
        assert_eq!(count, 4, "S-1-5-21-a-b-c 应有 4 个 subauthority: {sid_str}");
        assert_eq!(
            *GetSidSubAuthority(sid, 0),
            21,
            "第一节 = 21（域形态）: {sid_str}"
        );
        let auth = GetSidIdentifierAuthority(sid);
        assert_eq!(
            (*auth).Value,
            [0, 0, 0, 0, 0, 5],
            "identifier authority = 5"
        );
        // 子授权 a/b/c 与字符串逐节一致（GetSidSubAuthority 从 21 起数，
        // 字符串枚举跳过 S/1/5/21 后从 a 开始 → 下标错位 +1）。
        for (i, part) in sid_str.split('-').skip(4).enumerate() {
            let expect: u32 = part.parse().expect("子授权解析");
            assert_eq!(
                *GetSidSubAuthority(sid, (i + 1) as u32),
                expect,
                "子授权第 {} 节不一致",
                i + 1
            );
        }
        LocalFree(sid as _);
    }
}

/// 不存在路径 → Err（canonicalize 失败）——fail-closed，不静默产出假 SID。
#[test]
fn missing_path_is_err() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("no_such_subdir");
    assert!(
        derive_workspace_sid(&missing).is_err(),
        "不存在路径应 Err: {:?}",
        derive_workspace_sid(&missing)
    );
}
