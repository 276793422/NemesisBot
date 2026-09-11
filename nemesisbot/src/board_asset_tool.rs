//! Swarm M3（§5.4/G9）：`board_asset` 工具——cluster agent 的资产拉取 /
//! 发布执行者。G9 判据「worker HTTP 拉取 → sha256 校验 → 本地文件可用」
//! 与其反向对称（worker 产物登记 + 签发引用束回贴线程）都落在这里。
//!
//! 自包含设计：只需 workspace 路径。fetch 是纯 HTTP + sha256 校验（不碰
//! store）；publish 自己 open BoardStore（SQLite WAL 多连接安全，与 CLI
//! 同款模式）+ 幂等加载资产密钥（gateway 启动时写同文件）+ 读 gateway
//! bind 后落盘的 `<workspace>/config/asset_node_url.txt`（跨进程一致的
//! 对外基址）。注册面：**仅 cluster agent**（master 本尊有 filesystem 工具
//! 且 gateway 就在本地，主 agent 同理不装）。
//!
//! SSRF 闸：集群互拉本来就是内网地址，默认 `block_private_ips` 会把
//! G9 场景全拦——这里定制宽松 config（放行内网/回环），**云 metadata
//! 端点照拦**（169.254.169.254 与集群网段无关，没有放行理由）。
//! security feature 关掉的裁剪构建没有 guard，跳过校验（该构建本就无
//! 安全管线，语义一致）。

use std::path::{Path, PathBuf};

use nemesis_agent::context::RequestContext;

/// 工具注册名。
pub const TOOL_NAME: &str = "board_asset";

/// publish 单文件上限（1 GiB）。资产走 HTTP 全内存中转（提供方与消费方
/// 都是），超大树形物应走压缩包或外部存储，不进看板资产面。
pub const MAX_PUBLISH_BYTES: u64 = 1024 * 1024 * 1024;

/// board_asset 工具：从提供方节点拉取任务资产，或把本地文件发布为可
/// 分发的看板资产。
pub struct BoardAssetTool {
    workspace: PathBuf,
}

impl BoardAssetTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    fn assets_dir(&self) -> PathBuf {
        nemesis_path::resolve_board_assets_dir_in_workspace(&self.workspace)
    }
}

/// 解析后的工具参数（execute 内部第一步；独立成纯函数便于单测）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AssetArgs {
    /// 拉取：四字段逐字来自 dispatch prompt / 评论里的引用束 JSON。
    Fetch {
        node_url: String,
        asset_ref: String,
        asset_token: String,
        expires_at: i64,
        sha256: String,
    },
    /// 发布：workspace 内文件 → 资产（ref 缺省取文件名）。
    Publish {
        path: String,
        ref_name: Option<String>,
    },
}

/// args JSON → 结构化参数（缺字段 / action 词表外 / 空值一律诚实报错
/// ——args_validator 兜的是 schema 层，语义层这里自己守）。
pub(crate) fn parse_asset_args(args: &str) -> Result<AssetArgs, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {e}"))?;
    match v.get("action").and_then(|x| x.as_str()).unwrap_or("") {
        "fetch" => {
            let node_url = str_field(&v, "node_url")?;
            let asset_ref = str_field(&v, "asset_ref")?;
            let asset_token = str_field(&v, "asset_token")?;
            let expires_at = v
                .get("expires_at")
                .and_then(|x| x.as_i64())
                .ok_or("missing or non-integer expires_at")?;
            let sha256 = str_field(&v, "sha256")?;
            Ok(AssetArgs::Fetch {
                node_url,
                asset_ref,
                asset_token,
                expires_at,
                sha256,
            })
        }
        "publish" => {
            let path = str_field(&v, "path")?;
            let ref_name = v
                .get("ref_name")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Ok(AssetArgs::Publish { path, ref_name })
        }
        other => Err(format!(
            "action must be \"fetch\" or \"publish\", got {other:?}"
        )),
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> Result<String, String> {
    let s = v
        .get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if s.is_empty() {
        return Err(format!("missing or empty {key}"));
    }
    Ok(s)
}

/// fetch 参数前置校验（不碰网络）：ref 过白名单、sha256 是 64 hex（大小写
/// 不敏感，归一为小写）、拼出最终下载 URL。返回 (url, 归一 sha256)。
pub(crate) fn validate_fetch_params(
    node_url: &str,
    asset_ref: &str,
    asset_token: &str,
    expires_at: i64,
    sha256: &str,
) -> Result<(String, String), String> {
    nemesis_board::sanitize_asset_ref(asset_ref)?;
    let sha = sha256.trim().to_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "sha256 must be 64 hex chars, got {} chars",
            sha256.trim().len()
        ));
    }
    let base = node_url.trim_end_matches('/');
    let url = format!(
        "{base}/api/board/asset/{asset_ref}?asset_token={asset_token}&expires_at={expires_at}"
    );
    Ok((url, sha))
}

