//! task-eval 构建脚本。
//!
//! BUG-20261104-369：task-eval 本体构建戳（unix 秒）——基线 / 对比门
//! 锚定判定语义版本。判定逻辑（TASKS 表 + 检查器）编译进二进制，陈旧
//! 二进制跑对比门会产生假回退/假通过；与 agent-bench 同源语义
//!（2026-11-04 agent-bench 假回退实录后的横向同修）。

fn main() {
    let build_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=TASK_EVAL_BUILD_UNIX={build_unix}");
}
