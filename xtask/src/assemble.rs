//! 装配编排：preflight → 递增同步 → 换指 → 环境增量清理 → 构建 → 验证 → dist。
//!
//! 全部写入收敛在 `assembly/`（git 忽略）：复制体（`assembly/nemesisbot`）、
//! 共享 target（`assembly/target`）、交付物（`assembly/dist`）、构建环境指纹
//!（`assembly/build_env.json`）。公开工作区与私有实现仓零写入——装配前后
//! `git status` 恒干净。
//!
//! 环境增量闸：`option_env!` 不进 cargo fingerprint——`NEMESIS_TELEMETRY_*`
//! 变了不做定向清理会把上次的旧值烤进新 exe。指纹（build_env.json）有变
//! 时 `cargo clean -p nemesis-taiwei` 定向刷新，无变走增量快路径。

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{AssembleOpts, Profile};
use crate::{copy_tree, transform};

pub const ASSEMBLY_DIR: &str = "assembly";
pub const COPY_DIR: &str = "nemesisbot";
pub const TARGET_DIR: &str = "target";
pub const DIST_DIR: &str = "dist";
pub const ENV_META_FILE: &str = "build_env.json";

pub const ENV_ENDPOINT: &str = "NEMESIS_TELEMETRY_ENDPOINT";
pub const ENV_PUBKEY: &str = "NEMESIS_TELEMETRY_PUBKEY";
pub const ENV_TOKEN: &str = "NEMESIS_TELEMETRY_TOKEN";

/// 链接产物里的实现指纹（双双命中 = 真实现编进 exe）。公开仓源码零命中
///（grep 实证），stub 误开连编译都过不了——双保险。
/// `client.rs` 上报路径字面量 + `host.rs` 运行期 env 名。
pub const IMPL_MARKERS: &[&[u8]] = &[b"/v1/events", b"NEMESIS_TELEMETRY_KEY_ID"];

/// 编译期注入生效值（None = 不设 env，实现内建默认烤入）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveTelemetry {
    pub endpoint: Option<String>,
    pub pubkey: Option<String>,
    pub token: Option<String>,
}

impl EffectiveTelemetry {
    /// 指纹文件文本（逐行 key=value；值已禁换行/空白敏感值，字符串整体
    /// 等值比较即指纹比较）。
    fn fingerprint(&self, profile: &str) -> String {
        format!(
            "profile={profile}\nendpoint={}\npubkey={}\ntoken={}",
            self.endpoint.as_deref().unwrap_or(""),
            self.pubkey.as_deref().unwrap_or(""),
            self.token.as_deref().unwrap_or(""),
        )
    }
}

pub fn run(opts: &AssembleOpts) -> Result<()> {
    let repo = repo_root()?;
    let assembly = repo.join(ASSEMBLY_DIR);
    let copy = assembly.join(COPY_DIR);

    preflight(&repo, opts)?;
    let eff = resolve_effective(opts);
    let commit = git_short_commit(&repo);

    if opts.clean {
        println!("① 清空装配工作区 {}（--clean）", assembly.display());
        remove_dir_all_resilient(&assembly)?;
    }

    println!("② 同步仓库 → 复制体 {}（mtime 保持，增量）", copy.display());
    let stats = copy_tree::sync_repo(&repo, &copy)?;
    for w in &stats.warnings {
        println!("   WARN: {w}");
    }

    println!("③ 复制体换指：stub 槽位 → 实现挂载点");
    transform::apply_to_file(&copy.join("nemesisbot").join("Cargo.toml"))?;

    ensure_fresh_env_bake(&assembly, opts.profile, &eff)?;

    println!(
        "⑤ 构建：cargo build --profile {} -p nemesisbot --features taiwei",
        opts.profile.as_str()
    );
    let exe = cargo_build(&repo, &copy, opts.profile, &eff, commit.as_deref())?;

    println!("⑥ 验证链接产物（实现指纹 + 注入值烤入面）");
    verify_baked(&exe, &eff)?;

    println!("⑦ 交付物落位 assembly/dist/");
    let dist_exe = publish_dist(&assembly, &exe)?;
    let size_mb = std::fs::metadata(&dist_exe)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);

    println!("=== 装配完成 ===");
    println!("复制体     : {}", copy.display());
    println!("共享 target: {}", assembly.join(TARGET_DIR).display());
    println!("profile    : {}", opts.profile.as_str());
    println!("交付物     : {}（{size_mb} MB）", dist_exe.display());
    println!(
        "endpoint   : {}",
        eff.endpoint.as_deref().unwrap_or("实现内建默认")
    );
    println!(
        "信封公钥   : {}",
        eff.pubkey.as_deref().unwrap_or("实现内建默认")
    );
    println!(
        "产品 token : {}",
        if eff.token.is_some() {
            "已注入（不回显）"
        } else {
            "实现内建默认"
        }
    );
    println!(
        "git commit : {}",
        commit
            .as_deref()
            .unwrap_or("（未取到——exe 版本面将无 hash 后缀）")
    );
    println!(
        "同步       : 复制 {} 文件 / 跳过未变 {} / 新建目录 {} / 镜像删除 {} 文件 {} 目录",
        stats.copied,
        stats.skipped_unchanged,
        stats.dirs_created,
        stats.pruned_files,
        stats.pruned_dirs
    );
    Ok(())
}