/// fetch 用的 SSRF 配置：放行内网/回环（集群互拉的本态），metadata 照拦。
#[cfg(feature = "security")]
fn fetch_ssrf_guard() -> nemesis_security::ssrf::Guard {
    nemesis_security::ssrf::Guard::new(nemesis_security::ssrf::SsrfConfig {
        block_private_ips: false,
        block_localhost: false,
        block_metadata: true,
        ..nemesis_security::ssrf::SsrfConfig::default()
    })
    .expect("ssrf config with no custom cidrs always parses")
}

/// publish 文件大小守卫（纯函数便于单测）。
pub(crate) fn validate_publish_size(size: u64) -> Result<(), String> {
    if size > MAX_PUBLISH_BYTES {
        return Err(format!(
            "file too large for board asset: {size} bytes (max {MAX_PUBLISH_BYTES})"
        ));
    }
    Ok(())
}

impl BoardAssetTool {
    /// fetch：SSRF 闸（如启用）→ 下载到 `<assets>/{ref}.part` → sha256
    /// 校验 → 原子改名落定。任何失败都清掉 part 文件，不留半截内容。
    async fn do_fetch(
        &self,
        node_url: &str,
        asset_ref: &str,
        asset_token: &str,
        expires_at: i64,
        sha256: &str,
    ) -> Result<String, String> {
        let (url, expected_sha) =
            validate_fetch_params(node_url, asset_ref, asset_token, expires_at, sha256)?;

        #[cfg(feature = "security")]
        fetch_ssrf_guard()
            .validate_url(&url)
            .map_err(|e| format!("url blocked by ssrf guard: {e}"))?;
        #[cfg(not(feature = "security"))]
        let _ = &url;

        let dest = self.assets_dir().join(format!("{asset_ref}.part"));
        let pool = nemesis_http_pool::pool::shared_pool();
        if let Err(e) = pool.download_file(&url, &dest.to_string_lossy()).await {
            let _ = std::fs::remove_file(&dest);
            return Err(format!("download failed: {e}"));
        }

        let actual_sha = nemesis_board::sha256_file(&dest)?;
        if actual_sha != expected_sha {
            let _ = std::fs::remove_file(&dest);
            return Err(format!(
                "sha256 mismatch: expected {expected_sha}, got {actual_sha} — \
                 file discarded; request a fresh reference if the source changed"
            ));
        }

        let final_path = self.assets_dir().join(asset_ref);
        std::fs::rename(&dest, &final_path)
            .map_err(|e| format!("finalize {}: {e}", final_path.display()))?;
        let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
        Ok(format!(
            "Asset `{asset_ref}` downloaded and sha256-verified ({size} bytes) → {}",
            final_path.display()
        ))
    }

    /// publish：workspace 边界自查（本工具是把文件分发给其他节点的出口，
    /// 自己守边界，不依赖外层分类）→ 拷贝进 assets → 登记索引 → 签发
    /// 引用束。origin_issue 留空（发布时未必知道归属；dispatch 侧登记
    /// 才带 issue 绑定）。
    async fn do_publish(&self, path: &str, ref_name: Option<&str>) -> Result<String, String> {
        let source = self.resolve_workspace_path(path)?;
        let meta =
            std::fs::metadata(&source).map_err(|e| format!("source {}: {e}", source.display()))?;
        if !meta.is_file() {
            return Err(format!("source {} is not a regular file", source.display()));
        }
        validate_publish_size(meta.len())?;

        let ref_name = match ref_name {
            Some(r) => r.to_string(),
            None => source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string(),
        };
        nemesis_board::sanitize_asset_ref(&ref_name)?;

        let assets_dir = self.assets_dir();
        std::fs::create_dir_all(&assets_dir)
            .map_err(|e| format!("mkdir {}: {e}", assets_dir.display()))?;
        let dest = assets_dir.join(&ref_name);
        std::fs::copy(&source, &dest).map_err(|e| format!("copy to {}: {e}", dest.display()))?;
        let sha = nemesis_board::sha256_file(&dest)?;
        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);

