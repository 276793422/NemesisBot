//! connector 远程清单源（D6.2，2026-10-15）。
//!
//! 最小生态形态：`add-source`（git clone/拉取到本地缓存）→ `search`
//! （源内清单扫描）→ `install`（装配漏斗裁剪版：验签 → parse → 落位）。
//! 源注册表落 `<workspace>/config/connector_sources.json`，克隆缓存落
//! `<workspace>/cache/connector_sources/<name>/`。不做商店 UI / 评分 /
//! 自动更新（计划 §六 6.2 拍板）。
//!
//! 装配漏斗（对齐 plugin install 语义）：
//! - 验签失败（篡改/调换/吊销）→ loud 拒装；
//! - 未签名 → 需显式 `--allow-unsigned`；
//! - 验签过 + verified → 装（徽标透出）；
//! - 验签过 + community / 不在库 → 装 + WARN 留痕。
//!
//! 落位后盘上真相回读（目标目录 assess + registry find——拷贝成功但
//! 签名/清单失效不得报成功）。

use std::path::{Path, PathBuf};

use anyhow::Result;
use nemesis_connectors::manifest::{ValidatedConnector, parse_connector_manifest};
use nemesis_connectors::registry::MANIFEST_FILE;
use nemesis_connectors::signing::{
    ConnectorTrust, SIG_FILE, assess_connector_signature, workspace_trust_store,
};

// ============ 源注册表（config/connector_sources.json） ============

/// 单个远程源条目（skills registry.add_source 同模式 name/url/branch）。
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ConnectorSource {
    pub name: String,
    pub url: String,
    #[serde(default = "default_branch")]
    pub branch: String,
}

fn default_branch() -> String {
    "main".to_string()
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Default)]
struct SourcesFile {
    #[serde(default)]
    sources: Vec<ConnectorSource>,
}

pub fn sources_path(workspace: &Path) -> PathBuf {
    workspace.join("config").join("connector_sources.json")
}

pub fn cache_dir(workspace: &Path, name: &str) -> PathBuf {
    workspace.join("cache").join("connector_sources").join(name)
}

fn load_sources(workspace: &Path) -> Vec<ConnectorSource> {
    let Ok(text) = std::fs::read_to_string(sources_path(workspace)) else {
        return Vec::new(); // 缺文件 = 空源表（常态）
    };
    // 显式标注 SourcesFile——裸 unwrap_or_default 会被返回类型推断成
    // Vec<ConnectorSource>，对象形 JSON 按 Vec 解恒失败 = 静默空表。
    match serde_json::from_str::<SourcesFile>(&text) {
        Ok(f) => f.sources,
        Err(e) => {
            tracing::warn!(
                "[connector.sources] {} 解析失败（按空源表处理）: {e}",
                sources_path(workspace).display()
            );
            Vec::new()
        }
    }
}

fn save_sources(workspace: &Path, sources: &[ConnectorSource]) -> Result<()> {
    let path = sources_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&SourcesFile {
        sources: sources.to_vec(),
    })?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// 从 git URL 推源名（repo 名去 `.git`；源名做 id 形态约束）。
pub fn derive_source_name(url: &str) -> Result<String> {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or("");
    let name = last.strip_suffix(".git").unwrap_or(last);
    let name = name.to_lowercase();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        anyhow::bail!("无法从 URL 推源名 `{name}`（用 --name 显式指定）");
    }
    Ok(name)
}

