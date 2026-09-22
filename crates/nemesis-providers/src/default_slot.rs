//! 统一默认模型 provider 槽 + 委派 wrapper（2026-09-22）。
//!
//! 病史：模型热切的运行期联动原是一份手写罗列清单（主 loop → Forge →
//! 项目 loop → AppState 展示字段），清单外的 provider 消费者在启动装配期
//! 把 provider 实例「烘焙」进长生命周期对象，`models.set_default` 之后
//! 永远用旧实例——项目 loop（09-21 修复）、集群节点 loop（09-22 事故：
//! 切模型后集群回消息仍报 NullProvider「启动时装配失败」）、workflow
//! 引擎、security guardian judge、SSE/persona streaming 槽，全是同病。
//!
//! 根治手段：这些装配点不再持有裸 provider，改持 [`default_following`]
//! 产出的委派 wrapper。wrapper 捕获装配时的 (provider, 模型名两种形态)；
//! 调用到来时若模型名是「装配时默认」或「当前默认」（或空），委派给槽里
//! 的**当前**默认 provider + 当前模型名；否则视为显式钉扎（small_model、
//! run --model 等），保持装配时语义原样透传。槽的唯一运行期写点是 web
//! `models.set_default`（及 update_field 的 protocol/proxy 命中当前默认
//! 分支）——手写联动清单收敛为单一 chokepoint，新消费者装配时用 wrapper
//! 即自动跟随热切。守卫：`scripts/check-provider-bake.sh`（白名单外出现
//! 新的 factory 烘焙点即 CI 红）。
//!
//! 槽未安装（`swap` 从未被调用——单测直建 cluster agent、CLI 一次性进程）
//! 时 wrapper 走捕获降级 = 历史行为，零影响。
//!
//! 已知不可分辨情形（良性）：装配时钉扎名恰好等于当时默认名 → 之后跟随
//! 槽。「钉扎到默认同款模型」与「默认跟随」语义本就重合，接受。

use crate::failover::FailoverError;
use crate::http_provider::StreamChunk;
use crate::router::LLMProvider;
use crate::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::sync::{Arc, OnceLock};

/// 槽条目：当前默认 provider + 名字两种形态（去前缀 model_name 与
/// provider 前缀化 llm_ref）——调用方两种形态都会传，判定都要认。
struct SlotEntry {
    provider: Arc<dyn LLMProvider>,
    model_name: String,
    llm_ref: String,
}

/// 进程级默认模型槽。None = 尚未热切过（wrapper 走捕获降级）。
static SLOT: OnceLock<RwLock<Option<SlotEntry>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<SlotEntry>> {
    SLOT.get_or_init(|| RwLock::new(None))
}

/// 运行期写点（唯一）：web `models.set_default` / `update_field` 的
/// protocol、proxy 命中当前默认分支。传入与主 loop 热切同一个 provider
/// Arc，无二次构造。
pub fn swap(provider: Arc<dyn LLMProvider>, model_name: &str, llm_ref: &str) {
    *slot().write() = Some(SlotEntry {
        provider,
        model_name: model_name.to_string(),
        llm_ref: llm_ref.to_string(),
    });
}

/// 当前默认 (provider, model_name, llm_ref)；从未热切过为 None。
pub fn current() -> Option<(Arc<dyn LLMProvider>, String, String)> {
    slot().read().as_ref().map(|e| {
        (
            Arc::clone(&e.provider),
            e.model_name.clone(),
            e.llm_ref.clone(),
        )
    })
}

/// 把启动装配的 provider 包成「跟随默认」的委派 wrapper。
///
/// - `captured`：装配期 factory 产物（可为 NullProvider——用户配好模型并
///   set_default 后经槽热切立即恢复，无需重启 loop）。
/// - `captured_model` / `captured_ref`：装配时解析出的模型名两种形态
///   （`resolve_model_config` 的 model_name 与 provider 前缀化 llm_ref）。
pub fn default_following(
    captured: Arc<dyn LLMProvider>,
    captured_model: &str,
    captured_ref: &str,
) -> Arc<dyn LLMProvider> {
    Arc::new(DefaultFollowingProvider {
        captured,
        captured_model: captured_model.to_string(),
        captured_ref: captured_ref.to_string(),
    })
}

struct DefaultFollowingProvider {
    captured: Arc<dyn LLMProvider>,
    captured_model: String,
    captured_ref: String,
}

impl DefaultFollowingProvider {
    /// 路由判定：这次调用要的是「默认」还是「钉扎」。
    ///
    /// 判据必须**同时**比对捕获名与当前名：SSE/persona 传的是
    /// AppState.model_name（活槽文本，set_default 会更新），换型后传入的
    /// 是新名——只比捕获名会把「要默认」误判成钉扎。空模型名视为要默认。
    fn route(&self, model: &str) -> (Arc<dyn LLMProvider>, String) {
        if let Some((provider, cur_model, cur_ref)) = current()
            && (model.is_empty()
                || model == self.captured_model
                || model == self.captured_ref
                || model == cur_model
                || model == cur_ref)
        {
            return (provider, cur_model);
        }
        // 钉扎：provider 与模型名都不劫持，保持装配时语义（与历史行为逐
        // 字节一致）。空名兜底捕获名（ProviderAdapter 之外的直调者）。
        let model = if model.is_empty() {
            self.captured_model.clone()
        } else {
            model.to_string()
        };
        (Arc::clone(&self.captured), model)
    }
}

#[async_trait]
impl LLMProvider for DefaultFollowingProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let (provider, model) = self.route(model);
        provider.chat(messages, tools, &model, options).await
    }

    fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        model: &str,
        options: &ChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        let (provider, model) = self.route(model);
        provider.chat_stream(messages, tools, &model, options)
    }

    fn default_model(&self) -> &str {
        self.captured.default_model()
    }

    fn name(&self) -> &str {
        self.captured.name()
    }
}

#[cfg(test)]
mod tests;
