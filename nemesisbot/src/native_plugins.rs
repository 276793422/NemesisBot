//! 原生插件装载闸 + 签名策略（本地 C-ABI 插件 DLL 的统一裁决点）。
//!
//! 四个原生插件（plugin_ui / plugin_onnx / eval_monitor_dll / plugin_wasm）
//! 的装载点分散在 gateway 装配与 eval 会话各处；本模块给它们三个统一件：
//!
//! - **开关闸**：`<workspace>/config/plugins_native.json`（用户要求的独立
//!   长期记忆配置；读写真相源在 nemesis-config `plugins_native`）。关 =
//!   对应子系统诚实跳过 + 日志注记（eval 即时生效，其余重启生效）。
//! - **签名策略**：与 exe 启动自验**同一三态矩阵**（`verify_policy::
//!   resolve_mode` 单一真相源；`security.signature_verify` off/warn/enforce
//!   + `verify-enforce-lock` 锁定版）——enforce（锁定版构建 / 显式配置）=
//!   库文件必须过 `verify_bytes` 九态管线 Valid 才许装载（fail-closed：读取
//!   失败同拒）；warn（缺省）= 放行 + 状态如实上报（开发构建四个自产 DLL
//!   均未签名，缺省强制会把开发态肢解）；off / 锚缺席 / verify 未编译 =
//!   休眠（放行不验）。锚缺席恒休眠——无锚即无验签能力，与 exe 自验的
//!   「无锚降级」同语义。
//! - **验签/装载同源**（R1 整改 2026-10-22）：[`resolve_load_path`] 是
//!   路径解析单一真相源，`GateOutcome::Allow` 携带获准装载的具体文件。
//!   消费义务两档：装载点另有查找规则的（wasm 宿主壳 loader 的 env 覆盖/
//!   开发路径候选、eval 监控 DLL 的开发形态仓库路径）**必须**按
//!   `Allow.path` 定点装载——enforce 态「验了 A 装 B」的解耦在此封死；
//!   与闸共用 [`find_plugin_library`] 单一查找的（plugin_ui 子窗/tray、
//!   plugin_onnx MemoryManager），闸验的对象与其自带查找恒同文件，
//!   gateway 三个装配点丢弃 `path` 与消费等价。
//! - **清单**：`status_list` / `scan_unrecognized` 供 Dashboard「本地插件
//!   库」页消费（目录全量列出 + 开关状态 + 签名状态 + 未知库列出不装载）。
//!   清单语义 = **exe 旁 plugins/ 目录身份**（部署目录视图），与闸的统一
//!   解析序列是两回事——env 覆盖/开发路径命中的装载对象不进清单。

use nemesis_config::plugins_native::{NativePluginId, load_enabled};
use nemesis_utils::{find_plugin_library, find_plugin_library_in};
use std::path::{Path, PathBuf};

/// 装载闸裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// 装载（开关开 + 签名策略放行、warn 或休眠）。携带获准装载的**具体
    /// 文件**（R1 整改：enforce 态验签的就是它；查找规则与此处不同的
    /// 装载点——wasm 宿主壳/eval 监控 DLL——必须按它定点装载，与
    /// [`find_plugin_library`] 同源的装载点丢弃与消费等价，见模块文档）；
    /// `None` = 库文件缺席（缺席不在此定性——装载点自己的「未找到」
    /// 日志/降级路径负责）。
    Allow { path: Option<PathBuf> },
    /// 用户开关关（诚实跳过，日志注记）。
    DisabledBySwitch,
    /// 签名策略拦截（enforce 态下非 Valid / 读取失败 fail-closed）。
    BlockedBySignature(String),
}

/// 签名闸三态（exe 自验矩阵的原生插件投影）。
#[cfg(feature = "verify")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignatureGateMode {
    /// 放行 + 状态可见（缺省；非 Valid 不拦）。
    Warn,
    /// 非 Valid / 读取失败一律拒载（锁定版 / 显式 enforce）。
    Enforce,
    /// 放行不验（off / 锚缺席）。
    Dormant,
}

