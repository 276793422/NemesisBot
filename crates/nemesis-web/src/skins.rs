//! 皮肤包（`.nbskin`）加载、分发与**来源验证**。
//!
//! `.nbskin` = 单文件 ZIP（nbskin v1）；签名形态 = ZIP 字节尾部追加 NMBSIG
//! v4 footer（`NMBSIG\x04\x00` magic，摘要覆盖 `[0, L)`，zip crate 按 spec
//! 从文件尾反向扫 EOCD，尾部附加数据天然容忍）。
//!
//! **载荷两形态（v2 起，声明式结构引擎）**：
//! - CSS 载荷（manifest `entry`）——换色，经 `/skins/active.css` 与
//!   `/skins/{id}` 分发，前端注入当前页 `<style>`；
//! - 结构载荷（manifest `structure`）——换骨架，皮肤包自带 UI 结构
//!   （`skin/structure.html`，声明式 `data-nb-*` 原语标注），经
//!   `/skins/active/structure` 与 `/skins/{id}/structure` 分发，前端
//!   结构引擎清洗后渲染。**包内绝不执行任意代码**（清洗/白名单全在
//!   前端引擎，服务端只管原样分发）。
//!
//! 皮肤的语义 = **给当前应用（Dashboard）原地换观感**：CSS 皮肤 = 换色
//! （原生 Vue 布局），结构皮肤 = 换骨架 + 换色；同一 URL、同一应用，
//! 不打开任何新页面。内置官方包 `skins/bot/` 与第三方包走同一引擎。
//! （早期 app 形态已裁定违背皮肤语义，整体移除。）
//!
//! 服务端职责：**分发 + 管理面来源验证**（`scan_skins`，list/reload/
//! set_active 时现扫现验）。数据面（CSS/structure）**不验签**——签名是
//! 来源徽标而非加载闸（D1 定案：目录内所有 .nbskin 一律可加载可用，
//! 目标用户的皮肤可能就是没签名的）。解析/清洗/注入全在前端。
//!
//! 激活 id 来自 `config.json` 的 `ui.skin`（`"default"`/空 = 无皮肤，
//! active.css 与 active/structure 都 404，前端回落原生 UI 并清缓存）。
//! 激活 id 存共享锁（`Arc<RwLock<String>>`），WSAPI `skins.set_active`
//! 免重启热翻。

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use nemesis_verify::verify::VerifyOutcome;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

/// 皮肤宿主：skins 目录 + 激活 id（共享锁，热切地基）。
#[derive(Clone)]
pub struct SkinHost {
    /// exe 同级 `skins/` 目录（None = 无法定位 exe 目录，皮肤面整体关闭）。
    dir: Option<PathBuf>,
    /// 激活皮肤 id（config `ui.skin`）。共享锁：WSAPI `skins.set_active`
    /// 免重启翻锁，下一请求即生效。
    active_id: Arc<RwLock<String>>,
}

/// id 合法性：非空、非 "default"、无路径穿越成分。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "default"
        && !id.contains("..")
        && !id.contains('/')
        && !id.contains('\\')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

impl SkinHost {
    /// 打开 `.nbskin` 包 ZIP（load_css / load_structure 共用件）。任何
    /// 失败（id 非法/文件缺失/ZIP 损坏）一律 `None` → 上层 404。
    fn open_zip(
        &self,
        id: &str,
    ) -> Option<zip::ZipArchive<std::io::BufReader<std::fs::File>>> {
        if !valid_id(id) {
            return None;
        }
        let path = self.dir.as_ref()?.join(format!("{id}.nbskin"));
        let file = std::fs::File::open(path).ok()?;
        zip::ZipArchive::new(std::io::BufReader::new(file)).ok()
    }

    /// 读 manifest.json 的字符串字段（拒 `..` 穿越）。
    fn manifest_str(
        zip: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
        field: &str,
    ) -> Option<String> {
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .ok()?
            .read_to_string(&mut manifest)
            .ok()?;
        let value = serde_json::from_str::<serde_json::Value>(&manifest)
            .ok()?
            .get(field)?
            .as_str()?
            .to_string();
        if value.contains("..") {
            return None;
        }
        Some(value)
    }

