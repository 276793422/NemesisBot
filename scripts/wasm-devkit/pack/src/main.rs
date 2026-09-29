//! NemesisBot WASM 插件开发包（devkit）一键打包工具。
//!
//! 用法（devkit 根目录下）：
//!
//! ```text
//! cargo run -p pack                # 编译并打包全部示例
//! cargo run -p pack -- textstat    # 只处理指定示例
//! ```
//!
//! 对每个示例（`examples/<name>/`，独立 cargo crate）执行：
//!   1. `cargo build --target wasm32-wasip2 --release`
//!   2. 定位产物 `target/wasm32-wasip2/release/<crate_name>.wasm`（连字符→下划线）
//!   3. 计算 SHA-256，读 `plugin.toml.sample` 把 `wasm-sha256 = "..."` 行替换为真实哈希
//!   4. 产出可安装 staging 目录 `dist/<slug>/`（plugin.toml + plugin.wasm——
//!      载荷归一化为宿主缺省约定名）
//!   5. 打印现成的安装命令（CLI 与 Dashboard 两种）
//!
//! 全程零仓库引用：本工具只认 devkit 自身目录结构，可在任何位置独立运行。

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| {
            eprintln!("无法定位 devkit 根目录（CARGO_MANIFEST_DIR 缺失）——请在 devkit 内用 cargo run 运行");
            std::process::exit(1);
        });

    let examples_dir = root.join("examples");
    if !examples_dir.is_dir() {
        eprintln!("未找到 examples/ 目录（{}）——请在 devkit 根目录结构内运行", examples_dir.display());
        std::process::exit(1);
    }

    // 示例名单：目录序不稳定（平台差异），显式排序保证输出可预期。
    let mut names: Vec<String> = match fs::read_dir(&examples_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir() && e.path().join("Cargo.toml").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(e) => {
            eprintln!("读取 examples/ 失败: {e}");
            std::process::exit(1);
        }
    };
    names.sort();
    if !args.is_empty() {
        names.retain(|n| args.contains(n));
    }
    if names.is_empty() {
        eprintln!("没有匹配的示例（examples/ 下需含 Cargo.toml 的目录）");
        std::process::exit(1);
    }

    let dist_dir = root.join("dist");
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for name in &names {
        println!("\n=== [{name}] 编译 + 打包 ===");
        match build_and_stage(&root, &examples_dir.join(name), name, &dist_dir) {
            Ok(staging) => {
                ok += 1;
                println!("[{name}] staging 就绪: {}", staging.display());
            }
            Err(e) => {
                failed.push(name.clone());
                eprintln!("[{name}] 失败: {e}");
            }
        }
    }

    println!("\n==== 完成：{ok}/{} 成功 ====", names.len());
    if !failed.is_empty() {
        eprintln!("失败示例: {}", failed.join(", "));
        std::process::exit(1);
    }
    println!("\n下一步：把打印出的安装命令复制执行（首次安装会弹审批卡，批准即可）。");
}