/// 三态裁决（锚缺席恒休眠；其余按 security.signature_verify 配置走
/// `verify_policy::resolve_mode`——锁定版在 resolve_mode 内强制 Enforce）。
#[cfg(feature = "verify")]
fn signature_gate_mode(config_dir: &Path) -> SignatureGateMode {
    use crate::verify_policy::VerifyMode;
    if crate::verify_policy::ROOT_ANCHOR.is_none() {
        return SignatureGateMode::Dormant;
    }
    let value = nemesis_utils::fsutil::read_config_string(config_dir.join("config.security.json"))
        .ok()
        .and_then(|raw| {
            crate::verify_policy::parse_signature_verify_value(&raw)
                .ok()
                .flatten()
        });
    let mode = match crate::verify_policy::resolve_mode(value.as_deref()) {
        Ok(r) => r.mode,
        // 非法字符串值在启动自验处 loud 拒绝（exit 86）；能走到这里的其他
        // 装配点保守按缺省 warn，不因重复读盘放大失败面。
        Err(_) => VerifyMode::Warn,
    };
    match mode {
        VerifyMode::Enforce => SignatureGateMode::Enforce,
        VerifyMode::Warn => SignatureGateMode::Warn,
        VerifyMode::Off => SignatureGateMode::Dormant,
    }
}

/// 统一路径解析（单一真相源）：闸的验签对象与装载点的实际装载对象都
/// 消费这里——R1 整改前闸只查 exe 旁 `plugins/`，而 wasm 宿主壳装载器
/// 另有 env 覆盖/exe 同目录/开发路径候选，enforce 态「验了 A 装 B」。
///
/// - `PluginWasm` → 宿主壳装载器候选扫描（`resolve_host_dll_path`，env
///   权威覆盖语义同源；plugins-wasm 未编译时回落 `plugins/`——该形态下
///   子系统根本不装配，解析只服务清单显示）。
/// - `PluginRoaming` → [`PluginLibSpec::ROAMING`] 查找链（`plugins/` 优先
///   → exe 同目录回落）——roaming 装载器（`roaming_plugin_loader`）的
///   dlopen 就走这条链，无连字符变体层；与 `find_plugin_library` 语义
///   不同，必须特判保持「清单/验签对象 = 实际装载对象」同文件。
/// - 其余（plugin_ui / plugin_onnx / eval_monitor_dll 部署形态）→ exe 旁
///   `plugins/`。eval_monitor 的开发形态仓库路径由 eval.rs 自行解析后走
///   [`gate_load_path`]（查找规则留在消费点，闸不掺业务搜索）。
pub fn resolve_load_path(id: NativePluginId) -> Option<PathBuf> {
    match id {
        NativePluginId::PluginWasm => {
            #[cfg(feature = "plugins-wasm")]
            if let Some(p) = nemesis_plugins_wasm::resolve_host_dll_path() {
                return Some(p);
            }
            find_plugin_library(id.id())
        }
        NativePluginId::PluginRoaming => crate::plugin_lib::PluginLibSpec::ROAMING.find_path(),
        _ => find_plugin_library(id.id()),
    }
}

/// 装载闸（四个装载点的唯一裁决入口）——解析与裁决同源：enforce 验的
/// 就是 [`resolve_load_path`] 给出的文件，`Allow` 原样带回该路径。
///
/// DLL 缺席不在此定性——装载点自己的「未找到」日志/降级路径负责（闸只管
/// 「找到了装不装」）；开关关的日志由调用点出（它知道自己在关哪个子系统）。
pub fn gate_load(config_dir: &Path, id: NativePluginId) -> GateOutcome {
    gate_load_path(config_dir, id, resolve_load_path(id).as_deref())
}