/// 注册远程源并克隆/拉取到本地缓存。同名已存在 = 拒（改 URL 先删再加）。
pub fn add_source(
    workspace: &Path,
    url: &str,
    name: Option<&str>,
    branch: Option<&str>,
) -> Result<ConnectorSource> {
    let name = match name {
        Some(n) => {
            let n = n.to_lowercase();
            if n.is_empty()
                || !n
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                anyhow::bail!("源名只允许小写字母/数字/`-`/`_`：{n}");
            }
            n
        }
        None => derive_source_name(url)?,
    };
    let branch = branch.unwrap_or("main").to_string();
    let mut sources = load_sources(workspace);
    if let Some(existing) = sources.iter().find(|s| s.name == name) {
        if existing.url != url {
            anyhow::bail!(
                "源 `{}` 已存在且 URL 不同（{}）——换仓库请用 --name 区分，或先 remove-source",
                name,
                existing.url
            );
        }
        // 同名同 URL = 拉取更新语义（clone_or_pull 走 fetch+reset）。
        clone_or_pull(&cache_dir(workspace, &name), url, &existing.branch)?;
        return Ok(existing.clone());
    }
    let source = ConnectorSource {
        name: name.clone(),
        url: url.to_string(),
        branch: branch.clone(),
    };
    let cache = cache_dir(workspace, &name);
    clone_or_pull(&cache, url, &branch)?;
    sources.push(source.clone());
    save_sources(workspace, &sources)?;
    Ok(source)
}

/// 移除源注册（缓存目录保留——落位过的 connector 不受影响；诚实注明）。
pub fn remove_source(workspace: &Path, name: &str) -> Result<bool> {
    let mut sources = load_sources(workspace);
    let before = sources.len();
    sources.retain(|s| s.name != name);
    if sources.len() == before {
        return Ok(false);
    }
    save_sources(workspace, &sources)?;
    Ok(true)
}

pub fn list_sources(workspace: &Path) -> Vec<ConnectorSource> {
    load_sources(workspace)
}

// ============ git 克隆/拉取（shell git，CREATE_NO_WINDOW 纪律） ============

fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("git 启动失败: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr.lines().rev().take(4).collect::<Vec<_>>().join("; ");
        anyhow::bail!("git {} 失败: {}", args.first().unwrap_or(&""), tail);
    }
    Ok(())
}

fn clone_or_pull(cache: &Path, url: &str, branch: &str) -> Result<()> {
    if cache.join(".git").exists() {
        // 已缓存 → 拉取更新（fetch + 硬重置到远端分支头；FETCH_HEAD 不依赖
        // 本地分支 tracking 配置）。
        run_git(&["fetch", "--depth", "1", "origin", branch], Some(cache))?;
        run_git(&["reset", "--hard", "FETCH_HEAD"], Some(cache))?;
    } else {
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // clone 带显式目标目录（落点名与源名对齐，不取 URL 推断名）。
        // --config core.autocrlf=false 落进新仓库本地配置（后续 fetch/reset
        // 沿用）：Windows 全局 autocrlf=true 会在 checkout 时写 CRLF，破坏
        // 签名载荷字节（签名按 LF 原文算）且清单解析拒 CRLF → 扫描全 corrupt。
        let cache_str = cache.to_string_lossy().into_owned();
        run_git(
            &[
                "clone",
                "--depth",
                "1",
                "--branch",
                branch,
                "--config",
                "core.autocrlf=false",
                url,
                &cache_str,
            ],
            None,
        )?;
    }
    Ok(())
}

// ============ 源内清单扫描 ============

/// 源内扫描条目（corrupt 诚实入列，registry 同纪律）。
#[derive(Debug, Clone)]
pub struct SourceHit {
    pub source: String,
    pub dir: PathBuf,
    /// 相对源缓存根的目录（展示定位用）。
    pub rel: String,
    pub connector: Option<ValidatedConnector>,
    pub error: Option<String>,
    pub trust: ConnectorTrust,
}

/// 递归收集目录下的 connector.toml（跳 .git；深度上限防异常深树）。
fn find_manifest_dirs(root: &Path, out: &mut Vec<PathBuf>) {
    const MAX_DEPTH: usize = 8;
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        // read_dir 顺序是平台实现细节 → 显式排序（CLAUDE.md 纪律）。
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.join(MANIFEST_FILE).exists() {
                out.push(p.clone());
            }
            walk(&p, depth + 1, out);
        }
    }
    if root.join(MANIFEST_FILE).exists() {
        out.push(root.to_path_buf());
    }
    walk(root, 1, out);
}

