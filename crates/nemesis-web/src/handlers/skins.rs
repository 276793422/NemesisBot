//! Skins WSAPI handler — Dashboard 设置页「皮肤」tab 的管理面。
//!
//! 命令：`list` / `reload`（现扫即最新，语义锚点）/ `detail` / `set_active`
//! / `install`（P2 渠道下载）。扫描与验证在 `crate::skins::scan_skins`（管理面
//! 按需现扫现验，stateless scan-per-call 无缓存失效面；数据面分发不验签）。
//!
//! `install` = verify-before-install 渠道（2026-09-27 四裁决）：`{url}` 任意
//! https 地址 / `{source:"release"}` 官方 Release（nightly-skins.zip 内存解包
//! 逐包安装），共用同一条「下载（SSRF 逐跳闸 + 25MB 上限）→ install_bytes
//! （物理闸在落盘前，信任全落盘）」管线；本地文件导入走 HTTP
//! `POST /api/skins/import`（server.rs，同落 install_bytes）。徽标 = 安装时
//! 验签结论，五态全部落盘（信任闸单点在 set_active）。
//!
//! `set_active` 是唯一的信任决策写点：校验（存在 + status=ok + 有主题
//! 载荷 + require_signed 策略）→ 写 config.json `ui.skin`（live 优先
//! 装配，与 config handler 同一 helper）→ 翻内存共享锁（免重启热切，
//! 下一请求即生效）。`id="default"` = 关皮肤（active.css 404，前端既有
//! 回落链生效）。
//!
//! 装配：web_init 注入 {skins 目录, 激活 id 共享锁句柄} 到模块级槽位
//!（PROJECTS_BRIDGE 同款模式）；未注入 = 全部命令诚实报「未装配」。SSRF
//! 闸经 `set_ssrf_guard` 静态槽回填（init_web 早于 init_agent 的安全插件
//! 构建，顺序解耦；None = 直通）。

#![cfg(feature = "skins")]

use crate::skins::{
    SKIN_PACKAGE_MAX_BYTES, SkinSignature, SkinStatus, crl_snapshot_info, install_bytes, scan_skins,
};
use crate::ws_router::{ModuleHandler, RequestContext};
use axum::extract::State;
use parking_lot::RwLock;
use serde_json::{Value, json};
use std::sync::Arc;

/// 渠道下载 SSRF 闸槽（P2）：`SecurityPlugin::ssrf_guard()` 的克隆（Guard
/// Clone 共享 inner 状态）。gateway 在 init_agent 后注入；None = 直通
///（security feature 关 / security.enabled=false / ssrf 层关）。
#[cfg(feature = "security")]
static SSRF_GUARD_SLOT: RwLock<Option<nemesis_security::ssrf::Guard>> = RwLock::new(None);

/// gateway 装配点：注入 SSRF 闸（与 set_handle 顺序无关）。
#[cfg(feature = "security")]
pub fn set_ssrf_guard(guard: Option<nemesis_security::ssrf::Guard>) {
    *SSRF_GUARD_SLOT.write() = guard;
}

/// 模块级装配槽：{skins 目录, 激活 id 共享锁}。
static SKINS_SLOT: RwLock<Option<SkinsHandle>> = RwLock::new(None);

#[derive(Clone)]
struct SkinsHandle {
    /// exe 同级 `skins/` 目录（None = exe 路径不可定位）。
    dir: Option<String>,
    /// 激活 id 共享锁——与 router 内 SkinHost 同一把锁（热切语义）。
    active: Arc<RwLock<String>>,
}

/// web_init 装配点：注入目录 + 锁句柄。
pub fn set_handle(dir: Option<String>, active: Arc<RwLock<String>>) {
    *SKINS_SLOT.write() = Some(SkinsHandle { dir, active });
}

fn take_handle() -> Result<SkinsHandle, String> {
    SKINS_SLOT
        .read()
        .clone()
        .ok_or_else(|| "皮肤系统未装配（web_init 未注入句柄）".to_string())
}

pub struct SkinsHandler;

impl Default for SkinsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SkinsHandler {
    pub fn new() -> Self {
        Self
    }

