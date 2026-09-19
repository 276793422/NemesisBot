//! Swarm M3（§5.4/G9）：`board_asset` 工具——cluster agent 的资产拉取 /
//! 发布执行者。G9 判据「worker HTTP 拉取 → sha256 校验 → 本地文件可用」
//! 与其反向对称（worker 产物登记 + 签发引用束回贴线程）都落在这里。
//!
//! 自包含设计：只需 workspace 路径（集群句柄可选注入，见下）。fetch 是
//! 两路兜底：HTTP 直连（bundle.node_url）→ 连接级失败时集群 RPC 分块
//! 拉取（board_asset_rpc 的 asset.meta/asset.chunk，需 bundle.node_id +
//! 集群句柄）；publish 自己 open BoardStore（SQLite WAL 多连接安全，与
//! CLI 同款模式）+ 幂等加载资产密钥（gateway 启动时写同文件）+ 读
//! gateway bind 后落盘的 `<workspace>/config/asset_node_url.txt`（跨进程
//! 一致的对外基址）。注册面：**仅 cluster agent**（master 本尊有
//! filesystem 工具且 gateway 就在本地，主 agent 同理不装）。
//!
//! SSRF 闸：集群互拉本来就是内网地址，默认 `block_private_ips` 会把
//! G9 场景全拦——这里定制宽松 config（放行内网/回环），**云 metadata
//! 端点照拦**（169.254.169.254 与集群网段无关，没有放行理由）。
//! security feature 关掉的裁剪构建没有 guard，跳过校验（该构建本就无
//! 安全管线，语义一致）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nemesis_agent::context::RequestContext;
use nemesis_cluster::cluster::Cluster;

/// 工具注册名。
pub const TOOL_NAME: &str = "board_asset";

/// publish 单文件上限（1 GiB）。资产走 HTTP 全内存中转（提供方与消费方
/// 都是），超大树形物应走压缩包或外部存储，不进看板资产面。
pub const MAX_PUBLISH_BYTES: u64 = 1024 * 1024 * 1024;

/// RPC 兜底单块请求大小（服务端 asset.chunk 还有 1MiB 钳制，此处更小
/// ——块小 → 单次 RPC 失败重试代价小，16MB 帧上限下富余极大）。
pub const RPC_CHUNK_BYTES: u64 = 256 * 1024;

/// RPC 兜底整包上限（64 MiB）。RPC 是**兜底通路**不是主干：大资产应走
/// 同网段 HTTP（transfer 级速度）；跨网段真要搬大文件属于部署问题，
/// 诚实报错好过无声吞下（内存峰值 + 长时间占 RPC 帧）。
pub const MAX_RPC_FETCH_BYTES: u64 = 64 * 1024 * 1024;

/// asset.meta 超时（一次验签 + 查表 + stat，10s 富余）。
const ASSET_META_TIMEOUT: Duration = Duration::from_secs(10);
/// asset.chunk 超时（256KB 读盘 + base64 + 帧，30s 富余）。
const ASSET_CHUNK_TIMEOUT: Duration = Duration::from_secs(30);

/// board_asset 工具：从提供方节点拉取任务资产，或把本地文件发布为可
/// 分发的看板资产。
pub struct BoardAssetTool {
    workspace: PathBuf,
    /// 集群句柄（HTTP 直连不可达时的 RPC 分块兜底通路）。None = 仅 HTTP
    /// （旧装配形态 / 测试）；装配点 agent_factory 注入 cluster agent 的
    /// Arc<Cluster>。
    cluster: Option<Arc<Cluster>>,
}

impl BoardAssetTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            cluster: None,
        }
    }

    /// 注入集群句柄（HTTP 直连连接级失败时的 RPC 兜底）。
    pub fn with_cluster(mut self, cluster: Arc<Cluster>) -> Self {
        self.cluster = Some(cluster);
        self
    }

    fn assets_dir(&self) -> PathBuf {
        nemesis_path::resolve_board_assets_dir_in_workspace(&self.workspace)
    }
}