/// 路径感知装载闸（自带查找规则的装载点——eval 监控 DLL 的部署/开发双
/// 形态——先解析出具体文件再入闸，enforce 验的就是这个文件）。
pub fn gate_load_path(config_dir: &Path, id: NativePluginId, path: Option<&Path>) -> GateOutcome {
    if !load_enabled(config_dir, id) {
        return GateOutcome::DisabledBySwitch;
    }
    #[cfg(feature = "verify")]
    if signature_gate_mode(config_dir) == SignatureGateMode::Enforce
        && let Some(path) = path
    {
        return match verify_file(path, crate::verify_policy::ROOT_ANCHOR.unwrap_or_default()) {
            Ok(nemesis_verify::verify::VerifyOutcome::Valid { .. }) => GateOutcome::Allow {
                path: Some(path.to_path_buf()),
            },
            Ok(other) => {
                let (state, detail) = outcome_label(&other);
                tracing::warn!(
                    "[NativePlugin] {} 签名策略拦截（{}：{}；enforce 态 = 只装自方根签名库）",
                    id.id(),
                    state,
                    detail
                );
                GateOutcome::BlockedBySignature(format!("{state}: {detail}"))
            }
            Err(e) => {
                tracing::warn!(
                    "[NativePlugin] {} 签名验证读取失败，fail-closed 拒载（{}）",
                    id.id(),
                    e
                );
                GateOutcome::BlockedBySignature(e)
            }
        };
    }
    GateOutcome::Allow {
        path: path.map(Path::to_path_buf),
    }
}

/// 三态的字符串观察窗（测试消费；模式名与 `SignatureStatus.mode` 同词表）。
#[cfg(feature = "verify")]
#[cfg(test)]
pub fn signature_gate_mode_for_test(config_dir: &Path) -> &'static str {
    match signature_gate_mode(config_dir) {
        SignatureGateMode::Enforce => "enforced",
        SignatureGateMode::Warn => "warn",
        SignatureGateMode::Dormant => "dormant",
    }
}

/// 九态 → （短态名，人读明细）——对齐 verify_policy 的 OutcomeSummary 映射。
#[cfg(feature = "verify")]
fn outcome_label(outcome: &nemesis_verify::verify::VerifyOutcome) -> (&'static str, String) {
    use nemesis_verify::verify::VerifyOutcome as O;
    match outcome {
        O::Valid { signed_at, .. } => ("Valid", format!("signed_at={signed_at}")),
        O::NoSignature => ("NoSignature", "无主签名（开发构建 / 未签产物）".to_string()),
        O::Tampered(s) => ("Tampered", s.clone()),
        O::SignatureInvalid => ("SignatureInvalid", "签名自洽验签失败".to_string()),
        O::Untrusted => (
            "Untrusted",
            "信任链不在编译期锚下（外来构建 / 换根）".to_string(),
        ),
        O::Revoked { dim, reason, .. } => ("Revoked", format!("{dim:?} reason={reason}")),
        O::Expired(s) => ("Expired", s.clone()),
        O::UnsupportedVersion(s) => ("UnsupportedVersion", s.clone()),
        O::Malformed(s) => ("Malformed", s.clone()),
    }
}

/// 对单个库文件跑 v4 验签（编译期锚单锚集，同 verify_policy::verify_current_exe 口径）。
#[cfg(feature = "verify")]
fn verify_file(
    path: &Path,
    anchor_hex: &str,
) -> Result<nemesis_verify::verify::VerifyOutcome, String> {
    let anchor = nemesis_verify::hex_util::hex_decode_32(anchor_hex)
        .map_err(|e| format!("锚指纹解码失败: {e}"))?;
    let bytes = std::fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(nemesis_verify::verify::verify_bytes(&bytes, &[anchor], now))
}

/// Dashboard 清单条目（serde 直出）。
#[derive(serde::Serialize)]
pub struct NativePluginStatus {
    pub id: &'static str,
    /// 库文件是否在 exe 旁 plugins/ 找到。
    pub found: bool,
    pub path: Option<String>,
    /// 开关文件当前值（缺省 = true）。
    pub enabled: bool,
    /// 关→开是否即时生效（false = 需重启 gateway）。
    pub immediate_effect: bool,
    pub signature: SignatureStatus,
}

#[derive(serde::Serialize)]
pub struct SignatureStatus {
    /// `enforced`（enforce 态，非 Valid 装载被拦）/ `warn`（放行但状态可见，
    /// 缺省）/ `dormant`（off / 锚缺席 / verify 未编译）。
    pub mode: &'static str,
    /// 九态短态名；dormant / 文件缺席 / 读取失败时可能为 None。
    pub state: Option<String>,
    pub detail: String,
}

