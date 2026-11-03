//! launch_and_inject shellcode 镜像装配测试（BUG-20261103-342）。

use super::*;

const DUMMY_BASE: u64 = 0x7F_0000_0000;
const DUMMY_LOADLIB: u64 = 0x1234_5678_9ABC_DEF0;
const DUMMY_GPA: u64 = 0x0FED_CBA9_8765_4321;

/// 255 字节纯 ASCII 路径（数据窗容量上限 = OFF_DATA_NAME - OFF_DATA_DLLPATH - 1）。
fn path_of_len(n: usize) -> String {
    let prefix = r"C:\".len();
    assert!(n >= prefix + 4, "必须留得出 .dll 后缀");
    let mut s = String::from("C:\\");
    s.push_str(&"a".repeat(n - prefix - 4));
    s.push_str(".dll");
    debug_assert_eq!(s.len(), n);
    s
}

/// 合法上限路径：布局逐项钉死（立即数补丁 / NUL / "Run\0" 名字区完好）。
#[test]
fn shellcode_image_happy_path_layout() {
    let p = path_of_len(255);
    let sc = build_shellcode_image(&p, DUMMY_BASE, DUMMY_LOADLIB, DUMMY_GPA)
        .expect("255 字节路径必须装得进数据窗");
    // 指令字节（除四个 imm64 补丁槽外）必须与模板逐字节一致——补丁槽的
    // 期望值由下方逐项断言钉死，这里只锁模板其余指令区不被污染。
    let mut got = sc[..SHCODE_SIZE].to_vec();
    let mut want = SHELLCODE.to_vec();
    for off in [OFF_DLLPATH, OFF_LOADLIB, OFF_NAME, OFF_GPA] {
        got[off..off + 8].fill(0);
        want[off..off + 8].fill(0);
    }
    assert_eq!(got, want, "非立即数区必须与模板逐字节一致");
    let mut q = [0u8; 8];
    q.copy_from_slice(&sc[OFF_DLLPATH..OFF_DLLPATH + 8]);
    assert_eq!(u64::from_le_bytes(q), DUMMY_BASE + OFF_DATA_DLLPATH as u64);
    q.copy_from_slice(&sc[OFF_LOADLIB..OFF_LOADLIB + 8]);
    assert_eq!(u64::from_le_bytes(q), DUMMY_LOADLIB);
    q.copy_from_slice(&sc[OFF_NAME..OFF_NAME + 8]);
    assert_eq!(u64::from_le_bytes(q), DUMMY_BASE + OFF_DATA_NAME as u64);
    q.copy_from_slice(&sc[OFF_GPA..OFF_GPA + 8]);
    assert_eq!(u64::from_le_bytes(q), DUMMY_GPA);
    // 路径字节原样落窗 + NUL 收尾
    assert_eq!(&sc[OFF_DATA_DLLPATH..OFF_DATA_DLLPATH + 255], p.as_bytes());
    assert_eq!(sc[OFF_DATA_DLLPATH + 255], 0);
    // "Run\0" 名字区完好（且不被路径数据侵入）
    assert_eq!(&sc[OFF_DATA_NAME..OFF_DATA_NAME + 4], b"Run\0");
}

/// 边界：路径字节数必须 < 名字区起点（≤255 + NUL）。旧实现不校验——
/// 256 字节路径的尾字节/NUL 覆写 OFF_DATA_NAME 的 "Run\0" → 目标内
/// GetProcAddress 失败 → shellcode `jmp self` 死循环（宿主 100% CPU
/// 永久空转），无任何诚实报错。
#[test]
fn shellcode_image_rejects_overlong_path() {
    let err = build_shellcode_image(&path_of_len(256), DUMMY_BASE, DUMMY_LOADLIB, DUMMY_GPA)
        .expect_err("256 字节路径必须诚实拒绝（旧实现静默覆写名字区）");
    assert!(
        matches!(err, InjectError::DllPathTooLong(256)),
        "诚实报超长: {err}"
    );
}

/// LoadLibraryA 按 ANSI 代码页解读路径——非 ASCII 的 UTF-8 字节序列在
/// CJK 代码页下变成别的字符 → 目标内加载必失败 → 同型 `jmp self` 死
/// 循环。装配前必须诚实拒绝。
#[test]
fn shellcode_image_rejects_non_ascii_path() {
    let p = format!("C:\\临时目录\\{}.dll", "a".repeat(100));
    let err = build_shellcode_image(&p, DUMMY_BASE, DUMMY_LOADLIB, DUMMY_GPA)
        .expect_err("非 ASCII 路径必须诚实拒绝（LoadLibraryA 语义）");
    assert!(
        matches!(err, InjectError::DllPathNotAscii),
        "诚实报非 ASCII: {err}"
    );
}
