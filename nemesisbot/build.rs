//! Build script for nemesisbot binary.
//!
//! Injects version information via environment variables that are read at compile time:
//! - `NEMESISBOT_GIT_COMMIT`: Short git commit hash
//! - `NEMESISBOT_BUILD_TIME`: RFC 3339 build timestamp
//! - `NEMESISBOT_RUSTC_VERSION`: Rust compiler version string

use std::path::Path;
use std::process::Command;

fn main() {
    // BUILD-001（2026-09-22 审查）：静态前端产物缺失时给出可读报错。
    // `embedded.rs` 的 `include_dir!` 编译期硬要求 crates/nemesis-web/static/
    //（gitignored 构建产物，干净 clone 必然缺失），proc-macro 的 panic 信息
    // 不指向根因——在这里先挡下并直接告知修复命令。CI 与一键构建脚本总是
    // 先执行 npm build，不受影响。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let static_dir = Path::new(&manifest_dir).join("../crates/nemesis-web/static");
    if !static_dir.is_dir() {
        println!(
            "cargo:error=nemesisbot 编译需要前端构建产物 crates/nemesis-web/static/（当前缺失）"
        );
        println!(
            "cargo:error=请先构建前端: npm --prefix web install && npm --prefix web run build"
        );
        println!("cargo:error=或使用一键脚本: scripts/build-windows.bat / scripts/build-linux.sh");
        std::process::exit(1);
    }

    // Git commit hash (short)
    let git_commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Build time (RFC 3339)
    let build_time = chrono::Local::now().to_rfc3339();

    // Rustc version
    let rust_version = Command::new("rustc")
        .args(["--version"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Set env vars for compile-time reading
    println!("cargo:rustc-env=NEMESISBOT_GIT_COMMIT={}", git_commit);
    println!("cargo:rustc-env=NEMESISBOT_BUILD_TIME={}", build_time);
    println!("cargo:rustc-env=NEMESISBOT_RUSTC_VERSION={}", rust_version);

    // ------------------------------------------------------------------
    // 签名验证接入（verify_policy 锚注入，三级优先级）：
    //   1. env NEMESIS_BUILD_ROOT_ANCHOR（CI 的 Inject root anchor step 注入）
    //   2. 仓库 certs/root_cert.der 现算 SHA-256（本地开发者：证书入仓后
    //      无需手动设 env，锁定版本地构建直接可跑）
    //   3. 皆缺 → 不 emit（verify_policy::ROOT_ANCHOR = None，消费版降级
    //      off / 锁定版拒启）
    // rerun 指令防 stale：env 变化或根证书文件变化都会重跑本脚本重烧常量。
    // ------------------------------------------------------------------
    println!("cargo:rerun-if-env-changed=NEMESIS_BUILD_ROOT_ANCHOR");
    // rerun-if-changed 路径相对**包根**（nemesisbot/）而非仓库根——证书在
    // 仓库根 certs/ 下，必须带 ../ 前缀；否则密钥仪式落盘证书后不会触发
    // 重烧锚（无锚旧产物 stale 存活）。
    println!("cargo:rerun-if-changed=../certs/root_cert.der");
    let repo_root = Path::new(&manifest_dir).join("..");
    let anchor = match std::env::var("NEMESIS_BUILD_ROOT_ANCHOR") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => {
            let cert = repo_root.join("certs").join("root_cert.der");
            match std::fs::read(&cert) {
                Ok(bytes) => {
                    use sha2::{Digest, Sha256};
                    let fp: String = Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect();
                    Some(fp)
                }
                Err(_) => None,
            }
        }
    };
    match anchor {
        Some(a) => println!("cargo:rustc-env=NEMESIS_ROOT_ANCHOR={}", a),
        // 显式 emit 空值：压掉编译环境里可能存在的同名 ambient 变量，
        // 保证「无锚」裁决只由本脚本的三级优先级决定（option_env! 侧
        // 把空串当 None）。
        None => println!("cargo:rustc-env=NEMESIS_ROOT_ANCHOR="),
    }

    // Re-run build script if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");

    // ------------------------------------------------------------------
    // 构建形态清单（system.features WSAPI 数据源）：解析
    // scripts/customize/features.toml（feature 清单单一真相源，49 条），
    // 逐条以 CARGO_FEATURE_<ID 大写下划线> env（cargo 为本包每个启用的
    // feature 设置）判定**本构建的真实编译态**，写 OUT_DIR/features.json
    // 供 embedded.rs include_str! 嵌入。清单缺失（罕见： customize 目录
    // 被裁）= 空数组，不阻塞构建。
    // ------------------------------------------------------------------
    emit_features_manifest(&manifest_dir);

    // Embed icon on Windows
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("recourse/nemesisbot_multi.ico");
        res.compile().expect("failed to embed icon");

        // 主线程栈预留 16MB（Windows 默认 1MB，std 线程不受影响）。CLI 已有
        // 37 个命令模块，clap derive 树在 debug 构建的解析/帮助渲染递归对
        // 1MB 贴线（2026-09-29 实证：新增 plugin 命令后 debug 二进制连
        // `--help` 都栈溢出，二分定位=命令树膨胀非 wasmtime 链接；release
        // 帧小未触线）。这是结构性余量问题——任何新命令都可能复发，故在
        // 链接期一次性给足。16MB 仅是地址空间预留，commit 按需。
        let stack_arg = match std::env::var("CARGO_CFG_TARGET_ENV")
            .unwrap_or_default()
            .as_str()
        {
            "gnu" => "-Wl,--stack,16777216",
            _ => "/STACK:16777216", // msvc（默认工具链）
        };
        println!("cargo:rustc-link-arg-bins={stack_arg}");
    }
}