/// 仓库根 = xtask crate 父目录（CARGO_MANIFEST_DIR 编译期钉死，运行位置无关）。
fn repo_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .context("定位仓库根（xtask 父目录）")
}

/// 装配前置检查：实现挂载点在场 + manifest 契约 + 前端产物 + 注入参数格式。
fn preflight(repo: &Path, opts: &AssembleOpts) -> Result<()> {
    let impl_manifest = repo.join("slots/taiwei/impl/crates/taiwei-impl/Cargo.toml");
    if !impl_manifest.is_file() {
        bail!(
            "私有实现挂载点缺席：{}\n先执行：git submodule update --init slots/taiwei/impl",
            impl_manifest.display()
        );
    }
    let mtext = std::fs::read_to_string(&impl_manifest)
        .with_context(|| format!("读取实现 manifest {}", impl_manifest.display()))?;
    if !mtext.contains("name = \"nemesis-taiwei\"") {
        bail!(
            "实现包名不符：{} 必须是 package name = nemesis-taiwei（nemesisbot 依赖名/feature 透传的锚点）",
            impl_manifest.display()
        );
    }
    if !mtext.contains("geoloc-win") {
        bail!(
            "实现 manifest 缺 geoloc-win feature——nemesisbot taiwei feature 的透传目标（stub 同名空声明教训：cargo 对 feature 转发的校验与开关激活无关）"
        );
    }

    let static_index = repo.join("crates/nemesis-web/static/index.html");
    if !static_index.is_file() {
        bail!(
            "前端构建产物缺席：{}\n先执行：npm --prefix web install && npm --prefix web run build\n（或使用一键脚本 scripts/build-windows.bat，再重跑装配）",
            static_index.display()
        );
    }

    if let Some(ep) = &opts.endpoint {
        validate_endpoint(ep)?;
    }
    if let Some(pk) = &opts.pubkey {
        validate_pubkey(pk)?;
    }
    if let Some(t) = &opts.token {
        if t.trim().is_empty() {
            bail!("--token 为空（有效值才注入；不想注入就别传）");
        }
        if t.chars().any(char::is_control) {
            bail!("--token 含控制字符（指纹文件按行比较，控制值非法）");
        }
    }

    // 装配以**工作区内容**构建（含未提交改动），exe 版本面盖 HEAD commit——
    // 两者可能不一致。发布纪律：商业交付前先提交，让盖章与内容对齐。
    if let Some(n) = git_dirty_count(repo)
        && n > 0
    {
        println!(
            "WARN: 工作区有 {n} 项未提交改动——exe 以工作区内容构建但以 HEAD commit 盖章（商业发布前应先提交对齐）"
        );
    }
    Ok(())
}