/// 单示例全流程：编译 → 哈希 → 填清单 → staging。返回 staging 目录。
fn build_and_stage(root: &Path, example: &Path, name: &str, dist_dir: &Path) -> Result<PathBuf, String> {
    // 1. 编译（继承 stdout/stderr，失败信息原样透出）
    let status = Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip2", "--release"])
        .current_dir(example)
        .status()
        .map_err(|e| format!("启动 cargo 失败（Rust 工具链必须已安装）: {e}"))?;
    if !status.success() {
        return Err("cargo build 失败（若报 can't find crate for core，先执行 rustup target add wasm32-wasip2）".into());
    }

    // 2. 产物定位：crate 名连字符→下划线
    let crate_name = read_crate_name(example)?;
    let wasm_name = format!("{}.wasm", crate_name.replace('-', "_"));
    let wasm_path = example
        .join("target/wasm32-wasip2/release")
        .join(&wasm_name);
    if !wasm_path.is_file() {
        return Err(format!("产物不存在: {}", wasm_path.display()));
    }

    // 3. SHA-256
    let bytes = fs::read(&wasm_path).map_err(|e| format!("读取产物失败: {e}"))?;
    let hash: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();

    // 4. 清单生成：sample 里 wasm-sha256 行替换为真实哈希
    let sample_path = example.join("plugin.toml.sample");
    if !sample_path.is_file() {
        return Err(format!("缺 plugin.toml.sample（{}", sample_path.display()) + "）");
    }
    let sample = fs::read_to_string(&sample_path)
        .map_err(|e| format!("读取 plugin.toml.sample 失败: {e}"))?;
    let manifest = replace_sha_line(&sample, &hash)?;
    let slug = read_slug(&manifest)?;

    // 5. staging：dist/<slug>/{plugin.toml, plugin.wasm}——载荷归一化为
    //    宿主缺省约定名（manifest `wasm` 字段缺省 "plugin.wasm"，sample 模板
    //    同款指引），不带 crate 名：清单零配置即可安装，避免模板与产物名漂移。
    let staging = dist_dir.join(&slug);
    fs::create_dir_all(&staging).map_err(|e| format!("创建 staging 失败: {e}"))?;
    fs::write(staging.join("plugin.toml"), &manifest)
        .map_err(|e| format!("写 plugin.toml 失败: {e}"))?;
    fs::copy(&wasm_path, staging.join("plugin.wasm"))
        .map_err(|e| format!("拷贝载荷失败: {e}"))?;

    // 6. 安装指引
    let staging_disp = staging.display();
    let mut hint = String::new();
    let _ = writeln!(hint, "\n[{name}] 安装方式二选一：");
    let _ = writeln!(hint, "  CLI:      nemesisbot plugin install \"{staging_disp}\" --yes --allow-unsigned");
    let _ = writeln!(hint, "  Dashboard: 插件页 → WASM 插件 → 安装表单填上面路径，勾「允许无签名」");
    println!("{hint}");

    Ok(staging)
}

/// 从示例 Cargo.toml 抠 `[package]` 段的 crate 名。
fn read_crate_name(example: &Path) -> Result<String, String> {
    let text = fs::read_to_string(example.join("Cargo.toml"))
        .map_err(|e| format!("读取 Cargo.toml 失败: {e}"))?;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            // 越过 [package] 段还没看到 name = 就不找了（本工具只认自己的模板形态）
            if !t.starts_with("[package]") {
                break;
            }
        }
        if let Some(v) = t.strip_prefix("name =") {
            return Ok(unquote(v));
        }
    }
    Err("Cargo.toml [package] 段无 name 键".into())
}

/// 从生成的 manifest 抠 slug（staging 目录名与安装提示用）。
fn read_slug(manifest: &str) -> Result<String, String> {
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            break; // slug 是根键，进 table 段就没了
        }
        if let Some(v) = t.strip_prefix("slug =") {
            let s = unquote(v);
            if s.is_empty() {
                break;
            }
            return Ok(s);
        }
    }
    Err("plugin.toml.sample 无根键 slug".into())
}

/// 把 sample 中的 `wasm-sha256 = "..."` 行整行替换为真实哈希（键级匹配，
/// 不依赖占位符文本形态）。根键必须位于任何 table 段之前（TOML 语义）。
fn replace_sha_line(sample: &str, hash: &str) -> Result<String, String> {
    let mut replaced = false;
    let mut out = String::with_capacity(sample.len() + 80);
    for line in sample.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if !replaced {
                return Err("plugin.toml.sample 头部（table 段之前）无 wasm-sha256 行——\
                            根键必须位于任何 table 段之前"
                    .into());
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !replaced && t.starts_with("wasm-sha256") {
            out.push_str(&format!("wasm-sha256 = \"{hash}\"\n"));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        return Err("plugin.toml.sample 无 wasm-sha256 行".into());
    }
    Ok(out)
}

/// 去掉 ` = ` 右值的引号与行尾注释。
fn unquote(value: &str) -> String {
    let v = value.trim();
    let v = v.split('#').next().unwrap_or("").trim();
    v.trim_matches('"').trim_matches('\'').to_string()
}