    fn list(&self) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let Some(dir) = h.dir else {
            // exe 路径不可定位的极端装配：诚实空表。
            return Ok(Some(
                json!({ "dir": Value::Null, "dir_exists": false, "skins": [] }),
            ));
        };
        let dir_exists = std::path::Path::new(&dir).is_dir();
        let skins = scan_skins(&dir);
        // P3 管理面 CRL 快照状态（在场 + 装载各态诚实呈现）。
        let crl = crl_snapshot_info(&dir, &crate::skins::resolve_anchors(), &skins);
        Ok(Some(
            json!({ "dir": dir, "dir_exists": dir_exists, "skins": skins, "crl": crl }),
        ))
    }

    fn detail(&self, data: Option<Value>) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let id = data
            .as_ref()
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 id".to_string())?;
        let dir = h
            .dir
            .as_deref()
            .ok_or_else(|| "皮肤系统未装配".to_string())?;
        let entry = scan_skins(dir)
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("皮肤不存在：{id}"))?;
        Ok(Some(json!({ "dir": dir, "skin": entry })))
    }

    fn set_active(
        &self,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let id = data
            .as_ref()
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 id".to_string())?;
        let home = ctx
            .home
            .clone()
            .ok_or_else(|| "home not configured".to_string())?;

        // config 一次装载：require_signed 读取与 ui.skin 写入同源。
        let mut cfg = super::config::load_config(&home)?;

        // ① 非默认 id：先过包体/策略校验，全绿才动 config 与锁。
        if id != "default" {
            let dir = h
                .dir
                .as_deref()
                .ok_or_else(|| "皮肤系统未装配".to_string())?;
            let entry = scan_skins(dir)
                .into_iter()
                .find(|e| e.id == id)
                .ok_or_else(|| format!("皮肤不存在：{id}"))?;
            if entry.status != SkinStatus::Ok {
                let reason = entry.status_detail.unwrap_or_else(|| "未知原因".into());
                return Err(format!("皮肤包损坏，无法启用（{reason}）"));
            }
            // v2 双载荷闸 → P2a 三载荷闸：entry（CSS 换色）∥ structure
            //（结构骨架）∥ script（行为扩展）任一在场即可激活；全无 =
            // 无任何可服务载荷，拒绝。
            if entry.manifest.entry.is_none()
                && entry.manifest.structure.is_none()
                && entry.manifest.script.is_none()
            {
                return Err(
                    "该皮肤包无任何载荷（skin/ CSS、structure 结构或 script 脚本），无法设为默认观感"
                        .to_string(),
                );
            }
            // require_signed 后手开关（默认 false；闸在信任决策点，数据面不拦）。
            let require_signed = cfg
                .ui
                .as_ref()
                .map(|u| u.skins.require_signed)
                .unwrap_or(false);
            if require_signed && entry.signature != SkinSignature::Verified {
                return Err("ui.skins.require_signed 已开启：拒绝非 verified 皮肤包".to_string());
            }
        }

        // ② 写 config.json `ui.skin`（typed save，未类型化键保留的回归已锁）。
        let mut ui = cfg.ui.take().unwrap_or_default();
        ui.skin = id.to_string();
        cfg.ui = Some(ui);
        super::config::save_config_to_disk(&home, &mut cfg)?;

        // ③ 翻内存锁：下一请求 active.css 立即切到新皮肤（免重启）。
        *h.active.write() = id.to_string();
        Ok(Some(json!({ "active": id })))
    }

    // -----------------------------------------------------------------------
    // P2 渠道下载（verify-before-install；2026-09-27 四裁决语义）
    // -----------------------------------------------------------------------

    /// `skins.install` 入口：`{url}` 任意 https 地址 / `{source:"release"}`
    /// 官方 Release；可选 `overwrite`（默认 false，重名拒绝）。
    async fn install(&self, data: Option<Value>) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let dir = h
            .dir
            .ok_or_else(|| "皮肤系统未装配（exe 路径不可定位）".to_string())?;
        let data = data.unwrap_or_else(|| json!({}));
        let overwrite = data
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let Some(source) = data.get("source").and_then(|v| v.as_str()) {
            if source == "release" {
                return install_from_release(&dir, overwrite).await;
            }
            return Err(format!("未知 source：{source}（支持 \"release\"）"));
        }
        let url = data
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "缺少 url 或 source（url=任意 https 地址 / source=\"release\"=官方发布）"
                    .to_string()
            })?;
        let bytes = fetch_skin_bytes(url).await?;
        let outcome = install_bytes(&dir, &bytes, overwrite)?;
        Ok(Some(serde_json::to_value(outcome).unwrap_or(json!({}))))
    }

    /// `skins.script_consent`（P2a）：脚本同意卡动作的审计漏斗。
    /// `{id, decision}`（decision = "allow" | "deny"）。逐包现扫取 sha256
    /// 与签名徽标，JSONL 追加 `<workspace>/logs/skin_scripts.log` +
    /// tracing。**记账不改变任何运行时状态**——脚本执行授权在前端
    ///（同意缓存 + `allow_scripts` 开关闸），这里只留防篡改痕迹
    ///（谁在何时允许了哪个包执行脚本）。
    fn script_consent(
        &self,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let dir = h
            .dir
            .as_deref()
            .ok_or_else(|| "皮肤系统未装配".to_string())?;
        let obj = data.as_ref().ok_or_else(|| "缺少 data".to_string())?;
        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 id".to_string())?;
        let decision = obj
            .get("decision")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if decision != "allow" && decision != "deny" {
            return Err("decision 必须是 \"allow\" 或 \"deny\"".to_string());
        }
        // 逐包现扫：sha256（整包摘要 = 同意对象的内容戳）+ 签名徽标如实
        // 入账（allow 一个 🚫 包也会在审计里留下完整现场）。
        let entry = scan_skins(dir)
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("皮肤不存在：{id}"))?;
        let workspace = ctx
            .workspace
            .clone()
            .ok_or_else(|| "workspace not configured".to_string())?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let line = json!({
            "ts": ts,
            "id": id,
            "sha256": entry.sha256,
            "decision": decision,
            "signature": entry.signature,
            "has_script": entry.has_script,
        });
        let logs_dir = std::path::Path::new(&workspace).join("logs");
        std::fs::create_dir_all(&logs_dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs_dir.join("skin_scripts.log"))
            .map_err(|e| format!("打开脚本同意审计日志失败: {e}"))?;
        use std::io::Write as _;
        writeln!(f, "{line}").map_err(|e| format!("写入审计日志失败: {e}"))?;
        tracing::info!(
            target: "security",
            skin = %id,
            %decision,
            sha256 = %entry.sha256,
            signature = ?entry.signature,
            "skin script consent"
        );
        Ok(Some(
            json!({ "logged": true, "id": id, "decision": decision }),
        ))
    }
}