/// 扫描单个源缓存（验签走 workspace 共享信任库）。
pub fn scan_source(workspace: &Path, source_name: &str) -> Result<Vec<SourceHit>> {
    let root = cache_dir(workspace, source_name);
    if !root.join(".git").exists() {
        anyhow::bail!("源 `{source_name}` 缓存不存在（先 connector add-source，或缓存被删后重跑）");
    }
    let store = workspace_trust_store(workspace);
    let mut dirs = Vec::new();
    find_manifest_dirs(&root, &mut dirs);
    let mut hits = Vec::new();
    for dir in dirs {
        let rel = dir
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let text = match std::fs::read_to_string(dir.join(MANIFEST_FILE)) {
            Ok(t) => t,
            Err(e) => {
                hits.push(SourceHit {
                    source: source_name.to_string(),
                    rel: rel.clone(),
                    dir,
                    connector: None,
                    error: Some(format!("读取失败: {e}")),
                    trust: ConnectorTrust::Unsigned,
                });
                continue;
            }
        };
        match parse_connector_manifest(&text) {
            Ok(c) => match assess_connector_signature(&dir, &store) {
                Err(e) => hits.push(SourceHit {
                    source: source_name.to_string(),
                    rel: rel.clone(),
                    dir,
                    connector: None,
                    error: Some(e),
                    trust: ConnectorTrust::Unsigned,
                }),
                Ok(a) => hits.push(SourceHit {
                    source: source_name.to_string(),
                    rel,
                    dir,
                    connector: Some(c),
                    error: None,
                    trust: a.trust,
                }),
            },
            Err(e) => hits.push(SourceHit {
                source: source_name.to_string(),
                rel,
                dir,
                connector: None,
                error: Some(e),
                trust: ConnectorTrust::Unsigned,
            }),
        }
    }
    Ok(hits)
}

/// 意图检索（registry::search_connectors 同词元逻辑；空 query = 列全部）。
pub fn search_sources(
    workspace: &Path,
    query: &str,
    source_filter: Option<&str>,
) -> Result<Vec<SourceHit>> {
    let tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let sources: Vec<String> = match source_filter {
        Some(s) => vec![s.to_string()],
        None => list_sources(workspace)
            .into_iter()
            .map(|s| s.name)
            .collect(),
    };
    let mut hits = Vec::new();
    for source in sources {
        for hit in scan_source(workspace, &source)? {
            let Some(c) = &hit.connector else {
                continue; // 检索面只暴露可解析条目（corrupt 在 scan 全量可见）
            };
            let matched = tokens.is_empty()
                || tokens.iter().any(|t| {
                    c.id.to_lowercase().contains(t)
                        || c.name.to_lowercase().contains(t)
                        || c.description
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(t)
                        || c.ops.iter().any(|o| {
                            o.id.to_lowercase().contains(t)
                                || o.intent.iter().any(|i| i.to_lowercase().contains(t))
                        })
                });
            if matched {
                hits.push(hit);
            }
        }
    }
    Ok(hits)
}

// ============ 装配漏斗（裁剪版） ============

/// install 结果（人话打印 + tests 断言两用）。
#[derive(Debug)]
pub struct InstallReport {
    pub id: String,
    pub trust: ConnectorTrust,
    pub target_dir: PathBuf,
    pub copied: Vec<String>,
    /// 非致命注记（community WARN 等）。
    pub warnings: Vec<String>,
}

