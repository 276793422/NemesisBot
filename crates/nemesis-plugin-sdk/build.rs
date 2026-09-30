//! 构建脚本：声明 WIT 合同依赖，使 cargo 增量编译感知合同变更。
//!
//! SDK 无本地 wit 副本（`host.rs` 的 bindgen 直接引用宿主 crate 权威目录
//! `nemesis-plugins-wasm/wit/`）；该文件不在本 crate 源树内，cargo 指纹
//! 默认不追踪——显式 rerun-if-changed 后，合同变更自动触发 SDK 重编。

fn main() {
    println!(
        "cargo:rerun-if-changed={}",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../nemesis-plugins-wasm/wit/plugin.wit"
        )
    );
}