// ---------------------------------------------------------------------------
// 渠道下载机制（fetch + release 解包）
// ---------------------------------------------------------------------------

/// 重定向跳数上限（GitHub release 资产 → S3 一跳即可，5 跳宽裕）。
const FETCH_MAX_HOPS: usize = 5;
const FETCH_UA: &str = "nemesisbot-skins";
const FETCH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const FETCH_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 不跟随重定向的下载 client（Policy::none：SSRF 闸只校验发出去的那一跳，
/// 自动跟跳 = `302 → 内网` 绕闸——image_attach A1 同判例，循环在调用方）。
fn fetch_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(FETCH_CONNECT_TIMEOUT)
        .timeout(FETCH_TOTAL_TIMEOUT)
        .user_agent(FETCH_UA)
        .build()
        .unwrap_or_default()
}

/// 按 SSRF 闸验证过的 IP 集钉死 DNS 的下载 client（防 rebinding TOCTOU——
/// 闸解析过 ≠ reqwest 连接用的 IP；TLS SNI/证书校验仍按原 host）。
#[cfg(feature = "security")]
fn pinned_fetch_client(url: &str, ips: &[std::net::IpAddr]) -> Option<reqwest::Client> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(FETCH_CONNECT_TIMEOUT)
        .timeout(FETCH_TOTAL_TIMEOUT)
        .user_agent(FETCH_UA);
    for ip in ips {
        builder = builder.resolve(&host, std::net::SocketAddr::new(*ip, port));
    }
    builder.build().ok()
}

