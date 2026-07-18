//! 语言补救（language remedy）—— 独立的策略模块。
//!
//! # 解决什么问题
//! 某些多语种 STT 模型在 `language = "auto"` 下会把目标语言误判成别的语言
//! （典型：SenseVoice-Small 把英文误判成日语）。本模块提供"补救"：检测到的语言
//! 若不在白名单内，就用一个强制目标语言的识别器把同一句重解一次。
//!
//! # 设计（四步）
//! 1. 本模块独立于 STT 解码主体（`stt.rs`），只管"补救策略 + 执行"。
//! 2. **底层判断 = 模型自声明**：[`default_remedy_for_model`] 决定某模型是否需要补救、
//!    补救方式是什么。只有已知会误判的特定模型返回 `Some`，其它模型返回 `None`——
//!    `None` 时本模块完全不介入，补丁"不存在"。
//! 3. 补救方式 = [`Remedy { allowed, fallback }`](Remedy)。
//! 4. 执行 = [`LangRestriction::apply`]：检测语言不在 `allowed` → 用 fallback 识别器重解。
//!
//! `SttEngine` 只负责：问模型要不要补救（拿 `Remedy`）→ 要就建 fallback 识别器 →
//! 主解码后交给本模块决定是否重解。
//!
//! # 操作指引（HOWTO）
//! - **启用**：`config.toml` 里 `[stt] lang_remedy = true`（且 `language = "auto"`）。
//!   发布模板默认 `true`，故新 onboarding 自动启用。
//! - **禁用**：`lang_remedy = false`，或删掉该行（serde 默认 `false` = 不补救 = 原始 auto）。
//! - **换模型**：新模型若不在 [`default_remedy_for_model`] 的声明表里 → 返回 `None` →
//!   自动不补救，**无需改任何代码**。这就是"换不误判的模型即可规避此问题"的落地方式。
//! - **新增一个会误判的模型**：在 [`default_remedy_for_model`] 里加一条 `starts_with` 分支，
//!   返回对应的 `Remedy`。不需要动 `SttEngine` 或解码逻辑（见该函数文档里的例子）。
//! - **改补救策略**（如换成 zh+en 之外的组合）：改对应模型那条 `Remedy` 的 `allowed`/`fallback`。
//!
//! # 代价
//! 启用时多挂一个识别器实例（约 +250MB 常驻内存），误判句多一次解码（约 75ms，单线程）。

use std::collections::HashSet;

use anyhow::Result;

use crate::sherpa;
use crate::stt::decode_recognizer;

/// 一个模型的"补救方式"。
#[derive(Debug, Clone)]
pub struct Remedy {
    /// 允许的语言码（归一化后，如 "zh"、"en"）。检测结果在此集合内就不补救。
    pub allowed: HashSet<String>,
    /// 检测结果不在 `allowed` 时，用哪个语言重解。
    pub fallback: String,
}

/// 模型自声明：这个模型在 auto 模式下是否需要语言补救，需要的话补救方式是什么。
///
/// 这就是"底层标记"：只有已知会误判的特定模型返回 `Some`；其它模型返回 `None`，
/// 补救逻辑自然不介入。**换模型时，新模型若不在本表 → 返回 `None` → 无补救，无需改代码。**
///
/// # 新增一个会误判的模型
/// 在这里加一条 `starts_with` 匹配即可，**不需要改 `SttEngine` 或任何解码逻辑**：
/// ```ignore
/// // 在 default_remedy_for_model 里加一条分支：
/// if lower.starts_with("some-new-model") {
///     let mut allowed = HashSet::new();
///     allowed.insert("zh".to_string());
///     allowed.insert("en".to_string());
///     return Some(Remedy { allowed, fallback: "en".to_string() });
/// }
/// ```
pub fn default_remedy_for_model(model_name: &str) -> Option<Remedy> {
    let lower = model_name.to_lowercase();
    if lower.starts_with("sensevoice") {
        // SenseVoice-Small (zh-en-ja-ko-yue) 在 auto 下会把英文误判成日语/韩语/粤等。
        // "只识别中英文"：检测到非 zh/en 就用强制英文引擎重解。
        let mut allowed = HashSet::new();
        allowed.insert("zh".to_string());
        allowed.insert("en".to_string());
        Some(Remedy {
            allowed,
            fallback: "en".to_string(),
        })
    } else {
        None
    }
}

/// 持有一个强制 fallback 语言的识别器 + 补救策略，负责执行重解。
///
/// 由 `SttEngine` 构建并拥有（`SttEngine` 负责 FFI 建识别器，本结构负责策略）。
/// 生命周期跟随 `SttEngine`：`SttEngine` 的 `restriction: Option<LangRestriction>` 字段
/// drop 时，本结构的 `Drop` 销毁 fallback 识别器。
pub struct LangRestriction {
    remedy: Remedy,
    fallback_recognizer: *const sherpa::SherpaOnnxOfflineRecognizer,
}

unsafe impl Send for LangRestriction {}
unsafe impl Sync for LangRestriction {}

impl Remedy {
    /// 检测结果是否需要补救（纯逻辑，可单测，不碰识别器）。
    /// `None`（检测不到语言）或语言在白名单内 → 不补救；否则补救。
    pub fn needs_remedy(&self, detected: Option<&str>) -> bool {
        match detected {
            None => false,
            Some(d) => !self.allowed.contains(d),
        }
    }
}

impl LangRestriction {
    /// 由 `SttEngine` 调用：`fallback_recognizer` 必须是用 `remedy.fallback` 语言建好的识别器。
    pub fn new(
        remedy: Remedy,
        fallback_recognizer: *const sherpa::SherpaOnnxOfflineRecognizer,
    ) -> Self {
        Self {
            remedy,
            fallback_recognizer,
        }
    }

    /// 看检测结果是否要补救。要就用 fallback 识别器重解，返回 `(文本, fallback 语言)`；
    /// 不要（允许的语言 / 检测不到语言）返回 `None`。
    pub fn apply(
        &self,
        detected: Option<&str>,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<Option<(String, String)>> {
        if !self.remedy.needs_remedy(detected) {
            return Ok(None);
        }
        let (text, _) = decode_recognizer(self.fallback_recognizer, samples, sample_rate)?;
        Ok(Some((text, self.remedy.fallback.clone())))
    }
}

impl Drop for LangRestriction {
    fn drop(&mut self) {
        if !self.fallback_recognizer.is_null() {
            unsafe { sherpa::SherpaOnnxDestroyOfflineRecognizer(self.fallback_recognizer) };
        }
    }
}

#[cfg(test)]
mod tests;