/// 解析后的工具参数（execute 内部第一步；独立成纯函数便于单测）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AssetArgs {
    /// 拉取：四字段逐字来自 dispatch prompt / 评论里的引用束 JSON。
    /// node_id 可选（新 bundle 带——HTTP 不可达时走集群 RPC 兜底；
    /// 旧 bundle 没有 → 仅 HTTP 语义）。
    Fetch {
        node_url: String,
        asset_ref: String,
        asset_token: String,
        expires_at: i64,
        sha256: String,
        node_id: Option<String>,
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
            // 可选：bundle 的 node_id 字段（RPC 兜底寻址）。空串视同缺省。
            let node_id = v
                .get("node_id")
                .and_then(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Ok(AssetArgs::Fetch {
                node_url,
                asset_ref,
                asset_token,
                expires_at,
                sha256,
                node_id,
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

/// 读本节点集群 node_id（gateway bind 后与 node url 同拍落盘；读不到 =
/// 空串 = bundle 不带该字段，消费方仅 HTTP 通路——非集群/旧网关形态的
/// 诚实退化，不是错误）。
pub(crate) fn read_asset_node_id(workspace: &Path) -> String {
    std::fs::read_to_string(nemesis_path::resolve_asset_node_id_path_in_workspace(
        workspace,
    ))
    .map(|s| s.trim().to_string())
    .unwrap_or_default()
}

/// download_file 错误分类（纯函数便于单测）：nemesis-http-pool 的
/// download_file 中 reqwest send 阶段失败（DNS 解析 / 连接拒绝 / 连接
/// 超时——连接级，换通路才有意义）统一前缀 `download request: `；HTTP
/// 状态错是 `HTTP xxx`（403/404 = 通路在、凭据/实体有问题）；读体/写盘
/// 是本地 IO（`read body`/`write`/`mkdir`）。**只有连接级才触发 RPC
/// 兜底**——业务错兜底是噪音（token 过期换个通路一样过期）。
fn is_connection_level_error(e: &str) -> bool {
    e.starts_with("download request: ")
}

impl BoardAssetTool {
    /// fetch：SSRF 闸（如启用）→ HTTP 直连下载 → 连接级失败时集群 RPC
    /// 分块兜底 → sha256 校验 → 原子改名落定。任何失败都清掉 part 文件，
    /// 不留半截内容。
    async fn do_fetch(
        &self,
        node_url: &str,
        asset_ref: &str,
        asset_token: &str,
        expires_at: i64,
        sha256: &str,
        node_id: Option<&str>,
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
        match pool.download_file(&url, &dest.to_string_lossy()).await {
            Ok(()) => {
                let (final_path, size) = self.finalize_download(&dest, asset_ref, &expected_sha)?;
                Ok(format!(
                    "Asset `{asset_ref}` downloaded and sha256-verified ({size} bytes) → {}",
                    final_path.display()
                ))
            }
            Err(e) => {
                if !is_connection_level_error(&e) {
                    // HTTP 业务错 / 本地 IO 错：通路没问题，换通路无意义。
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!("download failed: {e}"));
                }
                // 连接级失败 → 集群 RPC 兜底（需要集群句柄 + bundle 带来
                // 的提供方 node_id，两者缺一诚实报缺）。
                let Some(cluster) = self.cluster.as_ref() else {
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!(
                        "download failed: {e} — provider unreachable over HTTP, \
                         and this node has no cluster connection for RPC fallback"
                    ));
                };
                let Some(node_id) = node_id.filter(|s| !s.is_empty()) else {
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!(
                        "download failed: {e} — provider unreachable over HTTP, \
                         and the reference bundle carries no node_id (legacy \
                         bundle), so cluster RPC fallback is unavailable"
                    ));
                };
                match self
                    .fetch_via_rpc(
                        cluster,
                        node_id,
                        asset_ref,
                        asset_token,
                        expires_at,
                        &expected_sha,
                        &dest,
                    )
                    .await
                {
                    Ok((final_path, size)) => Ok(format!(
                        "Asset `{asset_ref}` fetched via cluster RPC from `{node_id}` \
                         and sha256-verified ({size} bytes) → {}",
                        final_path.display()
                    )),
                    Err(rpc_err) => {
                        let _ = std::fs::remove_file(&dest);
                        Err(format!(
                            "download failed: {e}; cluster RPC fallback via \
                             `{node_id}` also failed: {rpc_err}"
                        ))
                    }
                }
            }
        }
    }

    /// 落定共享腿（HTTP 与 RPC 两路共用）：sha256 校验 → 原子改名。
    /// 校验不过删 part 返回 Err（调用方无需重复清理）。
    fn finalize_download(
        &self,
        dest: &Path,
        asset_ref: &str,
        expected_sha: &str,
    ) -> Result<(PathBuf, u64), String> {
        let actual_sha = nemesis_board::sha256_file(dest)?;
        if actual_sha != expected_sha {
            let _ = std::fs::remove_file(dest);
            return Err(format!(
                "sha256 mismatch: expected {expected_sha}, got {actual_sha} — \
                 file discarded; request a fresh reference if the source changed"
            ));
        }
        let final_path = self.assets_dir().join(asset_ref);
        std::fs::rename(dest, &final_path)
            .map_err(|e| format!("finalize {}: {e}", final_path.display()))?;
        let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
        Ok((final_path, size))
    }

    /// RPC 兜底腿：asset.meta（对表登记 sha256 与 bundle 比对 + 64MiB
    /// 上限预检）→ asset.chunk 256KB 循环拉取 → 拼包 sha256 校验 → 写
    /// part（finalize_download 做校验改名）。数据全内存拼装——64MiB
    /// 上限即内存上界（HTTP 路径 bytes() 同为全内存，量级一致）。
    #[allow(clippy::too_many_arguments)]
    async fn fetch_via_rpc(
        &self,
        cluster: &Arc<Cluster>,
        node_id: &str,
        asset_ref: &str,
        asset_token: &str,
        expires_at: i64,
        expected_sha: &str,
        dest: &Path,
    ) -> Result<(PathBuf, u64), String> {
        let creds = serde_json::json!({
            "asset_ref": asset_ref,
            "asset_token": asset_token,
            "expires_at": expires_at,
        });

        // 1) meta：提供方文件漂移检测（登记 sha ≠ bundle sha = 源已变，
        //    照搬会拿到坏实体——直接拒绝，提示要新引用）。
        let raw = cluster
            .call_with_context_async(
                node_id,
                crate::board_asset_rpc::ACTION_ASSET_META,
                creds.clone(),
                ASSET_META_TIMEOUT,
            )
            .await?;
        let meta: serde_json::Value = serde_json::from_slice(&raw)
            .map_err(|e| format!("asset.meta response malformed: {e}"))?;
        let provider_sha = meta
            .get("sha256")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if provider_sha != expected_sha {
            return Err(format!(
                "provider content sha256 ({provider_sha}) does not match the \
                 reference bundle ({expected_sha}) — file changed since the \
                 bundle was issued; request a fresh reference"
            ));
        }
        let size = meta
            .get("size")
            .and_then(|v| v.as_u64())
            .ok_or("asset.meta response missing size")?;
        if size > MAX_RPC_FETCH_BYTES {
            return Err(format!(
                "asset too large for RPC fallback: {size} bytes \
                 (max {MAX_RPC_FETCH_BYTES}) — fetch over HTTP from the same \
                 network segment instead"
            ));
        }

        // 2) 分块拉取（256KB 步进；服务端另有 1MiB 钳制兜底）。
        let mut buf: Vec<u8> = Vec::with_capacity(size as usize);
        let mut offset: u64 = 0;
        while offset < size {
            let len = RPC_CHUNK_BYTES.min(size - offset);
            let raw = cluster
                .call_with_context_async(
                    node_id,
                    crate::board_asset_rpc::ACTION_ASSET_CHUNK,
                    serde_json::json!({
                        "asset_ref": asset_ref,
                        "asset_token": asset_token,
                        "expires_at": expires_at,
                        "offset": offset,
                        "len": len,
                    }),
                    ASSET_CHUNK_TIMEOUT,
                )
                .await?;
            let chunk: serde_json::Value = serde_json::from_slice(&raw)
                .map_err(|e| format!("asset.chunk response malformed: {e}"))?;
            let data_b64 = chunk
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or("asset.chunk response missing data")?;
            let data = nemesis_cluster::transfer::b64_decode(data_b64)?;
            buf.extend_from_slice(&data);
            offset += len;
        }
        if buf.len() as u64 != size {
            return Err(format!(
                "RPC transfer incomplete: expected {size} bytes, got {}",
                buf.len()
            ));
        }

        // 3) 端到端完整性 + 落定（写 part → finalize 校验改名）。
        let actual = nemesis_board::sha256_bytes(&buf);
        if actual != expected_sha {
            return Err(format!(
                "sha256 mismatch after RPC transfer: expected {expected_sha}, \
                 got {actual}"
            ));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        std::fs::write(dest, &buf).map_err(|e| format!("write {}: {e}", dest.display()))?;
        self.finalize_download(dest, asset_ref, expected_sha)
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
            &read_asset_node_id(&self.workspace),
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
         HMAC token + sha256 verification; if the provider is unreachable \
         over HTTP, automatically falls back to cluster RPC chunked \
         transfer when the bundle carries a node_id), or publish a local \
         workspace file as a board asset that other nodes can download. \
         For fetch, pass the fields of the reference bundle verbatim. For \
         publish, the result includes a reference bundle to paste into the \
         board thread so the requester can download your file."
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
                "node_id": {
                    "type": "string",
                    "description": "fetch: optional provider cluster node id from the bundle (node_id field). Enables cluster RPC fallback when direct HTTP is unreachable; omit for legacy bundles."
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
                node_id,
            } => {
                self.do_fetch(
                    &node_url,
                    &asset_ref,
                    &asset_token,
                    expires_at,
                    &sha256,
                    node_id.as_deref(),
                )
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
