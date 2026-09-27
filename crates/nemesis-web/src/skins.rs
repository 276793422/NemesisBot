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
    fn load_css(&self, id: &str) -> Option<String> {
        let mut zip = self.open_zip(id)?;
        let entry = Self::manifest_str(&mut zip, "entry")?;
        let mut css = String::new();
        zip.by_name(&entry).ok()?.read_to_string(&mut css).ok()?;
        Some(css)
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

/// 签名徽标四态。
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
            SkinSignature::Invalid,
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
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries
}

#[cfg(test)]
mod tests;