/// 渠道下载：https-only + 每跳独立过 SSRF 闸（拦 = 诚实失败）+ 手动重定向
/// 循环（≤[`FETCH_MAX_HOPS`] 跳，相对 Location 走 `Url::join`；重定向降级
/// 到非 https 拒绝）+ 流式 25MB 上限。
async fn fetch_skin_bytes(url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("仅支持 https:// 地址".to_string());
    }
    #[cfg(feature = "security")]
    let guard = SSRF_GUARD_SLOT.read().clone();
    let base = fetch_client();
    let mut current = reqwest::Url::parse(url).map_err(|e| format!("URL 解析失败：{e}"))?;
    for _hop in 0..=FETCH_MAX_HOPS {
        #[cfg(feature = "security")]
        let client = match &guard {
            Some(g) => match g.resolve_and_validate_collect(current.as_str()) {
                // Ok(空集) = 闸放行但未给出可钉 IP（allowlist 域名等）→ 共享池。
                Ok(ips) if ips.is_empty() => base.clone(),
                Ok(ips) => {
                    pinned_fetch_client(current.as_str(), &ips).unwrap_or_else(|| base.clone())
                }
                Err(e) => return Err(format!("SSRF 闸拦截：{e}")),
            },
            None => base.clone(),
        };
        #[cfg(not(feature = "security"))]
        let client = &base;

        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| format!("下载失败：{e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| format!("重定向 {status} 缺少 Location 头"))?;
            let next = current
                .join(loc)
                .map_err(|e| format!("Location 解析失败：{e}"))?;
            if next.scheme() != "https" {
                return Err("重定向降级到非 https，已拒绝".to_string());
            }
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(format!("下载失败：HTTP {status}"));
        }
        return read_capped(resp).await;
    }
    Err(format!("重定向超过 {FETCH_MAX_HOPS} 跳，已放弃"))
}

/// 流式读响应体（Content-Length 预检 + chunk 累计双闸；25MB 对齐包上限）。
async fn read_capped(resp: reqwest::Response) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length()
        && len as usize > SKIN_PACKAGE_MAX_BYTES
    {
        return Err(format!(
            "包体 {len} 字节超过 {}MB 上限，已拒收",
            SKIN_PACKAGE_MAX_BYTES / (1024 * 1024)
        ));
    }
    use futures::StreamExt as _;
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断：{e}"))?;
        if out.len() + chunk.len() > SKIN_PACKAGE_MAX_BYTES {
            return Err(format!(
                "包体超过 {}MB 上限，下载已中止",
                SKIN_PACKAGE_MAX_BYTES / (1024 * 1024)
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// 官方发布源 Release 的 nightly-skins.zip 附件名与仓库 API 址——
/// CI 打包链的固定产物（plan §2 P0）。枚举用列表 API 而非
/// `releases/latest`：nightly-build 是 prerelease，latest 语义不认
/// prerelease（实测 HTTP 404），列表第一条才是真实的最新发布。
const RELEASE_ZIP_NAME: &str = "nightly-skins.zip";
const RELEASE_LIST_API: &str =
    "https://api.github.com/repos/276793422/NemesisBot_Rust/releases?per_page=10";

/// 官方 Release 安装：GitHub API 近期 Release 列表 → 首个含 nightly-skins.zip
/// 资产的 Release → 内存解包 → 逐 `.nbskin` 走同一条 [`install_bytes`] 管线
///（signatures.json / certs/ 目录条目跳过）。单包失败不拦其余（errors 逐条
/// 回报，语义 = 每包独立徽标）。
async fn install_from_release(dir: &str, overwrite: bool) -> Result<Option<Value>, String> {
    let api_bytes = fetch_skin_bytes(RELEASE_LIST_API).await?;
    let api: Value = serde_json::from_slice(&api_bytes)
        .map_err(|e| format!("Release API 响应不是合法 JSON：{e}"))?;
    let releases = api
        .as_array()
        .ok_or_else(|| "Release API 响应不是列表（仓库不存在或 API 限流）".to_string())?;
    let zip_url = releases
        .iter()
        .filter_map(|r| r.get("assets").and_then(|v| v.as_array()))
        .find_map(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name").and_then(|v| v.as_str())?;
                (name == RELEASE_ZIP_NAME)
                    .then(|| a.get("browser_download_url").and_then(|v| v.as_str()))
                    .flatten()
            })
        })
        .ok_or_else(|| {
            format!("近期 Release 均未找到 {RELEASE_ZIP_NAME} 附件（CI 尚未产出皮肤包）")
        })?
        .to_string();
    let zip_bytes = fetch_skin_bytes(&zip_url).await?;
    install_zip_entries(dir, &zip_bytes, overwrite)
        .await
        .map_err(|e| format!("{RELEASE_ZIP_NAME} 安装失败：{e}"))
}

