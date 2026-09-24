//! 签名验证三态裁决——单一真相源（接入计划 §1/§2.1，
//! docs/PLAN/2026-09-23_signature-verify-integration-switch.md）。
//!
//! 职责边界：
//! - **裁决**（[`resolve_mode`]）：锁定版 feature / config 三态 / 编译期锚
//!   三输入 → 生效模式。§1 矩阵的唯一实现，禁止在别处散落同款 if。
//! - **执行**（[`self_check_and_enforce`]）：main.rs `Cli::parse` 后、任何
//!   命令装配前调用（gateway / run / acp 三入口同源覆盖——headless/ACP
//!   不是安全旁路）。enforce 失败 = exit `86`。
//! - **留痕**（[`start_check`]）：启动自验结果存进程级 OnceLock，gateway
//!   装配完安全插件后据此补 tracing 汇总 + 审计链一条；WSAPI
//!   `security.signature_verify_status` 只读消费。
//!
//! 边界声明（接入计划 §6 诚实边界）：
//! - 消费版开关与二进制同处一个可篡改面——开关保可用性与可见性，强制层级
//!   = 锁定版（本模块 `VERIFY_ENFORCE_LOCKED`）> 外部启动器（远期）；
//! - 启动时验一次，运行期间磁盘 exe 可被换（周期复验 = 远期项）；
//! - v1 只验主程序自身，插件不强制验（第三方插件无我方根签证书）。

use nemesis_verify::hex_util::hex_decode_32;
use nemesis_verify::verify::{self, VerifyOutcome};
use std::sync::OnceLock;

/// 锁定版旗标：cargo feature `verify-enforce-lock`（空 feature，纯编译期
/// cfg）。feature 指纹原生跟踪——同 target 目录先消费后锁定绝无 stale
/// （一稿 option_env! 方案的缓存复用坑，见计划 §2.1）。
#[cfg(feature = "verify-enforce-lock")]
pub const VERIFY_ENFORCE_LOCKED: bool = true;
#[cfg(not(feature = "verify-enforce-lock"))]
pub const VERIFY_ENFORCE_LOCKED: bool = false;

/// 编译期信任锚（根证书 SHA-256 指纹 hex）。build.rs 三级优先级注入：
/// env `NEMESIS_BUILD_ROOT_ANCHOR` > 仓库 `certs/root_cert.der` 现算 > None。
/// build.rs 无锚时显式 emit 空值压掉 ambient 同名变量，空串按 None 处理
/// （const 上下文不能 match str 模式，走 `str::is_empty` 判空）。
const fn anchor_from_env(v: Option<&'static str>) -> Option<&'static str> {
    match v {
        None => None,
        Some(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
    }
}

pub const ROOT_ANCHOR: Option<&'static str> = anchor_from_env(option_env!("NEMESIS_ROOT_ANCHOR"));

/// enforce 拒启退出码（计划决策 6）。
pub const EXIT_ENFORCE_REJECTED: i32 = 86;

/// 生效模式（§1 矩阵输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    Off,
    Warn,
    Enforce,
}

impl VerifyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            VerifyMode::Off => "off",
            VerifyMode::Warn => "warn",
            VerifyMode::Enforce => "enforce",
        }
    }
}

/// 裁决结果。
#[derive(Debug, Clone)]
pub struct Resolution {
    /// 生效模式（矩阵裁决后）。
    pub mode: VerifyMode,
    /// 锁定版构建（config 被忽略）。
    pub locked: bool,
    /// 编译期锚（None = 无锚）。
    pub anchor: Option<&'static str>,
    /// 所配策略未兑现被降级（无锚且原请求非 off → 降级 off + 响亮声明）。
    pub degraded: bool,
}

/// 三态裁决矩阵（§1）——**单一实现**，裁决顺序硬编码：
/// 1. config 值合法性先行（未知值 loud 拒绝——错值不是「值」，锁定版也不吞）；
/// 2. 锁定版 → Enforce（config 任意合法值被忽略）；
/// 3. 非锁定：off→Off / 缺省或 warn→Warn / enforce→Enforce；
/// 4. 无锚修正：锁定 → 维持 Enforce（启动自验必失败 → 拒启 86，构建错误
///    诚实暴露）；非锁定 → 降级 Off（`degraded = true` 当原请求非 off）。
pub fn resolve_mode(config_value: Option<&str>) -> Result<Resolution, String> {
    let requested = match config_value {
        None | Some("") => VerifyMode::Warn,
        Some("off") => VerifyMode::Off,
        Some("warn") => VerifyMode::Warn,
        Some("enforce") => VerifyMode::Enforce,
        Some(other) => {
            return Err(format!(
                "security.signature_verify 非法值 {other:?}（合法值：off / warn / enforce；留空 = 默认 warn）"
            ));
        }
    };
    let anchor = ROOT_ANCHOR;
    if VERIFY_ENFORCE_LOCKED {
        return Ok(Resolution {
            mode: VerifyMode::Enforce,
            locked: true,
            anchor,
            degraded: anchor.is_none(),
        });
    }
    let (mode, degraded) = match (requested, anchor.is_none()) {
        (VerifyMode::Off, _) => (VerifyMode::Off, false),
        (_req, true) => (VerifyMode::Off, true),
        (req, false) => (req, false),
    };
    Ok(Resolution {
        mode,
        locked: false,
        anchor,
        degraded,
    })
}

