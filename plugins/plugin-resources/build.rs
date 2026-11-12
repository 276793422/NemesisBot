//! 编译期附加资源内嵌：`$OUT_DIR/persona_catalog.json` 由本脚本保证**永远
//! 存在**，`lib.rs` 用 `include_str!(OUT_DIR)` 吃进二进制（快照载体）。
//!
//! 三条路径（2026-11-12，与 `crates/nemesis-data/build.rs` 价目表逐条同构）：
//! - **默认（`NEMESIS_PERSONA_REFRESH` / `NEMESIS_PERSONA_FILE` 均未设）**：
//!   纯离线——仓库基线 `resources/persona-catalog/agency-agents/md/` 组装进
//!   产物，零网络（本地/离线/受限网络构建永不因下载卡住或失败）；仓库基线
//!   文件不被改写，git 工作区保持干净。
//! - **`NEMESIS_PERSONA_FILE=<路径>`**（优先级最高）：官方构建脚本预下载的
//!   上游 tarball 缓存——同一构建会话只在脚本里下载一次；解析/校验失败 →
//!   `cargo:warning` + 基线兜底，绝不内嵌坏数据。
//! - **`NEMESIS_PERSONA_REFRESH=<指纹>`**（FILE 未设时）：直拉上游 tarball
//!   （codeload 一次全量，不逐文件打 API）→ 机械裁剪（`*.md` + 根级 LICENSE
//!   原文，§3.3）→ 校验（≥1 文件 + LICENSE 在场）→ 写 OUT_DIR。任一步失败 →
//!   `cargo:warning` + 基线兜底（fail-open：构建永不因下载失败而失败）。指纹
//!   值（上游 commit / 缓存 size+mtime）兼当 `rerun-if-env-changed` 重跑信号
//!   ——内容变了才重跑，日常增量构建零网络。
//! - **`NEMESIS_PERSONA_OFFLINE=1`**：强制基线逃生口（设计 §3.2）——无视
//!   FILE/REFRESH 直落仓库基线，离线/可复现构建的确定性开关。
//!
//! 溯源注入 `NEMESIS_PERSONA_EMBED_SOURCE`（downloaded / pre-downloaded /
//! bundled snapshot，价目表同构），进 manifest，shop 响应可追溯。
//!
//! 共享逻辑（机械裁剪/manifest/catalog 组装）在 `src/resource_filter.rs`，
//! `#[path]` include 进本脚本——与 lib 统一定义。

#[path = "src/resource_filter.rs"]
mod resource_filter;

use std::io::Read;

use resource_filter::{
    BASELINE_DIR, BASELINE_LICENSE, BASELINE_MD_DIR, BASELINE_META, CATALOG_ID, OUT_FILE,
    UPSTREAM_LICENSE, UPSTREAM_REPO, assemble_catalog, date_part, format_epoch_secs, is_md,
    strip_tarball_root, tarball_url,
};

/// 单次下载超时（两层）：连接 10s + 总预算 120s（实测慢网络拉 1.7MB
/// tarball 可超 30s——价目表 30s 经验不够；REFRESH 只在官方构建/显式触发
/// 发生，本地默认离线路径零网络，最坏停顿 2 分钟换 fail-open 兜底可接受）。
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 10;
const DOWNLOAD_TOTAL_TIMEOUT_SECS: u64 = 120;

fn main() {
    println!("cargo:rerun-if-env-changed=NEMESIS_PERSONA_FILE");
    println!("cargo:rerun-if-env-changed=NEMESIS_PERSONA_REFRESH");
    println!("cargo:rerun-if-env-changed=NEMESIS_PERSONA_OFFLINE");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/resource_filter.rs");
    println!("cargo:rerun-if-changed={BASELINE_DIR}");
    println!("cargo:rerun-if-changed={BASELINE_META}");
    println!("cargo:rerun-if-changed={BASELINE_LICENSE}");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let dst = std::path::Path::new(&out_dir).join(OUT_FILE);
    let built_at = utc_now_string();

    // 强制基线逃生口（设计 §3.2：NEMESIS_PERSONA_OFFLINE=1 → 无视 FILE/REFRESH
    // 直落基线——离线/可复现构建的确定性开关，优先级压过一切）。
    let offline = std::env::var("NEMESIS_PERSONA_OFFLINE")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let file_mode = if offline {
        None
    } else {
        std::env::var("NEMESIS_PERSONA_FILE")
            .ok()
            .filter(|v| !v.trim().is_empty())
    };
    let refresh = if offline {
        None
    } else {
        std::env::var("NEMESIS_PERSONA_REFRESH")
            .ok()
            .filter(|v| !v.trim().is_empty())
    };
    if offline && (file_mode.is_some() || refresh.is_some()) {
        println!(
            "cargo:warning=[plugin-resources] NEMESIS_PERSONA_OFFLINE=1 —— FILE/REFRESH 已忽略，强制仓库基线"
        );
    }

    let (catalog, source) = if let Some(path) = file_mode {
        // 构建脚本预下载（会话内唯一网络点）：读本地 tarball，与网络路径同一套
        // 解析/校验；失败同样基线兜底（fail-open 语义不变）。
        let commit = refresh.clone().unwrap_or_else(|| "unknown".to_string());
        match std::fs::File::open(&path) {
            Ok(mut f) => match tarball_catalog(&mut f, &commit, &built_at, "pre-downloaded") {
                Ok(pair) => pair,
                Err(err) => {
                    println!(
                        "cargo:warning=[plugin-resources] 预下载人格目录不可用（{err}），使用仓库基线兜底"
                    );
                    baseline_catalog(&built_at)
                }
            },
            Err(err) => {
                println!(
                    "cargo:warning=[plugin-resources] 预下载 tarball 打不开（{path}: {err}），使用仓库基线兜底"
                );
                baseline_catalog(&built_at)
            }
        }
    } else {
        match refresh {
            None => baseline_catalog(&built_at),
            Some(fingerprint) => match fetched_catalog(&fingerprint, &built_at) {
                Ok(pair) => pair,
                Err(err) => {
                    println!(
                        "cargo:warning=[plugin-resources] 人格目录下载失败（{err}），使用仓库基线兜底"
                    );
                    baseline_catalog(&built_at)
                }
            },
        }
    };

    std::fs::write(&dst, &catalog).unwrap_or_else(|e| panic!("write {}: {e}", dst.display()));
    println!("cargo:rustc-env=NEMESIS_PERSONA_EMBED_SOURCE={source}");
}

