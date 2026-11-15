//! 子进程文本输出解码单一真相源（BUG-20261115-034）。
//!
//! exec 家车子进程（`cmd /C` / `bash -c` / 解释器直 spawn）的 stdout/stderr
//! 字节流编码不受我方控制：cmd 内建命令（dir/type/echo）在**无控制台管道
//! 场景**按系统 OEM 码页输出（中文 Windows = CP936/GBK），而 UTF-8 原生
//! 工具（cargo/rustc/git/node）输出严格合法 UTF-8。旧实现一律
//! `String::from_utf8_lossy`——GBK 字节被打成 U+FFFD 乱码，模型看到的
//! 中文文件名与命令输出全损坏。
//!
//! 策略：**严格 UTF-8 优先**（合法 → 字节精确原样返回，既有 UTF-8 工具
//! 行为零变化）；失败回落系统 OEM 码页解码（CJK 码页族 936/932/949/950
//! 经 encoding_rs）；其余码页诚实回落 lossy。
//!
//! 为什么不做源侧修复：`chcp 65001` 前缀依赖 `SetConsoleOutputCP`，需要
//! 子进程附着控制台——gateway 以 Hidden 窗运行（无控制台）时实测无效
//! （dir 输出原样 `?`/乱码）；CP437 类机器上字符本就在 cmd 输出层被毁成
//! `?`，解码侧对这类输出同样诚实回落 lossy（字节面无信息可恢复）。
//!
//! 消费方：agent loop exec / shell 工具 / guest exec / 安全中间件 exec
//! 的 stdout/stderr 解码点（同 BUG 横向接线）。

/// CJK OEM 码页 → encoding_rs 标签。只收东亚四码页（本项目主要用户群）；
/// 其余码页无可靠族映射，诚实回落 lossy。
fn codepage_label(cp: u16) -> Option<&'static str> {
    match cp {
        936 => Some("gbk"),
        932 => Some("shift_jis"),
        949 => Some("euc-kr"),
        950 => Some("big5"),
        _ => None,
    }
}

/// 纯函数解码（可测）：显式码页。UTF-8 合法 → 字节精确原样；否则按码页
/// 族解码（ASCII 在 CJK 码页内透明）；无对应解码器 → lossy（现状语义）。
pub fn decode_with_codepage(bytes: &[u8], codepage: u16) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    match codepage_label(codepage).and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())) {
        Some(enc) => enc.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 机器面解码（exec 解码点统一入口）：Windows 取系统 OEM 码页——无控制台
/// 管道场景 cmd 内建命令的实际输出口径；非 Windows 子进程生态约定 UTF-8
/// → 维持 lossy（既有行为）。
pub fn decode_child_output(bytes: &[u8]) -> String {
    #[cfg(windows)]
    {
        // GetOEMCP 无失败路径（恒返回已安装 OEM 码页 id），FFI 无副作用。
        let cp = unsafe { windows_sys::Win32::Globalization::GetOEMCP() };
        decode_with_codepage(bytes, cp as u16)
    }
    #[cfg(not(windows))]
    {
        let _ = codepage_label; // 纯函数仍被测试消费，非 Windows 侧不告警
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests;
