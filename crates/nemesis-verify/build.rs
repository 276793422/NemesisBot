//! 构建脚本：只声明根锚 env 追踪。
//!
//! 根锚值由 lib.rs 的 `option_env!("NEMESIS_BUILD_ROOT_ANCHOR")` 在编译期
//! 读取；没有本声明时，env 出现/变化**不会**触发本 crate 重编——本地先编出
//! 无锚二进制后再带 env 构建 = cargo 复用旧产物，锚静默缺失（2026-10-23
//! 自更新 E2E 实证，当时只能 `cargo clean -p nemesis-verify` 兜底）。声明后：
//! env 出现/变化/消失都会自动重编本 crate，构建脚本注入锚无需任何 clean。
fn main() {
    println!("cargo:rerun-if-env-changed=NEMESIS_BUILD_ROOT_ANCHOR");
}