/// 启动自验的单次结果摘要（九态名 + key_fp + 细节；WSAPI 只读透传）。
#[derive(Debug, Clone)]
pub struct OutcomeSummary {
    /// "Valid" / "NoSignature" / "Tampered" / "SignatureInvalid" / "Untrusted" /
    /// "Revoked" / "Expired" / "UnsupportedVersion" / "Malformed"；
    /// "Error" = 读自身 exe / 解码锚等装配层失败（非九态，诚实标注）。
    pub state: String,
    pub key_fp: Option<String>,
    pub detail: String,
}

/// 启动自验快照（gateway 审计链 / WSAPI / 前端徽标的数据源）。
#[derive(Debug, Clone)]
pub struct StartCheck {
    /// 生效模式（矩阵裁决后）。
    pub mode: VerifyMode,
    pub locked: bool,
    pub anchor_fp: Option<&'static str>,
    /// None = 未执行验签（off 跳过 / 无锚降级）。
    pub outcome: Option<OutcomeSummary>,
    /// 配置了非 off 但无锚被降级。
    pub degraded: bool,
}

static START: OnceLock<StartCheck> = OnceLock::new();

/// 启动自验快照只读访问（gateway 审计 + WSAPI status）。
pub fn start_check() -> Option<&'static StartCheck> {
    START.get()
}

/// 启动自验单一入口：`Cli::parse` 之后、任何命令装配之前调用
/// （main.rs 两个平台入口各一行，gateway / run / acp 及其余子命令同源覆盖）。
///
/// 控制台输出走 `eprintln!`——调用点早于 lazy logger 初始化，tracing 全局
/// subscriber 尚未装配；gateway 稍后会从 [`start_check`] 补正式日志与审计链。
pub fn self_check_and_enforce(local_mode: bool) {
    let config_value = read_config_value(local_mode);
    let resolution = match resolve_mode(config_value.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            // 未知取值 loud 拒绝（计划 §1；对齐 protocol 字段未知值风格）。
            eprintln!("[verify] ❌ {e}");
            std::process::exit(EXIT_ENFORCE_REJECTED);
        }
    };

    let check = |outcome: Option<OutcomeSummary>| StartCheck {
        mode: resolution.mode,
        locked: resolution.locked,
        anchor_fp: resolution.anchor,
        outcome,
        degraded: resolution.degraded,
    };

    // 锁定版无锚 = 构建错误：拒启（矩阵第 5 行；绝不产「起不来的锁定包」）。
    if resolution.locked && resolution.anchor.is_none() {
        eprintln!(
            "[verify] ❌ 锁定版构建缺少编译期信任锚（NEMESIS_BUILD_ROOT_ANCHOR / certs/root_cert.der 皆缺）\
——锁定 enforce 无锚 = 构建错误，拒绝启动（exit {EXIT_ENFORCE_REJECTED}）"
        );
        let _ = START.set(check(None));
        std::process::exit(EXIT_ENFORCE_REJECTED);
    }

    // 降级 off（非锁定 + 无锚 + 原请求非 off）：响亮声明后照常运行。
    if resolution.mode == VerifyMode::Off {
        if resolution.degraded {
            eprintln!(
                "[verify] ⚠️ 无信任锚（编译期未注入 NEMESIS_BUILD_ROOT_ANCHOR 且 certs/root_cert.der 缺席）\
——签名验证不可用，降级 off。锁定版部署请先完成密钥仪式并注入锚"
            );
        } else {
            // off 显式关闭：完全跳过验签，INFO 声明（矩阵第 1 行）。
            eprintln!("[verify] 签名验证已关闭（security.signature_verify=off）");
        }
        let _ = START.set(check(None));
        return;
    }

    // Warn / Enforce：跑自验。
    let anchor = resolution.anchor.unwrap_or_default(); // 非 off 必有锚（矩阵已保证）
    let outcome = verify_current_exe(anchor);
    let ok = outcome.state == "Valid";
    let kf = outcome.key_fp.clone();
    let state = outcome.state.clone();

    if ok {
        eprintln!(
            "[verify] ✓ 自身签名验证通过（mode={}，locked={}，key_fp={}）",
            resolution.mode.as_str(),
            resolution.locked,
            kf.as_deref().unwrap_or("-"),
        );
    } else {
        let action = if resolution.mode == VerifyMode::Enforce {
            "拒绝启动"
        } else {
            "继续运行（warn）"
        };
        eprintln!(
            "[verify] ⚠️ 自身签名验证失败：{state}（{}）——{action}",
            outcome.detail,
        );
        if let Some(kf) = &kf {
            eprintln!("[verify]    key_fp={kf}");
        }
    }

    let _ = START.set(check(Some(outcome)));

    if !ok && resolution.mode == VerifyMode::Enforce {
        std::process::exit(EXIT_ENFORCE_REJECTED);
    }
}