/// 内存解包 zip 并逐 `.nbskin` 安装（release 渠道与导入端点共用；name 字段
/// 仅作错误回报定位，落盘名一律 = 包内 `manifest.id`——与渠道语义同源）。
async fn install_zip_entries(
    dir: &str,
    zip_bytes: &[u8],
    overwrite: bool,
) -> Result<Option<Value>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("ZIP 无法解析：{e}"))?;
    let mut installed = Vec::new();
    let mut errors = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("ZIP 条目 {i} 不可读：{e}"))?;
        let name = entry.name().to_string();
        if entry.is_dir() || !name.ends_with(".nbskin") {
            continue; // signatures.json / certs/ / 目录条目不是可安装对象
        }
        let mut buf = Vec::new();
        if let Err(e) = std::io::Read::read_to_end(&mut entry, &mut buf) {
            errors.push(json!({ "file": name, "error": format!("条目不可读：{e}") }));
            continue;
        }
        match install_bytes(dir, &buf, overwrite) {
            Ok(o) => match serde_json::to_value(&o) {
                Ok(mut v) => {
                    v["source_file"] = json!(name);
                    installed.push(v);
                }
                Err(e) => errors.push(json!({ "file": name, "error": e.to_string() })),
            },
            Err(e) => errors.push(json!({ "file": name, "error": e })),
        }
    }
    if installed.is_empty() && !errors.is_empty() {
        let msgs = errors
            .iter()
            .filter_map(|e| e["error"].as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msgs);
    }
    if installed.is_empty() && errors.is_empty() {
        return Err("压缩包内没有 .nbskin 皮肤包".to_string());
    }
    Ok(Some(json!({ "installed": installed, "errors": errors })))
}

// ---------------------------------------------------------------------------
// HTTP 导入端点（本地文件第三入口；server.rs 挂载 /api/skins/import）
// ---------------------------------------------------------------------------

/// 导入 body 上限（25MB 有效载荷 + 1MB 余量，超限 axum 直接 413）。
pub const SKIN_IMPORT_BODY_LIMIT_BYTES: usize = SKIN_PACKAGE_MAX_BYTES + 1024 * 1024;

fn import_error(
    status: axum::http::StatusCode,
    code: &str,
    message: String,
) -> (axum::http::StatusCode, axum::response::Json<Value>) {
    (
        status,
        axum::response::Json(json!({ "error": code, "message": message })),
    )
}

/// `POST /api/skins/import?overwrite=bool` — 本地 .nbskin 文件导入（raw
/// body，前端 `fetch(url, {method:'POST', body: file})` 天然形态）。鉴权与
/// `/api/upload/image` 同约定；落盘走同一条 [`install_bytes`] 管线（物理闸
/// 在落盘前，信任全落盘——与 WSAPI `skins.install` 同一语义；落盘名 =
/// 包内 `manifest.id`，`?name=` 不参与裁决故不设）。
pub async fn handle_import_skin(
    State(state): State<std::sync::Arc<crate::api_handlers::AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Result<axum::response::Json<Value>, (axum::http::StatusCode, axum::response::Json<Value>)> {
    // Dashboard 鉴权（与 upload 同约定）。
    let token = headers
        .get("X-Auth-Token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::api_handlers::verify_token(token, &state.auth_token) {
        return Err(import_error(
            axum::http::StatusCode::UNAUTHORIZED,
            "unauthorized",
            "unauthorized".into(),
        ));
    }
    let overwrite = params
        .get("overwrite")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let dir = take_handle().ok().and_then(|h| h.dir).ok_or_else(|| {
        import_error(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "not_wired",
            "皮肤系统未装配".into(),
        )
    })?;
    match install_bytes(&dir, &body, overwrite) {
        Ok(outcome) => Ok(axum::response::Json(
            serde_json::to_value(outcome).unwrap_or_else(|_| json!({})),
        )),
        Err(e) => Err(import_error(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "rejected",
            e,
        )),
    }
}

#[async_trait::async_trait]
impl ModuleHandler for SkinsHandler {
    fn module_name(&self) -> &str {
        "skins"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "detail",
            "reload",
            "set_active",
            "install",
            "script_consent",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "list" | "reload" => self.list(),
            "detail" => self.detail(data),
            "set_active" => self.set_active(data, ctx),
            "install" => self.install(data).await,
            "script_consent" => self.script_consent(data, ctx),
            _ => Err(format!("unknown command: skins.{cmd}")),
        }
    }
}

#[cfg(test)]
mod tests;