/// `git status --porcelain` 非空行数（git 缺席/失败 = None 不提醒）。
fn git_dirty_count(repo: &Path) -> Option<usize> {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
        })
}

pub fn validate_endpoint(s: &str) -> Result<()> {
    if !s.starts_with("http://") && !s.starts_with("https://") {
        bail!("--endpoint 必须以 http:// 或 https:// 开头，收到 {s:?}");
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("--endpoint 含空白/控制字符");
    }
    Ok(())
}

/// sec1：compressed 33B（66 hex）/ uncompressed 65B（130 hex）。点在曲线上的
/// 精确校验由实现 TelemetryClient::new fail-closed 承担，这里只挡明显手误。
pub fn validate_pubkey(s: &str) -> Result<()> {
    if s.len() != 66 && s.len() != 130 {
        bail!(
            "--pubkey 长度须为 66（compressed）或 130（uncompressed）hex，收到 {} 字符",
            s.len()
        );
    }
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("--pubkey 含非 hex 字符");
    }
    Ok(())
}

/// CLI > ambient env > None（None = 实现内建默认）。ambient 空串 = 未设
///（与实现 env_with_default 语义对齐）。
pub fn resolve_effective(opts: &AssembleOpts) -> EffectiveTelemetry {
    resolve_with_ambient(opts, &|key| ambient(key))
}

/// 纯内核（ambient 取值器注入——测试不改进程 env，并行安全）。空白 ambient
/// 值在此统一按未设处理（与实现 env_with_default 语义对齐）。
fn resolve_with_ambient(
    opts: &AssembleOpts,
    get: &dyn Fn(&str) -> Option<String>,
) -> EffectiveTelemetry {
    let pick = |key: &str| -> Option<String> {
        opts_opt(opts, key).or_else(|| get(key).filter(|v| !v.trim().is_empty()))
    };
    EffectiveTelemetry {
        endpoint: pick(ENV_ENDPOINT),
        pubkey: pick(ENV_PUBKEY),
        token: pick(ENV_TOKEN),
    }
}

/// CLI 侧取值（哪个 env 键对应哪个 CLI 选项）。
fn opts_opt(opts: &AssembleOpts, key: &str) -> Option<String> {
    match key {
        ENV_ENDPOINT => opts.endpoint.clone(),
        ENV_PUBKEY => opts.pubkey.clone(),
        ENV_TOKEN => opts.token.clone(),
        _ => None,
    }
}