/// 读 config.security.json 顶层 `signature_verify`（原始 JSON 单键读取，
/// 与 security_setup.rs 同文件同姿态）。文件缺席 = 未配置（None）；
/// JSON 解析失败 / 键存在但非字符串 = None + 响亮提示（值合法性仍由
/// [`resolve_mode`] 把关，这里不做二次裁决）。
fn read_config_value(local_mode: bool) -> Option<String> {
    let home = crate::common::resolve_home(local_mode);
    let path = crate::common::security_config_path(&home);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return None,
    };
    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[verify] ⚠️ security 配置解析失败（{}）——signature_verify 按未配置处理",
                e
            );
            return None;
        }
    };
    match json.get("signature_verify") {
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Some(serde_json::Value::Null) => None,
        Some(_) => {
            eprintln!(
                "[verify] ⚠️ security.signature_verify 必须是字符串（off/warn/enforce）——当前按未配置处理"
            );
            None
        }
        None => None,
    }
}

/// 验证当前进程 exe（lib 直接验签，不走 DLL；编译期锚单锚集）。
fn verify_current_exe(anchor_hex: &str) -> OutcomeSummary {
    let run = || -> Result<VerifyOutcome, String> {
        let anchor = hex_decode_32(anchor_hex).map_err(|e| format!("锚指纹解码失败: {e}"))?;
        let exe = std::env::current_exe().map_err(|e| format!("current_exe 失败: {e}"))?;
        let bytes = std::fs::read(&exe)
            .map_err(|e| format!("读取自身 exe（{}）失败: {}", exe.display(), e))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(verify::verify_bytes(&bytes, &[anchor], now))
    };
    match run() {
        Ok(outcome) => match outcome {
            VerifyOutcome::Valid {
                signed_at, key_fp, ..
            } => OutcomeSummary {
                state: "Valid".into(),
                key_fp: Some(hex_encode(&key_fp)),
                detail: format!("signed_at={signed_at}"),
            },
            VerifyOutcome::NoSignature => OutcomeSummary {
                state: "NoSignature".into(),
                key_fp: None,
                detail: "无主签名（开发构建 / 未签产物 / 签名被剥离）".into(),
            },
            VerifyOutcome::Tampered(s) => OutcomeSummary {
                state: "Tampered".into(),
                key_fp: None,
                detail: s,
            },
            VerifyOutcome::SignatureInvalid => OutcomeSummary {
                state: "SignatureInvalid".into(),
                key_fp: None,
                detail: "签名自洽验签失败".into(),
            },
            VerifyOutcome::Untrusted => OutcomeSummary {
                state: "Untrusted".into(),
                key_fp: None,
                detail: "信任链不在编译期锚下（换根 / 外来构建）".into(),
            },
            VerifyOutcome::Revoked {
                dim, value, reason, ..
            } => OutcomeSummary {
                state: "Revoked".into(),
                key_fp: None,
                detail: format!("{dim:?}={value} reason={reason}"),
            },
            VerifyOutcome::Expired(s) => OutcomeSummary {
                state: "Expired".into(),
                key_fp: None,
                detail: s,
            },
            VerifyOutcome::UnsupportedVersion(v) => OutcomeSummary {
                state: "UnsupportedVersion".into(),
                key_fp: None,
                detail: format!("envelope version={v}"),
            },
            VerifyOutcome::Malformed(s) => OutcomeSummary {
                state: "Malformed".into(),
                key_fp: None,
                detail: s,
            },
        },
        Err(e) => OutcomeSummary {
            state: "Error".into(),
            key_fp: None,
            detail: e,
        },
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
