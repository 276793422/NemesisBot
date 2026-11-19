//! 附加资源插件共享逻辑（单一真相源）：`build.rs`（`#[path]` include，构建期
//! catalog 组装）与 `lib.rs`（运行期内嵌产物解析）双端同源——过滤规则、
//! manifest 结构、catalog 组装与校验只定义一次。
//!
//! 机械裁剪纪律：本 crate 只保留「一切 `*.md`」这一条 glob 规则；SKIP_DIRS /
//! SKIP_FILENAMES / QUICKSTART 等语义过滤**留在宿主消费侧**（nemesis-web
//! persona `is_agent_file`）——快照与在线数据走同一段枚举，规则不复制。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 常量（上游身份 / 路径布局——build.rs 与运行期共用）
// ---------------------------------------------------------------------------

/// 资源种类（manifest resources[].kind；宿主按 kind 对接消费面）。
pub const KIND_PERSONA_CATALOG: &str = "persona-catalog";
/// 人格目录资源 id（上游仓库身份）。
pub const CATALOG_ID: &str = "agency-agents";
/// 上游仓库（GitHub owner/name）。
pub const UPSTREAM_REPO: &str = "msitarzewski/agency-agents";
/// 上游分支。
pub const UPSTREAM_BRANCH: &str = "main";
/// 上游 License（内嵌=再分发，许可原文随插件以 LICENSE-upstream 分发并
/// 嵌进 manifest.license_text——§8 验收「插件产物含上游 LICENSE 原文」）。
pub const UPSTREAM_LICENSE: &str = "MIT";
/// 上游许可原文（crate 根 `LICENSE-upstream`；baseline 臂构建期读入，
/// FILE/REFRESH 臂从 tarball 根提取。缺失 = 拒绝内嵌——分发合规硬前提）。
pub const BASELINE_LICENSE: &str = "LICENSE-upstream";
/// 仓库基线快照目录（git 入库，离线构建可复现）。
pub const BASELINE_DIR: &str = "resources/persona-catalog/agency-agents";
/// 基线 md 子目录。
pub const BASELINE_MD_DIR: &str = "resources/persona-catalog/agency-agents/md";
/// 基线元数据（§3.1 命名 manifest.json——「基线 manifest」；与编译期内嵌的
/// PluginManifest 是两回事：本文件是刷新脚本写入的基线溯源记录，内嵌那个是
/// 构建期组装产物）。
pub const BASELINE_META: &str = "resources/persona-catalog/agency-agents/manifest.json";
/// OUT_DIR 产物文件名（lib.rs include_str! 单一嵌入产物）。
pub const OUT_FILE: &str = "persona_catalog.json";
/// 上游 tarball 直拉地址（codeload 一次全量，不逐文件打 API）。
pub fn tarball_url() -> String {
    format!("https://codeload.github.com/{UPSTREAM_REPO}/tar.gz/refs/heads/{UPSTREAM_BRANCH}")
}

// ---------------------------------------------------------------------------
// manifest（插件内资源清单 = 唯一元数据真相源）
// ---------------------------------------------------------------------------

/// 单资源条目（manifest.resources[] 元素）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ResourceEntry {
    pub kind: String,
    pub id: String,
    pub upstream_repo: String,
    pub upstream_commit: String,
    pub fetched_at_utc: String,
    pub file_count: usize,
    pub license: String,
    /// 路径清单（仓库相对路径，排序去重；语义过滤在宿主消费侧）。
    pub tree: Vec<String>,
}

/// 插件 manifest（构建期生成、编译期内嵌、运行期 host 经 ABI 读取）。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PluginManifest {
    pub abi: u32,
    pub built_at_utc: String,
    /// `fetched`（REFRESH 直拉或 FILE 脚本预下载——两者都是「拉取版」，值集
    /// 按设计 §3.2 钉死 fetched|baseline；脚本预下载的区分在 embed_source）/
    /// `baseline`（仓库基线兜底）。
    pub source: String,
    /// 人类可读溯源行（价目表 NEMESIS_PRICES_EMBED_SOURCE 同构）。
    pub embed_source: String,
    /// 上游许可**原文**（§8 验收：插件产物含 LICENSE 原文——分发合规义务，
    /// 不止 `license: "MIT"` 标签）。
    pub license_text: String,
    pub resources: Vec<ResourceEntry>,
}

/// catalog 单文件。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CatalogFile {
    pub path: String,
    pub content: String,
}

/// OUT_DIR 产物整体形状（manifest + files）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Catalog {
    pub manifest: PluginManifest,
    pub files: Vec<CatalogFile>,
}