/// 从源装一个 connector 进 workspace 注册表。
///
/// 漏斗：定位 → 验签（篡改/吊销 loud 拒；unsigned 需 `--allow-unsigned`）→
/// parse 校验 → 目标冲突拒 → 只拷清单 + 签名文件（连接器是声明式清单，
/// 无代码载荷——这就是裁剪）→ 落位后盘上真相回读。
pub fn install_connector(
    workspace: &Path,
    source_name: &str,
    connector_id: &str,
    allow_unsigned: bool,
) -> Result<InstallReport> {
    let hits = scan_source(workspace, source_name)?;
    let hit = hits
        .iter()
        .find(|h| h.connector.as_ref().is_some_and(|c| c.id == connector_id))
        .ok_or_else(|| {
            let scene: Vec<String> = hits
                .iter()
                .map(|h| match (&h.connector, &h.error) {
                    (Some(c), _) => format!("{}(id={},trust={})", h.rel, c.id, h.trust.as_str()),
                    (None, Some(e)) => format!("{}(corrupt: {e})", h.rel),
                    (None, None) => format!("{}(empty)", h.rel),
                })
                .collect();
            anyhow::anyhow!(
                "源 `{source_name}` 里找不到 connector `{connector_id}`（已扫描: [{}]; connector search 查看）",
                scene.join(", ")
            )
        })?;
    let connector = hit.connector.clone().expect("find 已保证");

    // 验签闸（corrupt 形态在 find 上面已经被过滤掉——能走到这里说明
    // 验签通过或未签名）。
    let mut warnings = Vec::new();
    match hit.trust {
        ConnectorTrust::Unsigned => {
            if !allow_unsigned {
                anyhow::bail!(
                    "`{connector_id}` 未签名——未签名清单可被中途篡改。确认来源可信后用 \
                     `connector install {source_name}/{connector_id} --allow-unsigned` 显式放行",
                );
            }
            warnings.push("未签名（--allow-unsigned 放行）——完整性无密码学保证".to_string());
        }
        ConnectorTrust::Community => {
            warnings.push(
                "签名者不在信任库（community）——`connector trust-list` 查看，\
                 信任后重装可得 verified 徽标"
                    .to_string(),
            );
        }
        ConnectorTrust::Verified => {}
    }

    // 目标冲突拒（不覆盖已有 connector——升级走手动删+装）。
    let target = workspace
        .join(nemesis_connectors::registry::CONNECTORS_DIR)
        .join(connector_id);
    if target.exists() {
        anyhow::bail!(
            "workspace 已有 connector `{connector_id}`（{}）——先手动删除再装",
            target.display()
        );
    }

    // 落位：只拷 connector.toml + connector.toml.sig（签名跟清单走）。
    std::fs::create_dir_all(&target)?;
    let mut copied = Vec::new();
    std::fs::write(
        target.join(MANIFEST_FILE),
        std::fs::read_to_string(hit.dir.join(MANIFEST_FILE))?,
    )?;
    copied.push(MANIFEST_FILE.to_string());
    if hit.dir.join(SIG_FILE).exists() {
        std::fs::copy(hit.dir.join(SIG_FILE), target.join(SIG_FILE))?;
        copied.push(SIG_FILE.to_string());
    }

    // 盘上真相回读：目标目录 assess + registry find（parse + enabled）。
    let store = workspace_trust_store(workspace);
    let reassessed = assess_connector_signature(&target, &store)
        .map_err(|e| anyhow::anyhow!("落位后验签失败（源缓存的清单与签名在拷贝中失配？）: {e}"))?;
    if reassessed.trust != hit.trust {
        anyhow::bail!(
            "落位后信任徽标漂移（{} → {}）——回滚安装",
            hit.trust.as_str(),
            reassessed.trust.as_str()
        );
    }
    let found = nemesis_connectors::registry::find_connector(workspace, connector_id)
        .map_err(|e| anyhow::anyhow!("落位后注册表查找失败: {e}"))?;
    if found.id != connector.id {
        anyhow::bail!("落位后 id 不符（{} ≠ {}）", found.id, connector.id);
    }

    Ok(InstallReport {
        id: connector_id.to_string(),
        trust: hit.trust,
        target_dir: target,
        copied,
        warnings,
    })
}

#[cfg(test)]
mod tests;
