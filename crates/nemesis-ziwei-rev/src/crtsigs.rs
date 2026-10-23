//! crtsigs —— MSVC CRT 折叠（T3.3）。
//!
//! 「疑似库函数」判定器：命中即从候选集折叠（排除出标注建议）。
//! 两路判定：
//! 1. **导入模块启发**（/MD 动态 CRT 的主通路）：函数只引用 CRT 模块
//!    导入（VCRUNTIME140 / api-ms-win-crt-* / uefoundation 等）→ 折叠；
//! 2. **字节签名库**（/MT 静态链接兜底）：常见 CRT 序言/安全 cookie
//!    特征。初版覆盖有限——折叠只是**建议**，漏折叠只产生候选噪声，
//!    不影响正确性（诚实边界）。
//!
//! 签名格式：`(name, [(byte, mask)])`，mask=false = 通配字节。数据
//! 内嵌 const（无运行时文件依赖；格式可扩展）。

use serde::Serialize;

use crate::funcs::FunctionSet;
use crate::pe::ImageMap;

/// 疑似库函数（折叠建议）。
#[derive(Debug, Clone, Serialize)]
pub struct CrtSuspect {
    pub rva: u32,
    /// 判定依据（module:<dll> / sig:<name>）。
    pub reason: String,
}

/// CRT 导入模块前缀/全名表（小写比较）。
const CRT_MODULES: &[&str] = &[
    "vcruntime140",
    "api-ms-win-crt",
    "ucrtbase",
    "msvcr",
    "msvcp",
    "ntdllmtdll",
    "concrt140",
];

/// 常见 CRT 字节签名（/MT 兜底；x64 序言样本，初版覆盖——诚实边界见模块头）。
/// (函数角色名, 字节模式, None=通配)。
const CRT_SIGNATURES: &[(&str, &[Option<u8>])] = &[
    // __security_init_cookie 典型序言：mov [rsp+X], reg; sub rsp,..; 常量 2C06... 不签
    // 收敛为几个高置信模式：
    // 1) memcpy 常见矢量序言（movdqu/ymm 载入 + jmp 尾）特征过宽，不签；
    // 2) _initterm 双参循环签名：test rcx/rcx + jz + test rdx/rdx + jz 序
    (
        "_initterm_like",
        &[
            Some(0x48),
            Some(0x85),
            Some(0xC9), // test rcx, rcx
            Some(0x74),
            None, // jz ...
            Some(0x48),
            Some(0x85),
            Some(0xD2), // test rdx, rdx
            Some(0x74),
            None, // jz ...
        ],
    ),
    // 2) __security_check_cookie 经典：cmp rcx, [__cookie]; jne fail; ret
    (
        "security_check_like",
        &[
            Some(0x48),
            Some(0x3B),
            Some(0x0D),
            None,
            None,
            None,
            None,
            None, // cmp rcx, [rip+disp32]
            Some(0x75),
            None,       // jne ...
            Some(0xC3), // ret
        ],
    ),
    // 3) 帧安全序言：mov [rsp+8], rbx?…过宽不签——保持小而准。
];

fn match_sig(bytes: &[u8], sig: &[Option<u8>]) -> bool {
    if bytes.len() < sig.len() {
        return false;
    }
    sig.iter().zip(bytes.iter()).all(|(s, b)| match s {
        Some(v) => v == b,
        None => true,
    })
}

/// 判定折叠集。
pub fn fold(
    image: &ImageMap,
    funcs: &FunctionSet,
    imports: &[crate::pe::ImportDll],
) -> Vec<CrtSuspect> {
    let mut out = Vec::new();
    // IAT 槽 → dll（小写）
    let mut iat_dll: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for d in imports {
        let lower = d.dll.to_lowercase();
        for (_, slot) in &d.funcs {
            iat_dll.insert(*slot, lower.clone());
        }
    }
    for f in &funcs.functions {
        // 路径 1：函数的 rip-rel 引用全部落在 CRT 模块 IAT → 折叠
        let mut crt_refs = 0usize;
        let mut non_crt_refs = 0usize;
        for insn in &f.insns {
            if let Some(t) = insn.rip_rel_target {
                if let Some(dll) = iat_dll.get(&t)
                    && CRT_MODULES.iter().any(|m| dll.contains(m))
                {
                    crt_refs += 1;
                    continue;
                }
                non_crt_refs += 1;
            }
        }
        if crt_refs > 0 && non_crt_refs == 0 {
            out.push(CrtSuspect {
                rva: f.rva,
                reason: format!("module:仅 CRT 导入引用（{crt_refs} 处）"),
            });
            continue;
        }
        // 路径 2：字节签名（函数体前 64 字节）
        let head_len = (f.end_rva - f.rva).min(64) as usize;
        if let Some(body) = image.read_rva(f.rva, head_len) {
            for (name, sig) in CRT_SIGNATURES {
                if match_sig(body, sig) {
                    out.push(CrtSuspect {
                        rva: f.rva,
                        reason: format!("sig:{name}"),
                    });
                    break;
                }
            }
        }
    }
    out
}