// ---------------------------------------------------------------------------
// 三路径（baseline / pre-downloaded / fetched）→ catalog JSON
// ---------------------------------------------------------------------------

/// 基线兜底：仓库 `md/` 子树组装。元数据来自基线 manifest（刷新脚本
/// 写入；缺失/损坏时诚实落 "unknown" 溯源，不猜）。许可原文读 crate 根
/// `LICENSE-upstream`（缺失 = 组装 Err = panic——分发合规硬前提，仓库被破坏）。
fn baseline_catalog(built_at: &str) -> (String, String) {
    let files = walk_baseline_md();
    let license_text = std::fs::read_to_string(BASELINE_LICENSE).unwrap_or_else(|e| {
        panic!(
            "[plugin-resources] {BASELINE_LICENSE} 不可读（{e}）——许可原文是内嵌再分发的\
             硬前提，基线目录被破坏，请走 scripts/update-persona-baseline.sh 重建"
        )
    });
    let meta: resource_filter::BaselineMeta = std::fs::read_to_string(BASELINE_META)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(resource_filter::BaselineMeta {
            repo: String::new(),
            branch: String::new(),
            commit: String::new(),
            fetched_at_utc: String::new(),
            file_count: 0,
            license: String::new(),
        });
    let entry = resource_filter::ResourceEntry {
        kind: resource_filter::KIND_PERSONA_CATALOG.to_string(),
        id: CATALOG_ID.to_string(),
        upstream_repo: UPSTREAM_REPO.to_string(),
        upstream_commit: if meta.commit.is_empty() {
            "unknown".to_string()
        } else {
            meta.commit.clone()
        },
        fetched_at_utc: meta.fetched_at_utc.clone(),
        file_count: 0, // assemble_catalog 按实际 tree 重算
        license: UPSTREAM_LICENSE.to_string(),
        tree: Vec::new(),
    };
    let embed_source = format!(
        "bundled snapshot ({} @ {})",
        entry.upstream_commit, meta.fetched_at_utc
    );
    match assemble_catalog(
        files,
        entry,
        built_at,
        "baseline",
        embed_source.clone(),
        license_text,
    ) {
        Ok(c) => (c, embed_source),
        Err(err) => {
            // 基线在仓库里必然存在且可组装——真到这里 = 仓库被破坏，
            // fail-loud（OUT_DIR 产物必须存在是 lib.rs include_str! 的硬前提）。
            panic!(
                "[plugin-resources] 仓库基线快照不可组装（{err}）——基线目录被破坏，请走 scripts/update-persona-baseline.sh 重建"
            );
        }
    }
}