/// 本地插件库全量清单（目录身份 + 文件探测 + 开关 + 签名）。
pub fn status_list(config_dir: &Path, exe_dir: &Path) -> Vec<NativePluginStatus> {
    NativePluginId::ALL
        .iter()
        .map(|id| {
            let found = find_plugin_library_in(exe_dir, id.id());
            let signature = signature_status(config_dir, found.as_deref());
            NativePluginStatus {
                id: id.id(),
                found: found.is_some(),
                path: found.map(|p| p.display().to_string()),
                enabled: load_enabled(config_dir, *id),
                immediate_effect: id.immediate_effect(),
                signature,
            }
        })
        .collect()
}

/// 条目签名状态（verify 编译：三态全谱——dormant/warn/enforce）。
#[cfg(feature = "verify")]
fn signature_status(config_dir: &Path, path: Option<&Path>) -> SignatureStatus {
    match signature_gate_mode(config_dir) {
        SignatureGateMode::Dormant => SignatureStatus {
            mode: "dormant",
            state: None,
            detail: "签名策略关闭（security.signature_verify=off 或无信任锚）——所有库放行"
                .to_string(),
        },
        SignatureGateMode::Warn => {
            let Some(path) = path else {
                return SignatureStatus {
                    mode: "warn",
                    state: None,
                    detail: "库文件未找到".to_string(),
                };
            };
            verify_or_state(path, "warn")
        }
        SignatureGateMode::Enforce => {
            let Some(path) = path else {
                return SignatureStatus {
                    mode: "enforced",
                    state: None,
                    detail: "plugins/ 目录未找到该库文件（enforce 态下闸按统一解析 \
                             序列裁决；缺席的降级/报错由装载点负责）"
                        .to_string(),
                };
            };
            verify_or_state(path, "enforced")
        }
    }
}

/// 条目签名状态（verify 未编译：验签执行体缺席，恒休眠——诚实文案）。
#[cfg(not(feature = "verify"))]
fn signature_status(_config_dir: &Path, _path: Option<&Path>) -> SignatureStatus {
    SignatureStatus {
        mode: "dormant",
        state: None,
        detail: "verify 未编译，签名策略休眠（所有库放行）".to_string(),
    }
}

/// warn/enforce 共用的验签状态产出（mode 不同：warn 只报告不拦）。
#[cfg(feature = "verify")]
fn verify_or_state(path: &Path, mode: &'static str) -> SignatureStatus {
    let anchor_hex = crate::verify_policy::ROOT_ANCHOR.unwrap_or_default();
    match verify_file(path, anchor_hex) {
        Ok(outcome) => {
            let (state, detail) = outcome_label(&outcome);
            SignatureStatus {
                mode,
                state: Some(state.to_string()),
                detail,
            }
        }
        Err(e) => SignatureStatus {
            mode,
            state: None,
            detail: e,
        },
    }
}

#[derive(serde::Serialize)]
pub struct UnrecognizedLibrary {
    pub name: String,
    pub path: String,
}

/// 未知库扫描：exe 旁 plugins/ 下平台库扩展名、不在四插件已知名（含连字符
/// 变体）内的文件——「只列出，不装载」。read_dir 结果显式排序（NTFS/ext4
/// 目录序是平台实现细节）。
pub fn scan_unrecognized(exe_dir: &Path) -> Vec<UnrecognizedLibrary> {
    let dir = exe_dir.join("plugins");
    let mut names: Vec<String> = Vec::new();
    for id in NativePluginId::ALL {
        names.push(id.id().to_ascii_lowercase());
        names.push(id.id().replace('_', "-").to_ascii_lowercase());
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<UnrecognizedLibrary> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| {
            matches!(
                e.path().extension().and_then(|x| x.to_str()),
                Some("dll") | Some("so") | Some("dylib")
            )
        })
        .filter(|e| {
            let stem = e
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            !names.contains(&stem)
        })
        .map(|e| UnrecognizedLibrary {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path().display().to_string(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Dashboard「本地插件库」probe 注入槽的全量载荷（状态清单 + 未知库）。
/// gateway 装配期把本函数的闭包（捕获 config_dir / exe_dir）注入
/// nemesis-web plugins handler；handler 侧槽空 = 诚实降级（签名 dormant +
/// 未探测注记，见 handler）。
pub fn native_payload_json(config_dir: &Path, exe_dir: &Path) -> serde_json::Value {
    serde_json::json!({
        "status": status_list(config_dir, exe_dir),
        "unrecognized": scan_unrecognized(exe_dir),
    })
}
