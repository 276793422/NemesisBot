//! v4 验签包装与九态 → 三色分级（计划 §4a：启动自检与签名分级处置）。
//!
//! 两层独立信任链（计划 §4）：
//! - **外壳自检**：安装包本体（PE/ELF/Raw 自动分派）——防安装器程序被改；
//! - **payload manifest 验签**：内嵌 manifest blob——防载荷被换。
//!
//! 分级处置（用户拍板「提示不阻止」+ 红色级知情确认「那就要」）：
//!
//! | VerifyOutcome | 分级 | 交互 |
//! |---------------|------|------|
//! | Valid | 绿 | 静默继续 |
//! | NoSignature（开发构建常态） | 黄 | 提示继续，零交互 |
//! | 其余七态 | 红 | GUI 勾选知情确认 / 静默需 `--accept-signature-risk`，缺 = 退出码 5 |
//!
//! **payload 完整性（sha256 对 manifest）失败 = 硬失败不属于信任决策**
//! ——损坏的包装出来就是坏的，与签名策略无关，不可越过（pkgfile 强制）。
//!
//! 诚实边界（与签名体系既有声明一致）：红警确认继续 = 残余风险面（攻击者
//! 改验证逻辑重打包），纯软件无内核强制下不存在绝对拦截；用户装后走
//! selfupdate（验签链完备）可回到签名版。v1 不做 CRL 联网拉取（安装场景
//! 离线优先），吊销面由信任锚 + 有效期覆盖——Revoked 态仅在提供 CRL 时可达，
//! 当前管线实际不可达，保留变体仅为完备。

use nemesis_verify::verify::VerifyOutcome;

/// 三色分级。
#[derive(Debug, Clone, PartialEq)]
pub enum SigGrade {
    /// 静默继续。
    Green,
    /// 黄色提示继续，零交互。
    Yellow,
    /// 红色警示：需显式知情确认（GUI 勾选 / 静默 `--accept-signature-risk`）。
    Red,
}

impl SigGrade {
    /// 人读文本（GUI 横幅 / 静默 WARN 用）。
    pub fn banner_text(&self, outcome: &VerifyOutcome) -> String {
        match self {
            SigGrade::Green => "签名有效".to_string(),
            SigGrade::Yellow => "未签名安装包，请确认来源可信".to_string(),
            SigGrade::Red => format!(
                "签名异常：{}（请勿信任来路不明的安装包）",
                outcome_text(outcome)
            ),
        }
    }
}

/// 九态 → 三色。映射表即 §4a 分级表（单一真相源）。
pub fn grade(outcome: &VerifyOutcome) -> SigGrade {
    match outcome {
        VerifyOutcome::Valid { .. } => SigGrade::Green,
        VerifyOutcome::NoSignature => SigGrade::Yellow,
        _ => SigGrade::Red,
    }
}

/// 九态人读短文本。
pub fn outcome_text(outcome: &VerifyOutcome) -> String {
    match outcome {
        VerifyOutcome::Valid { signed_at, .. } => format!("Valid（signed_at={signed_at}）"),
        VerifyOutcome::NoSignature => "无签名".to_string(),
        VerifyOutcome::Tampered(e) => format!("内容被篡改: {e}"),
        VerifyOutcome::SignatureInvalid => "签名验签失败".to_string(),
        VerifyOutcome::Untrusted => "不可信（链断/根不在锚集/EKU 缺席）".to_string(),
        VerifyOutcome::Revoked { dim, value, .. } => format!("已吊销（{dim:?}: {value}）"),
        VerifyOutcome::Expired(e) => format!("已过期: {e}"),
        VerifyOutcome::UnsupportedVersion(e) => format!("不支持的版本/算法: {e}"),
        VerifyOutcome::Malformed(e) => format!("结构损坏: {e}"),
    }
}

/// 当前信任锚集（编译期 `NEMESIS_BUILD_ROOT_ANCHOR` 优先 → 运行时
/// `NEMESIS_ROOT_ANCHOR` fallback → 空集 = 一切签名不可信）。
pub fn anchors() -> Vec<[u8; 32]> {
    nemesis_verify::builtin_root_anchors()
}

/// 外壳自检（计划 §4a）：验证**安装器本体**（当前进程可执行文件字节），
/// PE → 证书表 / ELF/raw → v4 footer 自动分派。整包经 `pack sign`（PE 证书表）
/// 签名后，byte-range digest 覆盖载荷/manifest/尾注全段——防安装器程序本体
/// 被改的运行时防线；未签形态 = NoSignature（黄，开发构建常态）。
pub fn shell_self_check() -> crate::pkgfile::VerifyOutcomeDisplay {
    let outcome = match std::env::current_exe().and_then(std::fs::read) {
        Ok(bytes) if !bytes.is_empty() => {
            nemesis_verify::verify::verify_bytes(&bytes, &anchors(), crate::pkgfile::now_unix())
        }
        _ => VerifyOutcome::Malformed("无法读取自身可执行文件".into()),
    };
    crate::pkgfile::VerifyOutcomeDisplay {
        grade: grade(&outcome),
        text: outcome_text(&outcome),
    }
}

#[cfg(test)]
mod tests;