    /// 从包读取皮肤 CSS（换色载荷）。缺 structure 的旧包照常可用。
    /// 出口经 [`sanitize_css_urls`]（外链 url() 零请求策略，见函数文档）。
    fn load_css(&self, id: &str) -> Option<String> {
        let mut zip = self.open_zip(id)?;
        let entry = Self::manifest_str(&mut zip, "entry")?;
        let mut css = String::new();
        zip.by_name(&entry).ok()?.read_to_string(&mut css).ok()?;
        Some(sanitize_css_urls(&css))
    }

    /// 从包读取结构载荷（声明式结构引擎消费；v2）。缺 structure 字段 =
    /// 纯 CSS 换色包 → `None` → 上层 404（前端回落原生 UI 布局）。
    fn load_structure(&self, id: &str) -> Option<String> {
        let mut zip = self.open_zip(id)?;
        let entry = Self::manifest_str(&mut zip, "structure")?;
        let mut html = String::new();
        zip.by_name(&entry).ok()?.read_to_string(&mut html).ok()?;
        Some(html)
    }
}

/// 统一响应：200 + text/css，或 404。
fn css_response(css: Option<String>) -> impl IntoResponse {
    match css {
        Some(css) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/css; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            css,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// 统一响应：200 + text/html，或 404。
fn structure_response(html: Option<String>) -> impl IntoResponse {
    match html {
        Some(html) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            html,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /skins/active/structure` — 激活皮肤的结构载荷（v2 结构引擎）。
/// 响应头 `X-Skin-Id` 回传激活 id（前端结构缓存校准真相源，同
/// active.css）。锁快照一次：load 与响应头用同一 id（热翻竞态下不会
/// 头/体分裂）。
async fn handle_active_structure(State(host): State<SkinHost>) -> impl IntoResponse {
    let active_id = host.active_id.read().clone();
    match host.load_structure(&active_id) {
        Some(html) => {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/html; charset=utf-8"),
            );
            headers.insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache"),
            );
            if let Ok(v) = header::HeaderValue::from_str(&active_id) {
                headers.insert(header::HeaderName::from_static("x-skin-id"), v);
            }
            (StatusCode::OK, headers, html).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /skins/active.css` — config `ui.skin` 指向的皮肤。响应头
/// `X-Skin-Id` 回传激活 id（前端打 `data-skin` 属性的真相源——localStorage
/// 缓存可能过期）。
async fn handle_active_css(State(host): State<SkinHost>) -> impl IntoResponse {
    // 锁快照一次：load 与响应头用同一 id（热翻竞态下不会头/体分裂）。
    let active_id = host.active_id.read().clone();
    match host.load_css(&active_id) {
        Some(css) => {
            let mut headers = header::HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("text/css; charset=utf-8"),
            );
            headers.insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-cache"),
            );
            if let Ok(v) = header::HeaderValue::from_str(&active_id) {
                headers.insert(header::HeaderName::from_static("x-skin-id"), v);
            }
            (StatusCode::OK, headers, css).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /skins/{id}` — 显式 id（前端 `?skin=` 预览路径）。axum 0.8 段内
/// 不允许「参数+后缀」，故路由不带 `.css`；这里接受带/不带后缀两种写法
///（`/skins/openlikebuddy.css` 的段值 = `openlikebuddy.css`）。
async fn handle_skin_css(
    State(host): State<SkinHost>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let id = id.strip_suffix(".css").unwrap_or(&id);
    css_response(host.load_css(id))
}

/// `GET /skins/{id}/structure` — 显式 id 的结构载荷（前端 `?skin=` 预览
/// 路径）。缺 structure 字段（纯 CSS 包）= 404。
async fn handle_skin_structure(
    State(host): State<SkinHost>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    structure_response(host.load_structure(&id))
}

/// 构建皮肤路由（挂在主 router 之外、鉴权层之外）。`dir = None`（exe
/// 路径不可定位）→ 不挂任何路由。`active_id` 是共享锁句柄（与 WSAPI
/// `skins.set_active` 同一把锁，热切语义的地基）。
pub fn skin_router(dir: Option<String>, active_id: Arc<RwLock<String>>) -> Option<Router> {
    let dir = dir?;
    let host = SkinHost {
        dir: Some(PathBuf::from(dir)),
        active_id,
    };
    Some(
        Router::new()
            .route("/skins/active.css", get(handle_active_css))
            .route("/skins/active/structure", get(handle_active_structure))
            .route("/skins/{id}", get(handle_skin_css))
            .route("/skins/{id}/structure", get(handle_skin_structure))
            .with_state(host),
    )
}

//
// 管理面：来源验证 + 注册表（scan-per-call，无持久缓存）
//
// 设计（docs/PLAN/2026-09-26_skin-signing-and-management-goal.md §3.2）：
// - 签名是**来源徽标而非加载闸**（D1）：四态 + 正交 broken 状态，灰卡展示。
// - 锚来源复用 v4 现有机制：`NEMESIS_BUILD_ROOT_ANCHOR`（编译期注入，
//   官方 CI 构建）→ `NEMESIS_ROOT_ANCHOR`（运行时 env）→ 皆无 = 无锚，
//   物理上无法验证 → ❔ unverified（诚实四态，非四态不可）。
// - 验证时机：管理面按需（list/reload/set_active 现扫现验）；数据面不验。
//   文件小 + ECDSA 毫秒级，stateless scan-per-call，无缓存失效 bug 面。
//

/// 签名徽标五态（P3 起 +🚫）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkinSignature {
    /// 锚在场，`VerifyOutcome::Valid`——✅
    Verified,
    /// 锚在场，`NoSignature`——⚪
    Unsigned,
    /// 锚在场，其余 outcome（Tampered/Expired/Untrusted/…）——🔴，
    /// 原文见 `sig_detail`
    Invalid,
    /// 锚不在场（本地源码构建），物理上无法验证——❔
    Unverified,
    /// 吊销命中（CRL 快照四维任一 / 网络验签 Revoked outcome）——🚫，
    /// 命中维度与原因见 `sig_detail`
    Revoked,
}

/// 包体状态（与签名**正交**）：物理可服务性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkinStatus {
    /// ZIP 可解析 + manifest.json 存在且可解析（可服务）
    Ok,
    /// ZIP 损坏 / manifest 缺失或不可解析 / format_version 不支持——
    /// 物理不可服务，灰卡注明原因
    Broken,
}

/// manifest.json 元数据（管理面展示用；字段宽松——缺省皆空串，不因
/// 元数据残缺拒服务）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestInfo {
    /// 包内 manifest 声明的 id。路由 id = 文件名 stem，此字段不参与
    /// 路由；与文件名不一致只降级为 `id_mismatch` 注记（元数据可信度）。
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// manifest 自述形态（serde 关键字 `type`；仅展示性元数据——皮肤
    /// 只有 theme 一种形态，路由/切换一律以 `entry` 有无裁决）
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub variants: Vec<String>,
    /// CSS 载荷路径（必需语义；无 = 无观感载荷，set_active 拒绝）
    #[serde(default)]
    pub entry: Option<String>,
    /// 结构载荷路径（v2 声明式结构引擎；缺省 = 纯 CSS 换色包，前端回落
    /// 原生 UI 布局）。set_active 裁决 = entry ∥ structure 任一在场。
    #[serde(default)]
    pub structure: Option<String>,
    /// 格式版本（缺省 = v1 隐含；在场且 ≠1 → broken）
    #[serde(default)]
    pub format_version: Option<u32>,
    /// 品牌显示名（前端骨架槽位：顶栏/主页启动器的品牌文案；空 = 回落 id）
    #[serde(default)]
    pub brand: String,
    /// 主页启动器场景标签（空会话时的快捷入口；空 = 槽位不渲染标签行）
    #[serde(default)]
    pub scenes: Vec<String>,
}

/// 扫描条目：管理面单包全量信息（WSAPI `skins.list` / `skins.detail` 行）。
#[derive(Debug, Clone, Serialize)]
pub struct SkinEntry {
    /// 路由 id = 文件名 stem（`{id}.nbskin`）
    pub id: String,
    /// 文件名（含扩展名）
    pub file: String,
    pub status: SkinStatus,
    /// broken 原因
    pub status_detail: Option<String>,
    pub signature: SkinSignature,
    /// invalid 时的 outcome 原文（Tampered/Expired/…）
    pub sig_detail: Option<String>,
    pub manifest: ManifestInfo,
    /// 完整文件字节（签名形态含 footer）的 SHA-256 hex
    pub sha256: String,
    /// manifest.id ≠ 文件名 stem 的注记（不拒服务，只降级元数据可信度）
    pub id_mismatch: bool,
}

/// 信任锚解析：编译期 `NEMESIS_BUILD_ROOT_ANCHOR` 优先，运行时
/// `NEMESIS_ROOT_ANCHOR` fallback；皆无 = 空集（无锚 → unverified）。
pub fn resolve_anchors() -> Vec<[u8; 32]> {
    nemesis_verify::builtin_root_anchors()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 锚在场时的徽标判定：Valid→verified / NoSignature→unsigned / 其余→
/// invalid（带 outcome 原文；VerifyOutcome 无 Display，此处按 exe-sign-tool
/// 同款口径出人读短文）。pub(crate)：handlers/skins 测试复用。
pub(crate) fn classify_signature(outcome: VerifyOutcome) -> (SkinSignature, Option<String>) {
    match outcome {
        VerifyOutcome::Valid { .. } => (SkinSignature::Verified, None),
        VerifyOutcome::NoSignature => (SkinSignature::Unsigned, None),
        VerifyOutcome::Tampered(s) => (SkinSignature::Invalid, Some(format!("Tampered({s})"))),
        VerifyOutcome::SignatureInvalid => {
            (SkinSignature::Invalid, Some("SignatureInvalid".into()))
        }
        VerifyOutcome::Untrusted => (SkinSignature::Invalid, Some("Untrusted".into())),
        VerifyOutcome::Revoked {
            dim, value, reason, ..
        } => (
            SkinSignature::Revoked,
            Some(format!("Revoked({dim:?}={value}:{reason})")),
        ),
        VerifyOutcome::Expired(s) => (SkinSignature::Invalid, Some(format!("Expired({s})"))),
        VerifyOutcome::UnsupportedVersion(v) => (
            SkinSignature::Invalid,
            Some(format!("UnsupportedVersion({v})")),
        ),
        VerifyOutcome::Malformed(s) => (SkinSignature::Invalid, Some(format!("Malformed({s})"))),
    }
}

/// 包体检查：ZIP 解析 + manifest 读取/解析 + format_version 校验。
/// 返回 (状态, 原因, manifest)。
fn inspect_package(bytes: &[u8]) -> (SkinStatus, Option<String>, Option<ManifestInfo>) {
    let mut zip = match zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
        Ok(z) => z,
        Err(e) => return (SkinStatus::Broken, Some(format!("ZIP 无法解析：{e}")), None),
    };
    let mut manifest_raw = String::new();
    match zip.by_name("manifest.json") {
        Ok(mut f) => {
            if let Err(e) = f.read_to_string(&mut manifest_raw) {
                return (
                    SkinStatus::Broken,
                    Some(format!("manifest.json 不可读：{e}")),
                    None,
                );
            }
        }
        Err(_) => {
            return (SkinStatus::Broken, Some("缺少 manifest.json".into()), None);
        }
    }
    let manifest: ManifestInfo = match serde_json::from_str(&manifest_raw) {
        Ok(m) => m,
        Err(e) => {
            return (
                SkinStatus::Broken,
                Some(format!("manifest.json 不可解析：{e}")),
                None,
            );
        }
    };
    if let Some(v) = manifest.format_version
        && v != 1
    {
        return (
            SkinStatus::Broken,
            Some(format!("format_version {v} 不支持（当前 =1）")),
            None,
        );
    }
    (SkinStatus::Ok, None, Some(manifest))
}

/// 扫描 skins 目录：逐 `.nbskin` 现读现验，产出管理面条目（id 升序）。
/// 目录不存在/不可读 = 空表（上层 handler 自行呈现目录状态）。
pub fn scan_skins(dir: &str) -> Vec<SkinEntry> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let anchors = resolve_anchors();
    let now = now_unix();
    let mut entries = Vec::new();
    for item in read_dir.flatten() {
        let path = item.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("nbskin") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(".nbskin") else {
            continue;
        };
        if !valid_id(stem) {
            // 不合路由 id 规则的文件名（中文/空格/穿越成分）不进注册表——
            // 它们本来就无法被任何路由寻址。
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let (signature, sig_detail) = if anchors.is_empty() {
            (SkinSignature::Unverified, None)
        } else {
            classify_signature(nemesis_verify::verify::verify_bytes(&bytes, &anchors, now))
        };
        let sha256 = sha256_hex(&bytes);
        let (status, status_detail, manifest) = inspect_package(&bytes);
        let id_mismatch = manifest
            .as_ref()
            .is_some_and(|m| !m.id.is_empty() && m.id != stem);
        entries.push(SkinEntry {
            id: stem.to_string(),
            file: file_name.to_string(),
            status,
            status_detail,
            signature,
            sig_detail,
            manifest: manifest.unwrap_or_default(),
            sha256,
            id_mismatch,
        });
    }
    // P3 吊销第五态：CRL 快照在场（且可验）时，对 Verified 包做四维复核
    //（offline）。
    apply_crl_revocations(dir, &anchors, &mut entries);
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries
}

/// CRL 快照吊销复核（P3 第五态；pub(crate) 供测试锚直传）：donor = 首枚
/// Verified 包（吊销只对签名有意义——目录里无 verified 包时快照自然无从
/// 生效）。命中 → Revoked 🚫，注明维度。快照不在场/验不过/过期 = 维持
/// 原态（离线诚实，不猜）。
pub(crate) fn apply_crl_revocations(dir: &str, anchors: &[[u8; 32]], entries: &mut [SkinEntry]) {
    let Some(donor) = entries
        .iter()
        .find(|e| e.signature == SkinSignature::Verified)
        .cloned()
    else {
        return;
    };
    let Ok(donor_bytes) = std::fs::read(std::path::Path::new(dir).join(&donor.file)) else {
        return;
    };
    let (crl, _info) = load_crl_snapshot_state(dir, anchors, Some(&donor_bytes));
    let Some(crl) = crl else {
        return;
    };
    for e in entries.iter_mut() {
        if e.signature != SkinSignature::Verified {
            continue;
        }
        let Ok(bytes) = std::fs::read(std::path::Path::new(dir).join(&e.file)) else {
            continue;
        };
        let meta = nemesis_verify::revocation::revocation_meta(&bytes);
        if let Some(hit) = nemesis_verify::revocation::crl_match_meta(&crl, &meta) {
            e.signature = SkinSignature::Revoked;
            e.sig_detail =
                Some(format!("Revoked({:?}={}): {}", hit.dim, hit.value, hit.reason));
        }
    }
}

//
// 渠道安装（P2 verify-before-install）与 CRL 快照（P3 吊销第五态）
//
// 语义（2026-09-27 用户四裁决，Q4 修订版）：
// - **物理闸在落盘前**：仅物理不可用（broken ZIP / 非 ZIP / 超限）拒收——
//   「这不是皮肤包」，与信任无关；
// - **信任闸单点在 set_active**：渠道验签只产出徽标，✅⚪🔴🚫❔ 全部落盘
//   （「获取自由、徽标诚实、启用单点把关」）；
// - **CRL 快照无网络**：`{skins_dir}/crl.pem`（`GET /v1/crl` 原样 JSON）由
//   运维手动放置，扫描时本地验签（根公钥 = verified 包链根，锚定校验）。
//

/// .nbskin 单包大小上限（25MB，对齐图片上传面）。
pub const SKIN_PACKAGE_MAX_BYTES: usize = 25 * 1024 * 1024;

/// 渠道安装单包结论（WSAPI `skins.install` / 导入端点返回体）。
#[derive(Debug, Clone, Serialize)]
pub struct InstallOutcome {
    pub id: String,
    pub file: String,
    pub signature: SkinSignature,
    pub sig_detail: Option<String>,
    pub manifest: ManifestInfo,
    pub sha256: String,
    pub overwritten: bool,
}

/// 渠道安装管线（三入口——官方 Release / 任意 https URL / 本地导入——共用
/// 同一条验签落盘管道，来源只决定字节从哪来）：
///
/// 1. **物理闸**：超限 / ZIP 损坏 / manifest 缺失或不可解析 → 拒收；
/// 2. **id 裁决**：落盘名 = `manifest.id`（渠道安装要求包内自声明 id，
///    缺失/非法 = 拒——产出物必须可被路由寻址）；
/// 3. **验签**：与 [`scan_skins`] 同源（锚解析 + `verify_bytes`）→ 徽标；
/// 4. **重名闸**：目标已存在且无 `overwrite` → 拒（显式覆盖才动已装包）；
/// 5. **原子落盘**：临时文件 + rename（Windows rename 不覆盖 → 先删旧）。
pub fn install_bytes(dir: &str, bytes: &[u8], overwrite: bool) -> Result<InstallOutcome, String> {
    if bytes.len() > SKIN_PACKAGE_MAX_BYTES {
        return Err(format!(
            "包体 {} 字节超过 {}MB 上限，已拒收",
            bytes.len(),
            SKIN_PACKAGE_MAX_BYTES / (1024 * 1024)
        ));
    }
    let (status, status_detail, manifest) = inspect_package(bytes);
    if status != SkinStatus::Ok {
        return Err(format!(
            "包体物理不可服务，已拒收：{}",
            status_detail.unwrap_or_else(|| "未知原因".into())
        ));
    }
    let manifest = manifest.unwrap_or_default();
    let id = manifest.id.trim();
    if !valid_id(id) {
        return Err(format!(
            "manifest.id（{id:?}）缺失或非法（仅限字母/数字/-/_，不可为 default），已拒收"
        ));
    }
    let anchors = resolve_anchors();
    let (signature, sig_detail) = if anchors.is_empty() {
        (SkinSignature::Unverified, None)
    } else {
        classify_signature(nemesis_verify::verify::verify_bytes(bytes, &anchors, now_unix()))
    };
    let dir_path = std::path::Path::new(dir);
    let path = dir_path.join(format!("{id}.nbskin"));
    let overwritten = path.exists();
    if overwritten && !overwrite {
        return Err(format!("皮肤 {id} 已存在（确认覆盖请带 overwrite=true）"));
    }
    std::fs::create_dir_all(dir_path).map_err(|e| format!("创建皮肤目录失败: {e}"))?;
    let tmp = dir_path.join(format!(".{id}.nbskin.tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;
    if overwritten
        && let Err(e) = std::fs::remove_file(&path)
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("移除旧包失败: {e}"));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("落盘失败: {e}"));
    }
    Ok(InstallOutcome {
        id: id.to_string(),
        file: format!("{id}.nbskin"),
        signature,
        sig_detail,
        manifest,
        sha256: sha256_hex(bytes),
        overwritten,
    })
}

/// CRL 快照管理面状态（诚实呈现装载各态；不在场 = 全默认）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CrlSnapshotInfo {
    pub present: bool,
    /// 根签验签通过（present 且验签/锚定/过期全过才算应用）
    pub verified: bool,
    pub expired: bool,
    pub version: u64,
    pub valid_until: u64,
    pub entries: usize,
    /// 未应用原因（验签失败 / 过期 / 无 donor 等）
    pub note: Option<String>,
}

/// CRL 快照装载（单一真相源）：读 `{dir}/crl.pem` → 根公钥取自
/// `root_bytes`（一枚 verified 包，`anchors` 锚定校验；None = 无 donor）→
/// 根签验签 → 过期检查。`crl = None` = 未应用（原因见 info.note）。
pub fn load_crl_snapshot_state(
    dir: &str,
    anchors: &[[u8; 32]],
    root_bytes: Option<&[u8]>,
) -> (Option<nemesis_verify::Crl>, CrlSnapshotInfo) {
    let mut info = CrlSnapshotInfo::default();
    let Ok(raw) = std::fs::read(std::path::Path::new(dir).join("crl.pem")) else {
        return (None, info); // 不在场 = 维持四态（离线诚实，不猜）
    };
    info.present = true;
    let Some(root_bytes) = root_bytes else {
        info.note = Some("快照在场但目录内无可信签名包提供根公钥，吊销检查未启用".into());
        return (None, info);
    };
    let root_vk = if anchors.is_empty() {
        None
    } else {
        nemesis_verify::revocation::root_pubkey_from_anchored(root_bytes, anchors).ok()
    };
    let Some(root_vk) = root_vk else {
        info.note = Some("无法从 verified 包提取锚定根公钥（无锚/链异常），吊销检查未启用".into());
        return (None, info);
    };
    let Ok(json) = String::from_utf8(raw) else {
        info.note = Some("快照不是 UTF-8 JSON".into());
        return (None, info);
    };
    match nemesis_verify::revocation::load_crl_snapshot(&json, &root_vk) {
        Ok(crl) => {
            info.verified = true;
            info.version = crl.version;
            info.valid_until = crl.valid_until;
            info.entries = crl.entries.len();
            let now = now_unix();
            if crl.valid_until < now {
                info.expired = true;
                info.note = Some(format!(
                    "快照已过期（valid_until 落后 now {} 秒），吊销检查未启用——请更新快照",
                    now - crl.valid_until
                ));
                return (None, info);
            }
            (Some(crl), info)
        }
        Err(e) => {
            info.note = Some(format!("快照验签失败（{e}），吊销检查未启用"));
            (None, info)
        }
    }
}

/// 管理面 CRL 快照状态（handler list 呈现用）：donor = `skins` 里首枚
/// verified 包。
pub fn crl_snapshot_info(dir: &str, anchors: &[[u8; 32]], skins: &[SkinEntry]) -> CrlSnapshotInfo {
    if !std::path::Path::new(dir).join("crl.pem").is_file() {
        return CrlSnapshotInfo::default();
    }
    let root_bytes = skins
        .iter()
        .find(|e| e.signature == SkinSignature::Verified)
        .and_then(|e| std::fs::read(std::path::Path::new(dir).join(&e.file)).ok());
    load_crl_snapshot_state(dir, anchors, root_bytes.as_deref()).1
}

/// CSS `url()` 外链策略（2026-09-27 拍板落地）：皮肤 CSS 内
/// `url(<http(s)/…>)` 是渲染即发起的外链请求面（跟踪像素信标，CDP 实测
/// 证实）——改写为 `about:blank`（声明保留、零请求）。放行：相对/绝对
/// 路径（无 scheme）与栅格图片 data:（与 structure 消毒器同一张 MIME
/// 白名单语义；SVG 等 data: 不放行）。
pub(crate) fn sanitize_css_urls(css: &str) -> String {
    let lower = css.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = String::with_capacity(css.len());
    let mut off = 0usize;
    while let Some(rel) = lower[off..].find("url(") {
        let pos = off + rel;
        let mut j = pos + 4;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let quote = if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
            Some(b[j])
        } else {
            None
        };
        let tok_start = if quote.is_some() { j + 1 } else { j };
        let mut k = tok_start;
        let closed = loop {
            if k >= b.len() {
                break false;
            }
            match quote {
                Some(q) if b[k] == q => break true,
                None if b[k] == b')' => break true,
                _ => k += 1,
            }
        };
        if !closed {
            break; // 畸形（token 未闭合）：剩余原样照抄
        }
        let close = if quote.is_some() {
            let mut m = k + 1;
            while m < b.len() && b[m] != b')' {
                m += 1;
            }
            if m >= b.len() {
                break; // 畸形（右括号缺失）：剩余原样照抄
            }
            m
        } else {
            k
        };
        let tok = &lower[tok_start..k];
        let external =
            tok.starts_with("https://") || tok.starts_with("http://") || tok.starts_with("//");
        let data_bad = tok.starts_with("data:") && !is_raster_data_uri(&tok[5..]);
        out.push_str(&css[off..tok_start]); // "url(" + 前导空白 + 开引号（原样）
        if external || data_bad {
            out.push_str("about:blank");
        } else {
            out.push_str(&css[tok_start..k]);
        }
        out.push_str(&css[k..=close]); // 收引号/收括号 + 后续到 ")"（原样）
        off = close + 1;
    }
    out.push_str(&css[off.min(css.len())..]);
    out
}

/// 栅格图片 MIME 白名单（structure 消毒器同表语义；入参 = data: 后段）。
fn is_raster_data_uri(rest: &str) -> bool {
    let mime = rest.split(';').next().unwrap_or("").trim();
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

#[cfg(test)]
pub(crate) mod tests;
