//! 构建脚本：声明 WIT 合同依赖，使 cargo 增量编译感知合同变更。
//!
//! `wasmtime::component::bindgen!` 在宏展开期读取 `wit/plugin.wit`，但该
//! 文件不是 `.rs` 源——cargo 指纹默认不追踪，改合同不重编就会用过期绑定。
//! 显式 rerun-if-changed 后，合同任何变更（含版本 bump）自动触发重编。

fn main() {
    println!(
        "cargo:rerun-if-changed={}",
        concat!(env!("CARGO_MANIFEST_DIR"), "/wit/plugin.wit")
    );
}