/// 基线元数据（基线 manifest.json；刷新脚本写入）。
/// 部分字段（repo/branch/file_count/license）是元数据完整性记录——build 侧
/// 溯源只消费 commit/fetched_at_utc，其余字段随文件存在即可，不读不算错。
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
pub struct BaselineMeta {
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub fetched_at_utc: String,
    #[serde(default)]
    pub file_count: usize,
    #[serde(default)]
    pub license: String,
}

// ---------------------------------------------------------------------------
// 机械裁剪与路径归一
// ---------------------------------------------------------------------------

/// 机械裁剪规则：保留一切 `*.md`（大小写不敏感）。语义过滤不在这里。
pub fn is_md(path: &str) -> bool {
    path.to_lowercase().ends_with(".md")
}

/// tarball 内路径 → 仓库相对路径（剥第一段根目录，如
/// `agency-agents-main/engineering/x.md` → `engineering/x.md`）。
/// 无 `/`（根目录自身）→ None。
pub fn strip_tarball_root(path: &str) -> Option<String> {
    let p = normalize_path(path);
    p.split_once('/').map(|(_, rest)| rest.to_string())
}

/// 路径归一：反斜杠 → 正斜杠、剥 `./` 前缀、剥尾 `/`。空串 → None。
pub fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }
    s.trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// catalog 组装与校验（build.rs 三路径共用同一出口）
// ---------------------------------------------------------------------------

/// 组装 catalog JSON。`files` 为 (仓库相对路径, 内容) 集；机械裁剪已由调用方
/// 完成（或内部再兜一层 is_md）。校验失败 = Err（构建侧据此 fail-open 兜基线）：
/// 至少 1 个文件、路径非空、全为 .md、许可原文非空。
pub fn assemble_catalog(
    mut files: Vec<(String, String)>,
    mut entry: ResourceEntry,
    built_at_utc: &str,
    source: &str,
    embed_source: String,
    license_text: String,
) -> Result<String, String> {
    if license_text.trim().is_empty() {
        return Err("license_text 空（许可原文缺失）——拒绝内嵌（分发合规硬前提）".to_string());
    }
    if files.is_empty() {
        return Err("catalog 空（0 个 .md 文件）——拒绝内嵌坏数据".to_string());
    }
    // 机械裁剪兜底 + 路径归一（调用方已裁时为零开销空转）。
    files.retain(|(p, _)| is_md(p) && !normalize_path(p).is_empty());
    if files.is_empty() {
        return Err("机械裁剪后 catalog 空——拒绝内嵌坏数据".to_string());
    }
    for (p, _) in &files {
        if normalize_path(p) != *p {
            return Err(format!("路径未归一：{p}"));
        }
    }
    // 路径去重（同路径保最后出现）。
    let mut seen = std::collections::BTreeMap::new();
    for (p, c) in files {
        seen.insert(p, c);
    }
    let tree: Vec<String> = seen.keys().cloned().collect();
    entry.kind = KIND_PERSONA_CATALOG.to_string();
    entry.file_count = tree.len();
    entry.tree = tree;
    validate_entry(&entry)?;

    let catalog = Catalog {
        manifest: PluginManifest {
            abi: 1,
            built_at_utc: built_at_utc.to_string(),
            source: source.to_string(),
            embed_source,
            license_text,
            resources: vec![entry],
        },
        files: seen
            .into_iter()
            .map(|(path, content)| CatalogFile { path, content })
            .collect(),
    };
    serde_json::to_string(&catalog).map_err(|e| format!("catalog 序列化失败: {e}"))
}

/// 条目校验：commit 非空、license 非空、tree 与 file_count 一致。
pub fn validate_entry(entry: &ResourceEntry) -> Result<(), String> {
    if entry.upstream_commit.is_empty() {
        return Err("upstream_commit 为空".to_string());
    }
    if entry.license.is_empty() {
        return Err("license 为空".to_string());
    }
    if entry.tree.is_empty() {
        return Err("tree 为空".to_string());
    }
    if entry.file_count != entry.tree.len() {
        return Err(format!(
            "file_count ({}) 与 tree.len ({}) 不一致",
            entry.file_count,
            entry.tree.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// UTC 时间格式化（无 chrono 依赖；build.rs 溯源行与单测共用）
// ---------------------------------------------------------------------------

/// epoch 秒 → `YYYY-MM-DDTHH:MM:SSZ`（纯函数，插件单测钉死）。
pub fn format_epoch_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// RFC3339 简形取日期段（`YYYY-MM-DD`，溯源行 / 前端徽标用）。
pub fn date_part(rfc3339: &str) -> &str {
    rfc3339.split('T').next().unwrap_or(rfc3339)
}

/// epoch 天数 → (年, 月, 日)（Howard Hinnant civil_from_days）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