fn ambient(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// option_env! 烤入值刷新闸：指纹有变（或指纹文件缺席且 target 已存在）→
/// 定向 clean nemesis-taiwei；否则增量快路径。首次构建（无 target）不清理。
fn ensure_fresh_env_bake(
    assembly: &Path,
    profile: Profile,
    eff: &EffectiveTelemetry,
) -> Result<()> {
    let meta_path = assembly.join(ENV_META_FILE);
    let fp = eff.fingerprint(profile.as_str());
    let target_profile_dir = assembly.join(TARGET_DIR).join(profile.as_str());
    if target_profile_dir.exists() {
        let previous = std::fs::read_to_string(&meta_path).ok();
        if previous.as_deref() != Some(fp.as_str()) {
            println!("④ 构建环境变化 → 定向清理 nemesis-taiwei（option_env! 烤入值刷新）");
            let target_dir = assembly.join(TARGET_DIR);
            let status = Command::new("cargo")
                .args([
                    "clean",
                    "--profile",
                    profile.as_str(),
                    "-p",
                    "nemesis-taiwei",
                ])
                .current_dir(assembly.join(COPY_DIR))
                .env("CARGO_TARGET_DIR", &target_dir)
                .status()
                .context("spawn cargo clean")?;
            if !status.success() {
                bail!(
                    "cargo clean 失败（exit {}）——target 可能被在跑构建占用",
                    status.code().unwrap_or(-1)
                );
            }
        }
    }
    std::fs::create_dir_all(assembly).ok();
    std::fs::write(&meta_path, fp).with_context(|| format!("写环境指纹 {}", meta_path.display()))
}

fn cargo_build(
    repo: &Path,
    copy: &Path,
    profile: Profile,
    eff: &EffectiveTelemetry,
    commit: Option<&str>,
) -> Result<PathBuf> {
    let target_dir = repo.join(ASSEMBLY_DIR).join(TARGET_DIR);
    let mut cmd = Command::new("cargo");
    cmd.current_dir(copy)
        .args([
            "build",
            "--profile",
            profile.as_str(),
            "-p",
            "nemesisbot",
            "--features",
            "taiwei",
        ])
        .env("CARGO_TARGET_DIR", &target_dir);
    if let Some(v) = &eff.endpoint {
        cmd.env(ENV_ENDPOINT, v);
    }
    if let Some(v) = &eff.pubkey {
        cmd.env(ENV_PUBKEY, v);
    }
    if let Some(v) = &eff.token {
        cmd.env(ENV_TOKEN, v);
    }
    if let Some(c) = commit {
        cmd.env("NEMESISBOT_GIT_COMMIT", c);
    }
    let status = cmd.status().context("spawn cargo build")?;
    if !status.success() {
        bail!(
            "装配构建失败（cargo exit {}）——上方为完整编译现场；复制体保留在 {} 供排查",
            status.code().unwrap_or(-1),
            copy.display()
        );
    }
    Ok(target_dir.join(profile.as_str()).join(exe_name()))
}

pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "nemesisbot.exe"
    } else {
        "nemesisbot"
    }
}

/// 验证真实现编进产物 + 显式注入值确实烤入（缺一即拒交付）。
pub fn verify_baked(exe: &Path, eff: &EffectiveTelemetry) -> Result<()> {
    let bytes = std::fs::read(exe).with_context(|| format!("读取产物 {}", exe.display()))?;
    for m in IMPL_MARKERS {
        if !contains(&bytes, m) {
            bail!(
                "装配验证失败：exe 里找不到实现指纹 {:?}——真实现未编进产物，禁止交付",
                String::from_utf8_lossy(m)
            );
        }
    }
    if let Some(ep) = &eff.endpoint
        && !contains(&bytes, ep.as_bytes())
    {
        bail!("装配验证失败：endpoint {ep:?} 未烤入 exe（option_env! 注入链断裂？）");
    }
    if let Some(pk) = &eff.pubkey
        && !contains(&bytes, pk.as_bytes())
    {
        bail!("装配验证失败：信封公钥未烤入 exe（option_env! 注入链断裂？）");
    }
    Ok(())
}

/// 字节序列包含判定（朴素窗口扫描——产物 ~百 MB，3 次扫描秒级可接受）。
pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && hay.len() >= needle.len()
        && hay.windows(needle.len()).any(|w| w == needle)
}

fn publish_dist(assembly: &Path, exe: &Path) -> Result<PathBuf> {
    let dist_dir = assembly.join(DIST_DIR);
    std::fs::create_dir_all(&dist_dir)
        .with_context(|| format!("创建交付目录 {}", dist_dir.display()))?;
    let dist_exe = dist_dir.join(exe_name());
    std::fs::copy(exe, &dist_exe).with_context(|| {
        format!(
            "拷贝交付物 {}（若被占用请先关闭在跑实例）",
            dist_exe.display()
        )
    })?;
    Ok(dist_exe)
}

fn git_short_commit(repo: &Path) -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 韧性删除（Windows AV/索引器瞬时句柄是常态）：3 次重试 + 退避。目录缺席
/// = 已是目标态，幂等成功。
fn remove_dir_all_resilient(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut last_err = None;
    for attempt in 0..3 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(300 * (attempt + 1)));
            }
        }
    }
    let e = last_err.expect("有错才走到这");
    bail!(
        "清空 {} 三次尝试均失败：{e}（在跑的构建/占用的资源请先关闭）",
        dir.display()
    )
}

#[cfg(test)]
mod tests;