        let db_path = self.workspace.join("board").join("board.db");
        let store = nemesis_board::BoardStore::open(&db_path, "NB")?;
        store.register_asset(nemesis_board::NewAsset {
            ref_name: ref_name.clone(),
            origin_issue: None,
            sha256: sha.clone(),
            size: size as i64,
        })?;

        let secret = nemesis_board::load_or_create_secret(
            &nemesis_path::resolve_asset_secret_path_in_workspace(&self.workspace),
        )?;
        let url_path = nemesis_path::resolve_asset_node_url_path_in_workspace(&self.workspace);
        let node_url = std::fs::read_to_string(&url_path)
            .map_err(|e| {
                format!(
                    "node url file {} unreadable ({e}) — the local gateway must be \
                     running once with board+cluster before assets can be served",
                    url_path.display()
                )
            })?
            .trim()
            .to_string();
        if node_url.is_empty() {
            return Err(format!("node url file {} is empty", url_path.display()));
        }

        let bundle = nemesis_board::issue_asset_bundle(
            &secret,
            &ref_name,
            &sha,
            size as i64,
            &node_url,
            nemesis_board::DEFAULT_TOKEN_TTL_SECS,
        );
        let bundle_json =
            serde_json::to_string(&bundle).map_err(|e| format!("serialize bundle: {e}"))?;
        Ok(format!(
            "Asset `{ref_name}` published ({size} bytes, sha256 {sha}).\n\
             Reference bundle (valid {}s):\n{bundle_json}\n\
             Paste this bundle into the board thread (board_discuss) or the \
             delivery comment so the requester can download it.",
            nemesis_board::DEFAULT_TOKEN_TTL_SECS
        ))
    }

    /// publish 源路径的 workspace 边界解析：相对路径锚在 workspace；
    /// 绝对路径 canonicalize 后必须在 workspace 内（8.3 短名/大小写安全
    /// 比较，见 nemesis-path::canonicalize_for_compare）。
    fn resolve_workspace_path(&self, raw: &str) -> Result<PathBuf, String> {
        let p = Path::new(raw);
        let candidate = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace.join(p)
        };
        let ws = nemesis_path::paths::canonicalize_for_compare(&self.workspace);
        let resolved = nemesis_path::paths::canonicalize_for_compare(&candidate);
        if !resolved.starts_with(&ws) {
            return Err(format!(
                "path {} is outside the workspace — board_asset.publish only \
                 accepts files inside the workspace",
                candidate.display()
            ));
        }
        Ok(candidate)
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for BoardAssetTool {
    fn description(&self) -> String {
        "Fetch a task asset from the providing node (HTTP download with \
         HMAC token + sha256 verification), or publish a local workspace \
         file as a board asset that other nodes can download. For fetch, \
         pass the fields of the reference bundle verbatim. For publish, \
         the result includes a reference bundle to paste into the board \
         thread so the requester can download your file."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["fetch", "publish"],
                    "description": "fetch = download a referenced asset; publish = share a local file."
                },
                "node_url": {
                    "type": "string",
                    "description": "fetch: provider base url from the bundle (node_url field)."
                },
                "asset_ref": {
                    "type": "string",
                    "description": "fetch: asset reference name from the bundle (asset_ref field)."
                },
                "asset_token": {
                    "type": "string",
                    "description": "fetch: HMAC token from the bundle (asset_token field)."
                },
                "expires_at": {
                    "type": "integer",
                    "description": "fetch: expiry unix seconds from the bundle (expires_at field)."
                },
                "sha256": {
                    "type": "string",
                    "description": "fetch: expected sha256 (64 hex chars) to verify the download."
                },
                "path": {
                    "type": "string",
                    "description": "publish: file path inside the workspace (relative or absolute)."
                },
                "ref_name": {
                    "type": "string",
                    "description": "publish: optional asset reference name (defaults to the file name)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        match parse_asset_args(args)? {
            AssetArgs::Fetch {
                node_url,
                asset_ref,
                asset_token,
                expires_at,
                sha256,
            } => {
                self.do_fetch(&node_url, &asset_ref, &asset_token, expires_at, &sha256)
                    .await
            }
            AssetArgs::Publish { path, ref_name } => {
                self.do_publish(&path, ref_name.as_deref()).await
            }
        }
    }
}

#[cfg(test)]
mod tests;
