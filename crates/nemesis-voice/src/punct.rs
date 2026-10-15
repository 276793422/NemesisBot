//! Offline punctuation restoration — ct-transformer via sherpa-onnx
//!
//! Adds proper punctuation (commas, question marks, periods) to STT output
//! before sending to LLM. Uses sherpa-onnx-punct-ct-transformer-zh-en model.

use anyhow::Result;
use std::ffi::CStr;
use std::path::Path;
use std::sync::Mutex;

use crate::sherpa;

pub struct PunctEngine {
    /// 串行化 punct 上的一切 FFI 调用。上游对 OfflinePunctuation 无线程安全
    /// 承诺（沉默 → 按非线程安全对待），而 channel_bridge 的
    /// `VoiceTranscriber: Send + Sync` 契约允许并发 `transcribe`——按处置
    /// 规则加锁，宁慢不炸。依据：`docs/INFO/2026-10-15_voice-ffi-safety-audit.md` C-3。
    lock: Mutex<()>,
    punct: *const sherpa::SherpaOnnxOfflinePunctuation,
}

// SAFETY（审计表 C-3）：punct 指向的 native 对象按非线程安全对待，但所有
// FFI 访问（add_punctuation）都在 `lock` 互斥段内串行执行；指针为 owning
// 裸指针，随实例移动（Send）不产生别名。Sync 由此锁真实成立。
unsafe impl Send for PunctEngine {}
unsafe impl Sync for PunctEngine {}

impl PunctEngine {
    pub fn new(model_path: &Path, num_threads: u32) -> Result<Self> {
        if !model_path.exists() {
            anyhow::bail!("Punctuation model not found: {}", model_path.display());
        }

        let model_c = sherpa::to_cstr(model_path.to_str().unwrap_or(""));
        let provider_c = sherpa::to_cstr("cpu");

        let config = sherpa::SherpaOnnxOfflinePunctuationConfig {
            model: sherpa::SherpaOnnxOfflinePunctuationModelConfig {
                ct_transformer: model_c.as_ptr(),
                num_threads: num_threads as libc::c_int,
                debug: 0,
                provider: provider_c.as_ptr(),
            },
        };

        let punct = unsafe { sherpa::SherpaOnnxCreateOfflinePunctuation(&config) };
        if punct.is_null() {
            anyhow::bail!("Failed to create punctuation engine");
        }

        Ok(Self {
            lock: Mutex::new(()),
            punct,
        })
    }

    /// Add punctuation to text. Returns the punctuated text.
    pub fn add_punctuation(&self, text: &str) -> Result<String> {
        if text.is_empty() {
            return Ok(String::new());
        }

        // 审计表 C-3：FFI 调用段持锁串行。
        // 毒化忽略：本锁只串行化 native 调用（线程亲和），不保护 Rust 不变量——
        // Rust 侧 panic 无法穿透 FFI 破坏 native 对象，持毒拒绝后续调用会把
        // 一次瞬态 panic 放大成引擎永久不可用（直到进程重启）。
        let _serial = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let text_c = sherpa::to_cstr(text);

        let result_ptr =
            unsafe { sherpa::SherpaOfflinePunctuationAddPunct(self.punct, text_c.as_ptr()) };

        let result = if !result_ptr.is_null() {
            let s = unsafe { CStr::from_ptr(result_ptr) }
                .to_str()
                .unwrap_or("")
                .to_string();
            unsafe { sherpa::SherpaOfflinePunctuationFreeText(result_ptr) };
            s
        } else {
            text.to_string()
        };

        Ok(result)
    }
}

impl Drop for PunctEngine {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOfflinePunctuation(self.punct) };
    }
}

#[cfg(test)]
mod tests;