/// 解析 scripts/customize/features.toml 生成构建形态清单 JSON 到 OUT_DIR。
///
/// 输出形态：`[{"id","label","desc","category","default","enabled"},…]`
///（build-profile 特殊条目跳过——它选 profile 不是 feature）。`enabled`
/// 取 CARGO_FEATURE_* env（cargo 语义 = 本构建实际编译态，非 customize
/// 默认值）。写入失败（OUT_DIR 不可写等）panic——构建期环境问题应显式炸
/// 而非静默产出空清单。
fn emit_features_manifest(manifest_dir: &str) {
    use std::fmt::Write as _;

    println!("cargo:rerun-if-changed=../scripts/customize/features.toml");
    let toml_path = Path::new(manifest_dir).join("../scripts/customize/features.toml");
    let Ok(raw) = std::fs::read_to_string(&toml_path) else {
        // 清单缺失：嵌入空清单（system.features 返回空数组），不阻塞构建。
        let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
        let _ = std::fs::write(Path::new(&out_dir).join("features.json"), "[]");
        return;
    };

    #[derive(serde::Deserialize)]
    struct FeatureEntry {
        id: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        desc: String,
        #[serde(default)]
        category: String,
        // build-profile 条目的 default 是字符串（"release"/"iotsmall"），
        // feature 条目才是 bool——用 Option<toml::Value> 收敛，取值时 as_bool。
        #[serde(default)]
        default: Option<toml::Value>,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        #[serde(default)]
        feature: Vec<FeatureEntry>,
    }

    let parsed: Manifest = match toml::from_str(&raw) {
        Ok(m) => m,
        Err(e) => panic!("features.toml 解析失败（{}）：{e}", toml_path.display()),
    };

    let mut items = Vec::with_capacity(parsed.feature.len());
    for f in &parsed.feature {
        // build-profile 条目（category="build"）选 cargo profile 不是
        // feature，没有对应 CARGO_FEATURE_* env——跳过，不进形态清单。
        if f.category == "build" {
            continue;
        }
        // id → CARGO_FEATURE_ 大写下划线（channels-web → CARGO_FEATURE_CHANNELS_WEB）
        let env_key = format!("CARGO_FEATURE_{}", f.id.replace('-', "_").to_uppercase());
        let enabled = std::env::var(&env_key).is_ok();
        let mut obj = String::new();
        let _ = write!(
            obj,
            "{{\"id\":{},\"label\":{},\"desc\":{},\"category\":{},\"default\":{},\"enabled\":{}}}",
            serde_json_string(&f.id),
            serde_json_string(&f.label),
            serde_json_string(&f.desc),
            serde_json_string(&f.category),
            f.default
                .as_ref()
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            enabled,
        );
        items.push(obj);
    }
    let json = format!("[{}]", items.join(","));

    let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
    std::fs::write(Path::new(&out_dir).join("features.json"), json)
        .expect("写 OUT_DIR/features.json 失败");
}

/// 极简 JSON 字符串字面量转义（build.rs 不依赖 serde_json——控制字符在
/// features.toml 文案里不出现，兜底按 \u 序列转义）。
fn serde_json_string(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
