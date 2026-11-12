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

    // Git commit hash (short)：显式 env 优先（构建脚本/CI 注入通道，
    // build-android.bat 同款），缺席才本地跑 git rev-parse。
    let git_commit = match std::env::var("NEMESISBOT_GIT_COMMIT") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => Command::new("git")
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
            .unwrap_or_default(),
    };
    if git_commit.is_empty() {
        // 失败留痕：空串会被 cargo 缓存为 rustc-env 长期存活（版本显示
        // 面静默丢 git 后缀），构建日志必须有痕可查。cargo:warning= 只在
        // stdout 被 cargo 解析成构建警告，eprintln 无效。
        println!(
            "cargo:warning=nemesisbot build: git rev-parse 失败且无 NEMESISBOT_GIT_COMMIT 注入，版本信息将不含 git hash"
        );
    }

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

    // Re-run build script when git state changes（BUG-20261109-016 根修）。
    //
    // rerun-if-changed 路径相对**包根**（nemesisbot/）而非仓库根——旧声明
    // `.git/HEAD` 实际指向 `nemesisbot/.git/HEAD`（不存在，cargo 对缺席
    // 路径静默空操作）= commit/branch 从不触发重跑，NEMESISBOT_GIT_COMMIT
    // 冻结在最后一次重跑时的值。且就算指到仓库根，`.git/HEAD` 在 commit
    // 后也不变（符号引用，checkout 才变）——commit 落在 refs/heads/ 或
    // packed-refs（gc 后 loose refs 收编）。三路全盯 + 显式 env 通道：
    let repo_git = Path::new(&manifest_dir).join("../.git");
    println!("cargo:rerun-if-changed={}", repo_git.join("HEAD").display());
    println!(
        "cargo:rerun-if-changed={}",
        repo_git.join("refs/heads").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo_git.join("packed-refs").display()
    );
    println!("cargo:rerun-if-env-changed=NEMESISBOT_GIT_COMMIT");

    // ------------------------------------------------------------------
    // 太微子系统 rlib 直连探测（2026-11-13 最终形态，取代归档手术直链）：
    //   slots/taiwei/impl（submodule）的 target/release/libnemesis_taiwei.rlib
    //   在场 → 四防线版本对质（stamp 两行：git 短 commit + rustc hash）→
    //   rustc-cfg=nb_taiwei 点亮引用面；缺席 → 零动作（纯开源构建：零 cfg、
    //   零 taiwei 符号）。
    // 链接机制：.cargo/config.toml 静态 --extern 把 rlib 挂进 extern 搜
    // 索面（dangling 未引用 = 零影响），宿主 lto="thin" 的 ThinLTO 管线
    // 直接吸收 taiwei bitcode——opaque archive 直链在 LTO 下结构性不可行
    //（std 符号在编译期已被内化，链接期任何输入都无定义可拉，2026-11-13
    // 实证 20 undefined / /INCLUDE 无效）。rlib 是 IR 载体，宿主各 profile
    // 最终 codegen，单产物服务全 profile。
    // 同一条 `cargo build --release`，目录状态自己说话——宿主 manifest/
    // 依赖图零 taiwei 痕迹。商业形态 = 先在 slots/taiwei/impl 跑一次
    // `cargo build --release`，再回根目录正常构建（两条命令，编译习惯与
    // 输出位置不变）。
    // ------------------------------------------------------------------
    probe_taiwei_rlib(&manifest_dir);

    // ------------------------------------------------------------------
    // 构建形态清单（system.features WSAPI 数据源）：解析
    // scripts/customize/features.toml（feature 清单单一真相源，50 条），
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

