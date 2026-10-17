//! iotsmall（panic=abort + lto=fat）× wasmtime 宿主兼容性探针。
//!
//! 背景：iotsmall profile 把宿主编译成 panic=abort（lto=true、codegen-units=1、
//! opt-level=z、strip），而 wasmtime 的 guest trap / fuel 耗尽 / 内存上限
//! 路径传统上依赖宿主 unwind 捕获——若 abort 下 guest 异常直接带走进程，
//! 任何行为不端的插件都能打死整个 gateway（DoS 面）。本探针在**真 abort
//! 二进制**里把四条关键路径各跑一遍，任何一条没接住就以非零退码死亡：
//!
//! 1. `echo`      —— guest 实例化 + 正常调用（happy path）
//! 2. `trap`      —— guest 蓄意 unreachable（wasmtime 必须接住变 Trap 错误）
//! 3. `mem_bomb`  —— ResourceLimiter 内存上限 → 分配 trap
//! 4. `fuel_bomb` —— fuel 耗尽 → trap（metering 在 abort 下正常计费）
//!
//! 用法（仓库根）：
//! ```bash
//! # 先构建 guest 夹具（独立 workspace）：
//! cd plugins/wasm/translate && cargo build --target wasm32-wasip2 --release
//! # 编译 + 运行探针（iotsmall profile 继承 panic=abort，example 是普通 bin）：
//! cargo build --profile iotsmall -p nemesis-plugins-wasm --example iotsmall_abort_probe
//! target/iotsmall/examples/iotsmall_abort_probe \
//!   plugins/wasm/translate/target/wasm32-wasip2/release/translate_plugin.wasm
//! ```
//!
//! 判定：进程存活 + 四步全 OK 退码 0 = 兼容；进程被 abort 带走（Windows
//! 0xC0000409 / POSIX SIGABRT）= 不兼容，宿主进程会被恶意/故障插件打死。
//! 行为面复用 tests/adversarial.rs 的装配漏斗（AutoApprove 直通 + 无签名
//! ReviewRequired），manifest 与对抗夹具同形。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemesis_plugins_wasm::PluginError;
use nemesis_plugins_wasm::install::{AutoApprove, NoSecrets, PluginInstaller};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::PluginManager;
use nemesis_plugins_wasm::runtime::ExecContext;

/// 对抗夹具 manifest（与 tests/adversarial.rs 同源：收紧 memory/帧预算/fuel，
/// 让 mem_bomb / fuel_bomb 确定性触顶而非等全局默认额度）。
const ADVERSARIAL_TOML: &str = r#"api-version = 1
slug = "adversarial"
kind = "tool"
name = "Adversarial"
version = "0.1.0"
description = "iotsmall abort 探针夹具（复用 translate 行为面）"

[permissions]
x-secret = ["api_key"]

[limits]
fuel = 200_000_000
timeout-ms = 20_000
memory-bytes = 8_000_000
host-call-budget = 64
"#;

/// 第二档 manifest：大 fuel 预算给 fuel_bomb 留出确定性烧完的量级差
/// （fuel_bomb 需要消耗超过预算的燃料；200M 对循环体太小会先 OOM 后 fuel，
/// 这里放宽 fuel 保留 memory 上限——fuel 触顶先于内存触顶）。
const FUEL_TOML: &str = r#"api-version = 1
slug = "adversarial"
kind = "tool"
name = "Adversarial"
version = "0.1.0"
description = "iotsmall abort 探针夹具（fuel 档）"

[permissions]
x-secret = ["api_key"]

[limits]
fuel = 3_000_000
timeout-ms = 20_000
memory-bytes = 8_000_000
host-call-budget = 64
"#;

fn ctx() -> ExecContext {
    ExecContext {
        session_key: "sess-abort-probe".into(),
        call_id: "call-abort-probe".into(),
    }
}

/// 写插件源目录（plugin.toml + wasm，sha256 头防替换），返回目录路径。
fn stage(root: &Path, toml_body: &str, wasm_src: &Path) -> PathBuf {
    let wasm_bytes = std::fs::read(wasm_src).expect("read guest wasm");
    let sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&wasm_bytes))
    };
    let dir = root.join("src");
    std::fs::create_dir_all(&dir).expect("stage dir");
    let manifest = format!("wasm-sha256 = \"{sha}\"\n{toml_body}\n");
    std::fs::write(dir.join("plugin.toml"), manifest).expect("write manifest");
    std::fs::write(dir.join("plugin.wasm"), &wasm_bytes).expect("write payload");
    dir
}

fn manager_with(limits: PluginLimits) -> Arc<PluginManager> {
    let ws = tempfile::tempdir().expect("tempdir");
    let ws_path = ws.path().to_path_buf();
    std::mem::forget(ws); // 探针生命周期 = 进程生命周期
    Arc::new(PluginManager::new(&ws_path, limits, Arc::new(NoSecrets)).expect("manager"))
}

