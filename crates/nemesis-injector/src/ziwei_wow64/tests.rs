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
    // RVA 落在 32 位空间（< 4GB）——WOW64 kernel32 是 PE32
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