/// FILE/REFRESH 共同出口：tarball 读端 → catalog。指纹 = REFRESH 环境值
/// （commit SHA 或缓存 size+mtime），进 manifest 溯源；`embed_prefix` 区分
/// 两臂溯源行（FILE=pre-downloaded tarball / REFRESH=downloaded tarball）。
/// manifest.source 按设计 §3.2 值集恒 "fetched"（拉取版，两臂同值——脚本
/// 预下载 vs 直拉的区分只留在 embed_source 溯源行）。
fn tarball_catalog<R: Read>(
    reader: &mut R,
    fingerprint: &str,
    built_at: &str,
    embed_prefix: &str,
) -> Result<(String, String), String> {
    let (files, skipped, license_text) = extract_tarball_payload(reader)?;
    let count = files.len();
    let Some(license_text) = license_text else {
        // §3.3 机械子集保留 *.md + LICENSE；上游把 LICENSE 拿掉 = 结构剧变，
        // 与「0 个 md」同待遇走 fail-open 兜基线（基线恒带许可原文）。
        return Err("tarball 内无根级 LICENSE——上游结构剧变，拒绝内嵌无许可产物".to_string());
    };
    let mut embed_source = format!(
        "{embed_prefix} ({fingerprint}, {count} files, {})",
        built_at_date(built_at)
    );
    if skipped > 0 {
        embed_source.push_str(&format!(", {skipped} non-utf8 skipped"));
    }
    let entry = resource_filter::ResourceEntry {
        kind: resource_filter::KIND_PERSONA_CATALOG.to_string(),
        id: CATALOG_ID.to_string(),
        upstream_repo: UPSTREAM_REPO.to_string(),
        upstream_commit: fingerprint.to_string(),
        fetched_at_utc: built_at.to_string(),
        file_count: 0, // assemble_catalog 按实际 tree 重算
        license: UPSTREAM_LICENSE.to_string(),
        tree: Vec::new(),
    };
    let catalog = assemble_catalog(
        files,
        entry,
        built_at,
        "fetched",
        embed_source.clone(),
        license_text,
    )?;
    Ok((catalog, embed_source))
}

/// REFRESH 模式：codeload 直拉 tarball → catalog。
fn fetched_catalog(fingerprint: &str, built_at: &str) -> Result<(String, String), String> {
    let url = tarball_url();
    let resp = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "NemesisBot-plugin-resources-build/",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TOTAL_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(
            DOWNLOAD_CONNECT_TIMEOUT_SECS,
        ))
        .build()
        .map_err(|e| format!("client 构建: {e}"))?
        .get(&url)
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GET {url}: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().map_err(|e| format!("读响应体: {e}"))?;
    let mut reader = std::io::Cursor::new(bytes);
    // embed_source 三形（§4.5）：downloaded tarball / pre-downloaded tarball /
    // bundled snapshot——manifest.source 恒 fetched|baseline，臂区分只在溯源行。
    tarball_catalog(&mut reader, fingerprint, built_at, "downloaded tarball")
}

/// (仓库相对路径, 内容) 集的别名（extract/walk 两出口共用；type_complexity）。
type MdFiles = Vec<(String, String)>;

/// tarball → (仓库相对路径, 内容) 机械子集 + 根级 LICENSE 原文（§3.3：
/// 保留 `*.md` + LICENSE）。非 UTF-8 单文件跳过计数，不毒化整包。
/// 返回 (md files, skipped_count, license_text)。
fn extract_tarball_payload<R: Read>(
    reader: &mut R,
) -> Result<(MdFiles, usize, Option<String>), String> {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);
    let mut files = Vec::new();
    let mut skipped = 0usize;
    let mut license_text = None;
    let entries = archive.entries().map_err(|e| format!("tar 解析: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("tar 条目读取: {e}"))?;
        let raw_path = entry
            .path()
            .map_err(|e| format!("tar 路径读取: {e}"))?
            .to_string_lossy()
            .to_string();
        let Some(rel) = strip_tarball_root(&raw_path) else {
            continue;
        };
        // 根级 LICENSE 原文（大小写不敏感、不带扩展名；LICENSE.md 会走 is_md
        // 进 files——许可义务优先取专用的无后缀 LICENSE）。
        if !rel.contains('/') && rel.eq_ignore_ascii_case("LICENSE") {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| format!("读 {rel}: {e}"))?;
            license_text = String::from_utf8(buf).ok();
            continue;
        }
        if !is_md(&rel) {
            continue;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("读 {rel}: {e}"))?;
        match String::from_utf8(buf) {
            Ok(content) => files.push((rel, content)),
            Err(_) => skipped += 1,
        }
    }
    Ok((files, skipped, license_text))
}

/// 基线 md 子树递归走查（仓库相对路径 = md/ 之下；顺序由消费侧
/// BTreeMap 归一——跨平台产物字节确定）。
fn walk_baseline_md() -> MdFiles {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut MdFiles) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            if p.is_dir() {
                walk(&p, &format!("{prefix}{name}/"), out);
            } else if is_md(&name)
                && let Ok(content) = std::fs::read_to_string(&p)
            {
                out.push((format!("{prefix}{name}"), content));
            }
        }
    }
    let mut out = Vec::new();
    walk(std::path::Path::new(BASELINE_MD_DIR), "", &mut out);
    out
}

/// 当前 UTC 时间（RFC3339 简形 `YYYY-MM-DDTHH:MM:SSZ`）。
fn utc_now_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_epoch_secs(secs)
}

/// RFC3339 简形取日期段（`YYYY-MM-DD`，溯源行用）。
fn built_at_date(built_at: &str) -> &str {
    date_part(built_at)
}
