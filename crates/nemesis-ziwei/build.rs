//! build.rs —— payload DLL 内嵌（T1.6）。
//!
//! 找 `plugins/ziwei-agent-dll/bin/ziwei_agent_{x64,x86}.dll`：
//! - 在场 → 生成 `embed.rs`：include_bytes! 内嵌 + sha256/大小常量（编译期快照）；
//! - 缺席 → 生成空内嵌 + `PAYLOAD_EMBEDDED=false`（运行期诚实报错，不破构建）。
//!
//! 版本戳纪律：embed 常量里的 sha256 是**编译期**快照；运行期 first-use 落盘
//! 时按它决定复用/重写（payload_file::ensure_payload）。

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../../plugins/ziwei-agent-dll/bin/ziwei_agent_x64.dll");
    println!("cargo:rerun-if-changed=../../plugins/ziwei-agent-dll/bin/ziwei_agent_x86.dll");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bin = root.join("../../plugins/ziwei-agent-dll/bin");

    let mut code = String::new();
    for arch in ["x64", "x86"] {
        let path = bin.join(format!("ziwei_agent_{arch}.dll"));
        match fs::read(&path) {
            Ok(bytes) => {
                let hash = hex_sha256(&bytes);
                code.push_str(&format!(
                    "/// 内嵌 payload（{arch}，编译期快照 {hash}）。\n\
                     pub const PAYLOAD_{ARCH}_BYTES: &[u8] = include_bytes!(r\"{path}\");\n\
                     pub const PAYLOAD_{ARCH}_SHA256: &str = \"{hash}\";\n\
                     pub const PAYLOAD_{ARCH}_SIZE: usize = {size};\n",
                    ARCH = arch.to_uppercase(),
                    path = path.display(),
                    hash = hash,
                    size = bytes.len(),
                ));
            }
            Err(_) => {
                code.push_str(&format!(
                    "/// payload（{arch}）未内嵌——仓库内缺席（先构建 plugins/ziwei-agent-dll）。\n\
                     pub const PAYLOAD_{ARCH}_BYTES: &[u8] = &[];\n\
                     pub const PAYLOAD_{ARCH}_SHA256: &str = \"\";\n\
                     pub const PAYLOAD_{ARCH}_SIZE: usize = 0;\n",
                    ARCH = arch.to_uppercase(),
                ));
            }
        }
    }
    let x64_present = !code.contains("PAYLOAD_X64_BYTES: &[u8] = &[]");
    let x86_present = !code.contains("PAYLOAD_X86_BYTES: &[u8] = &[]");
    // 生成面直写终值（true && true 会吃 clippy eq_op deny）
    let embedded = x64_present && x86_present;
    code.push_str(&format!(
        "/// 两架构 payload 是否都已内嵌。\n\
         pub const PAYLOAD_EMBEDDED: bool = {embedded};\n"
    ));
    fs::write(out.join("embed.rs"), code).unwrap();
}

fn hex_sha256(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