/// 太微 rlib 探测 + stale 四防线（详见下方逐段注释）。
///
/// 防线（stamp 由 taiwei-impl 自己的 build.rs 落盘，两行：第 1 行 = 短
/// commit [+ `-dirty`]，第 2 行 = rustc commit-hash；**条件写**——内容
/// 不变不 touch，mtime 语义可靠的前提）：
/// - lib 在场 + stamp 缺席 → 硬错（老构建，版本不可对质）；
/// - stamp 晚于 lib ≥2s → 硬错（上次 taiwei 构建半途而废：版本变了、
///   lib 未重编；条件写保证 stamp 刷新必然伴随版本 payload 变化）；
/// - rlib 与 staticlib mtime 差 ≥2s / staticlib 缺席 → 硬错（单产物链接
///   失败的半途而废形态——两产物同一次 rustc 调用产出，正常构建毫秒级
///   相邻）；
/// - stamp git 行 ≠ submodule HEAD → 硬错（taiwei 源更新后未重编 = 旧库假新链）；
/// - stamp rustc 行 ≠ 宿主 rustc → 硬错（rlib 元数据跨编译器版本无消费
///   保证——E0514 前置拦截，报错可读）；
/// - stamp `-dirty` / `unknown` / git 对质不了 → WARN 放行（诚实边界：
///   该形态版本不可验证，不阻塞无 git 环境）。
///
/// 放行后仅 emit `cargo:rustc-cfg=nb_taiwei`——链接交给 .cargo/config.toml
/// 的静态 --extern 与宿主 ThinLTO 管线（无 link-lib、无归档手术）。
fn probe_taiwei_rlib(manifest_dir: &str) {
    // 缺席形态也声明预期 cfg（压掉 rustc unexpected_cfgs 检查）
    println!("cargo::rustc-check-cfg=cfg(nb_taiwei)");

    // 交叉编译目标不探测：rlib 按宿主三元组编译，元数据跨三元组不可消费
    //（rustc loud 报错）。直连形态只覆盖本机目标（android/musl 交叉构建
    // 走各自脚本，天然不受影响）。
    let target = std::env::var("TARGET").unwrap_or_default();
    let host = std::env::var("HOST").unwrap_or_default();
    if target != host {
        return;
    }

    let impl_root = Path::new(manifest_dir).join("../slots/taiwei/impl");
    // 固定 release 目录（不分宿主 profile）：rlib 携带的 bitcode 是 IR，
    // 宿主 release/debug/iotsmall 各自最终 codegen——单一产物服务全 profile，
    // 旧 staticlib 形态的 profile 配对问题在 rlib+LTO 路线下不存在。
    let lib_dir = impl_root.join("target").join("release");
    let lib_name = "libnemesis_taiwei.rlib"; // Windows/Linux 同名
    let lib_path = lib_dir.join(lib_name);
    // profile 目录级 watch：rlib 落库/重链/stamp 更新都能触发重探；目录
    // 缺席（纯开源常态）= cargo 对缺席路径静默空操作，零开销。
    println!("cargo:rerun-if-changed={}", lib_dir.display());
    if !lib_path.is_file() {
        // 其他 profile 目录有库 → 诚实提醒（商业形态契约 = taiwei 侧
        // `cargo build --release`；debug-only 构建不进直连）
        let others = sibling_profile_libs(&impl_root, lib_name);
        if !others.is_empty() {
            println!(
                "cargo:warning=taiwei rlib 在 release 目录缺席，但其他 profile 目录已有库: {}——按开源形态构建；需商业形态时先在 slots/taiwei/impl 跑 cargo build --release",
                others.join(", ")
            );
        }
        return;
    }

    // ---- 防线 1：stamp 在场性（旧格式/半途删除也在此拦）----
    let stamp_path = lib_dir.join(format!("{lib_name}.stamp"));
    let Some(stamp_raw) = std::fs::read_to_string(&stamp_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        error_exit(&[
            &format!(
                "taiwei rlib 在场但缺版本戳（{}）——老构建，版本不可对质",
                stamp_path.display()
            ),
            "修复: 在 slots/taiwei/impl 重跑 cargo build --release（或删除该 rlib 恢复开源形态）",
        ]);
    };
    let mut stamp_lines = stamp_raw.lines();
    let stamp_git = stamp_lines.next().unwrap_or_default().trim().to_string();
    let stamp_rustc = stamp_lines
        .next()
        .map(|l| l.trim().to_string())
        .unwrap_or_default();

    // ---- 防线 2a：stamp/lib mtime 线（stamp 晚于 lib = 上次构建半途而
    // 废：版本 payload 变了、lib 未重编。条件写（taiwei build.rs 内容不变
    // 不 touch）是本线无负响的前提——build.rs 无谓 rerun（如 cargo test
    // 轮 dev-deps feature 集变化触发重跑）payload 不变 → 跳过写 → stamp
    // mtime 不动，不制造「stamp 新 lib 旧」假象；2026-11-13 实证误报后
    // 与条件写配套重设计）----
    let lib_mt = std::fs::metadata(&lib_path).and_then(|m| m.modified()).ok();
    let stamp_mt = std::fs::metadata(&stamp_path)
        .and_then(|m| m.modified())
        .ok();
    if let (Some(l), Some(s)) = (lib_mt, stamp_mt)
        && let Ok(delta) = s.duration_since(l)
        && delta.as_secs() >= 2
    {
        error_exit(&[
            &format!("taiwei rlib 构建半途而废（版本戳晚于库文件 {delta:?}）"),
            "修复: 在 slots/taiwei/impl 重跑 cargo build --release",
        ]);
    }

    // ---- 防线 2b：双产物 mtime 互证（rlib 与 staticlib 必同轮落盘）----
    // crate-type = ["rlib","staticlib"] 两产物同一次 rustc 调用产出，正常
    // 构建 mtime 毫秒级相邻；单侧链接失败则差距拉开——抓住 2a 看不到的
    // 「版本戳已对齐但编译内部单产物失败」形态。
    let peer_name = if target.contains("windows") {
        "nemesis_taiwei.lib"
    } else {
        "libnemesis_taiwei.a"
    };
    let peer_mt = std::fs::metadata(lib_dir.join(peer_name))
        .and_then(|m| m.modified())
        .ok();
    match (lib_mt, peer_mt) {
        (Some(l), Some(p)) => {
            let gap = l.duration_since(p).or_else(|_| p.duration_since(l));
            if let Ok(gap) = gap
                && gap.as_secs() >= 2
            {
                error_exit(&[
                    &format!(
                        "taiwei 构建半途而废（双产物不同轮：rlib 与 {peer_name} 相差 {gap:?}）"
                    ),
                    "修复: 在 slots/taiwei/impl 重跑 cargo build --release",
                ]);
            }
        }
        (Some(_), None) => {
            error_exit(&[
                &format!(
                    "taiwei rlib 在场但 staticlib（{peer_name}）缺席——构建形态不可信（crate-type 不全或产物被删）"
                ),
                "修复: 在 slots/taiwei/impl 重跑 cargo build --release",
            ]);
        }
        (None, _) => {
            error_exit(&[
                "taiwei rlib mtime 读取失败（探测后被移动/锁定？）",
                "修复: 重跑构建",
            ]);
        }
    }

    // ---- 防线 3：git 版本线（stamp == submodule HEAD 才放行）----
    if stamp_git == "unknown" {
        println!(
            "cargo:warning=taiwei rlib 版本戳 unknown（taiwei 侧构建时 git 缺席）——链接未经版本对质"
        );
    } else {
        let head = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&impl_root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        match head {
            Some(h) if stamp_git == h => {}
            Some(h) if stamp_git.strip_suffix("-dirty") == Some(h.as_str()) => {
                println!(
                    "cargo:warning=taiwei rlib 来自未提交工作区（stamp={stamp_git}，HEAD={h}）——发布前先在 taiwei 仓提交对齐"
                );
            }
            Some(h) => {
                error_exit(&[
                    &format!(
                        "taiwei rlib 版本错配（stamp={stamp_git} ≠ submodule HEAD={h}）——taiwei 源更新后未重编，旧库假新链"
                    ),
                    "修复: 在 slots/taiwei/impl 重跑 cargo build --release",
                ]);
            }
            None => {
                println!(
                    "cargo:warning=taiwei rlib 版本戳无法对质（git rev-parse 失败）——链接未经版本验证"
                );
            }
        }
    }

    // ---- 防线 4：rustc 版本线（rlib 元数据跨版本不可消费）----
    if stamp_rustc.is_empty() {
        error_exit(&[
            "taiwei 版本戳缺 rustc 行（旧格式 stamp）——请重编 taiwei rlib",
            "修复: 在 slots/taiwei/impl 重跑 cargo build --release",
        ]);
    }
    match host_rustc_commit_hash() {
        Some(h) if h == stamp_rustc => {}
        Some(h) => error_exit(&[
            &format!(
                "taiwei rlib 与宿主编译器不一致（库 rustc={}，宿主 rustc={h}）——rlib 元数据跨编译器版本无消费保证",
                stamp_rustc
            ),
            "修复: 用与宿主一致的 rustc 工具链重编 slots/taiwei/impl",
        ]),
        None => error_exit(&[
            "宿主 rustc commit-hash 不可解析（发行版自编 rustc？）——无法与 taiwei rlib 对质编译器版本，拒绝链接",
        ]),
    }

    println!("cargo:rustc-cfg=nb_taiwei");
}

/// cargo:error 逐行输出 + 构建失败退出（build.rs 的硬错惯例）。
fn error_exit(lines: &[&str]) -> ! {
    for l in lines {
        println!("cargo:error={l}");
    }
    std::process::exit(1);
}

/// 宿主 rustc 的 commit-hash（40 hex；RUSTC env 由 cargo 注入，比 PATH
/// 裸名可靠）。`unknown`（发行版自编）→ None = 拒绝对质。
fn host_rustc_commit_hash() -> Option<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc)
        .args(["-vV"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(h) = line.strip_prefix("commit-hash: ") {
            let h = h.trim();
            return (h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| h.to_string());
        }
    }
    None
}

/// 其他 profile 目录下已存在的 rlib 清单（提示用，排序稳定）。
fn sibling_profile_libs(impl_root: &Path, lib_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(impl_root.join("target")) else {
        return out;
    };
    for e in entries.flatten() {
        if e.path().join(lib_name).is_file()
            && let Some(name) = e.file_name().to_str()
        {
            out.push(name.to_string());
        }
    }
    out.sort();
    out
}