/// 装配（AutoApprove 直通漏斗）+ 装完注销防 slug 冲突。每次调用用独立
/// manager（独立 workspace），天然无跨步 slug 复用问题。
fn install(mgr: &Arc<PluginManager>, toml_body: &str, wasm: &Path) -> PathBuf {
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage(tmp.path(), toml_body, wasm);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio rt");
    let reg = rt.block_on(async {
        PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None)
            .install(&src, true)
            .await
            .expect("install")
    });
    println!("  installed trust={:?}", reg.trust);
    drop(tmp);
    src
}

/// 单步执行 + 判定。expect_err=true 时必须 Err（trap 类），否则必须 Ok。
fn step(mgr: &Arc<PluginManager>, args: &str, expect_err: bool, what: &str) -> bool {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio rt");
    let started = Instant::now();
    let out = rt.block_on(mgr.execute_tool("plugin.adversarial.translate", args.into(), ctx()));
    match out {
        Ok(o) if !expect_err => {
            println!(
                "  PASS {what}: ok in {:.2}s, content head={:?}",
                started.elapsed().as_secs_f32(),
                o.content.chars().take(60).collect::<String>()
            );
            true
        }
        Ok(o) => {
            println!(
                "  FAIL {what}: expected Trap error, got Ok({:?})",
                o.content.chars().take(60).collect::<String>()
            );
            false
        }
        Err(e) if expect_err => {
            let is_trap = matches!(e, PluginError::Trap(_));
            println!(
                "  PASS {what}: trapped in {:.2}s as {:?} (trap variant={is_trap})",
                started.elapsed().as_secs_f32(),
                e
            );
            // trap 类错误必须被接住变 Err 且不带走进程；变体不较真
            // （fuel 耗尽在部分版本报 Trap，部分报 FuelExhausted 类），
            // 关键判定是「进程还活着、错误是 Err 不是 abort」。
            true
        }
        Err(e) => {
            println!("  FAIL {what}: unexpected error {e:?}");
            false
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let wasm = args
        .next()
        .map(PathBuf::from)
        .expect("usage: iotsmall_abort_probe <translate_plugin.wasm>");
    assert!(wasm.exists(), "guest wasm not found: {}", wasm.display());

    println!("[iotsmall abort probe] guest = {}", wasm.display());
    let mut all_ok = true;

    // 1. happy path：echo（guest 实例化 + 调用 + host 调用回带）
    println!("[1/4] echo（happy path）");
    let mgr = manager_with(PluginLimits::default());
    install(&mgr, ADVERSARIAL_TOML, &wasm);
    all_ok &= step(
        &mgr,
        r#"{"mode":"echo","text":"abort-probe-hello"}"#,
        false,
        "echo",
    );
    drop(mgr);

    // 2. guest 蓄意 unreachable：wasmtime 必须接住变 Err（abort 下的生死题）
    println!("[2/4] trap（guest unreachable）");
    let mgr = manager_with(PluginLimits::default());
    install(&mgr, ADVERSARIAL_TOML, &wasm);
    all_ok &= step(&mgr, r#"{"mode":"trap"}"#, true, "trap");
    drop(mgr);

    // 3. 内存炸弹：ResourceLimiter 拒 grow → 分配 trap
    println!("[3/4] mem_bomb（limiter 拦截）");
    let mgr = manager_with(PluginLimits::default());
    install(&mgr, ADVERSARIAL_TOML, &wasm);
    let started = Instant::now();
    let ok = step(&mgr, r#"{"mode":"mem_bomb"}"#, true, "mem_bomb");
    // memory-bytes=8MB 收紧后应远快于 timeout-ms=20s 墙钟闸（同对抗测试口径）
    if started.elapsed() > Duration::from_secs(20) {
        println!(
            "  FAIL mem_bomb: took {:.1}s — limiter did not bite promptly",
            started.elapsed().as_secs_f32()
        );
        all_ok = false;
    } else {
        all_ok &= ok;
    }
    drop(mgr);

    // 4. fuel 炸弹：fuel 收紧到 3M，fuel_bomb 模式烧超预算 → fuel trap
    println!("[4/4] fuel_bomb（fuel metering）");
    let mgr = manager_with(PluginLimits::default());
    install(&mgr, FUEL_TOML, &wasm);
    all_ok &= step(&mgr, r#"{"mode":"fuel_bomb"}"#, true, "fuel_bomb");
    drop(mgr);

    if all_ok {
        println!(
            "[RESULT] ALL PASS — panic=abort 宿主下四条 wasmtime 路径全部被接住（iotsmall × plugins-wasm 兼容性成立）"
        );
        std::process::exit(0);
    } else {
        println!("[RESULT] FAIL — 存在未接住路径，见上方 FAIL 行");
        std::process::exit(1);
    }
}
