//! （2026-11-03）配置热发布世代化：AgentLoop 的 config 消费
//! 面从「每读者独立直读盘」收敛为「持代引用」。
//!
//! 规格四要素的本面落点：
//! - **配置变更 = 构建新代 prepared snapshot（校验→临时名→原子改名发布）**：
//!   写入侧由 `nemesis_utils::write_file_atomic` 承担（config.json 全部保存点
//!   已同源，见 nemesis-config save_config 家族）；读取侧对应物是本文件的
//!   prepare（读盘+解析=校验）→ publish（写锁内 CAS 改名换代）。
//! - **消费者持代引用**：读者经 [`AgentLoop::config_generation`] 取
//!   `Arc<serde_json::Value>`——不可变快照，代内恒一致；每轮 ≈20+ 次磁盘读
//!   +JSON 解析收敛为每发布点一次。
//! - **在途准备被取代 = retryable**：prepare 在锁外做 IO（含基线代号），
//!   publish 时 CAS——基线已被并发发布者推进 = `Superseded`，调用方以最新
//!   基线重试（上限 3 次，仍败让到下个发布点自愈；赢家同内容 = 已收敛，
//!   收敛，不重试）。
//! - **执行开始钉代（嵌套不跨代）**：发布点唯一三处（`set_config_path`
//!   绑定强制 / round start `check_config_reload` 内容闸 / 模型切换
//!   `refresh_active_tier` 对账），读者**绝不发布**——round 内无发布点，
//!   嵌套工具链拿到的都是同一 Arc。
//!
//! 刻意偏差（旧「每读者独立直读」→ 新，均为改进）：盘上 config 解析失败/
//! 瞬时不可读 → 保留上一代（last-known-good；旧为各 reader 独立回落缺省，
//! 同轮新旧混杂）；config.json 被删 → 保留上一代 + 发布点 warn（旧为静默
//! 回落名字启发式 tier）；坏 JSON 常驻 → 每个发布点重试准备（文件修好立即
//! 恢复，代价是坏文件期间每轮一条 warn——异常态值得响）。

use super::*;

/// 已发布代：单调代号 + 发布时锚定的盘上原始字节 + 不可变内容快照。
pub(crate) struct ConfigGeneration {
    /// 单调代号（从 1 起；0 = 无代）。CAS 的比较基准。
    pub(crate) generation: u64,
    /// 发布时锚定的盘上原始字节（变更闸与 CAS 收敛的比对基准）。
    /// **内容比对不赌钟**：mtime 等值判定在 Windows 粗时钟（毫秒级刷新
    /// 粒度）下同 tick 两次写不可分辨，闸不睁 → 解禁/换档不生效
    ///（CI Windows /8/10 五测连红；proxy_keys
    /// 缓存同族同修）。config.json KB 级，每发布点一次直读比对成本可忽略。
    pub(crate) raw: Vec<u8>,
    /// 代内容（读者持 `Arc` 引用，代内恒一致）。
    pub(crate) value: std::sync::Arc<serde_json::Value>,
}

/// 在途准备产物（IO 在锁外完成后交 CAS 发布）。字段 pub(crate)——测试面
/// 直接构造（模拟并发赢家/输家）。
pub(crate) struct PreparedConfig {
    /// 准备完成时的现行代号（CAS 基线）。
    pub(crate) base_generation: u64,
    /// 准备所读的盘上原始字节（收敛比对基准，见 [`ConfigGeneration::raw`]）。
    pub(crate) raw: Vec<u8>,
    pub(crate) value: std::sync::Arc<serde_json::Value>,
}

/// 发布被取代（规格「在途准备被取代=retryable」）：基线代号已推进，本次
/// 准备基于陈旧基线，发布会被拒；调用方重试。
pub(crate) struct PublishSuperseded;

impl AgentLoop {
    /// 读者入口：当前已发布代的快照引用（`None` = standalone / 绑定后从未
    /// 发布成功——调用方回落各自缺省，与旧独立直读的失败回落同语义）。
    /// **零 stat、零发布、Arc clone**——round 内恒返回同一 Arc（钉代契约，
    /// 见模块文档）。
    pub(crate) fn config_generation(&self) -> Option<std::sync::Arc<serde_json::Value>> {
        self.config_gen.read().as_ref().map(|g| g.value.clone())
    }

    /// 当前代号（观测/测试面；0 = 无代）。
    pub(crate) fn config_generation_seq(&self) -> u64 {
        self.config_gen
            .read()
            .as_ref()
            .map(|g| g.generation)
            .unwrap_or(0)
    }

    /// 发布协议：读盘（BOM 同源剥离）→ 内容比对 → 解析（校验，锁外）→
    /// CAS 发布（内容闸内建，不赌钟—— Windows 粗时钟实证）。
    /// 返回 true = 本调用结果使现行代代表盘上最新内容（新发布或并发赢家
    /// 已收敛）；false = 闸未过/校验失败/重试耗尽（现行代保留）。
    ///
    /// `force`（绑定/重绑）跳过内容闸——路径本身换了，闸比较无意义。
    pub(crate) fn republish_config_generation(&self, force: bool) -> bool {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return false, // standalone：无 config 面，世代层不存在
        };
        // 重试上限：CAS 连败 = 高频并发发布者（正常运行不存在），让到下个
        // 发布点自愈（round start / 模型切换都会再对账）。
        for _attempt in 0..3 {
            // 内容闸：现行代锚定的原始字节未变 = 现行代仍是盘上真相。读取
            // 走 read_config_string（BOM 剥离语义与全部配置读取点同源）；
            // 读不到（缺文件/瞬时 IO）= 校验必败，直接走保留上一代臂（与
            // 旧「穿闸→prepare 失败」的终态同语义，少一次注定失败的解析）。
            let bytes = match nemesis_utils::fsutil::read_config_string(&path) {
                Ok(s) => s.into_bytes(),
                Err(_) => {
                    if self.config_gen.read().is_some() {
                        tracing::warn!(
                            path = %path.display(),
                            "[ConfigGen] config 读取失败，保留上一代（last-known-good，下个发布点重试）"
                        );
                    }
                    return false;
                }
            };
            if !force
                && let Some(g) = self.config_gen.read().as_ref()
                && g.raw == bytes
            {
                return false;
            }
            let prepared = match self.prepare_config_generation(bytes) {
                Ok(p) => p,
                Err(e) => {
                    // 校验失败：保留上一代（last-known-good）；无代 = 读者
                    // 回落各自缺省（绑定到缺失/坏文件的合法预备态，静默）。
                    if self.config_gen.read().is_some() {
                        tracing::warn!(
                            error = %e,
                            path = %path.display(),
                            "[ConfigGen] config 校验失败，保留上一代（last-known-good，下个发布点重试）"
                        );
                    }
                    return false;
                }
            };
            match self.publish_prepared_config(prepared) {
                Ok(true) => return true,
                Ok(false) => return true, // 并发赢家已发布同内容 = 收敛
                Err(PublishSuperseded) => continue, // retryable：以最新基线重试
            }
        }
        tracing::warn!("[ConfigGen] 连续被并发发布者取代，让到下个发布点（retryable，现行代保留）");
        false
    }

    /// 准备：原始字节 → JSON 解析（= 校验）。解析在锁外；基线代号在解析
    /// 成功后现取（尽量缩小被取代窗口；CAS 兜底竞态）。
    pub(crate) fn prepare_config_generation(&self, raw: Vec<u8>) -> Result<PreparedConfig, String> {
        let value: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|e| format!("config 解析失败: {e}"))?;
        let base_generation = self
            .config_gen
            .read()
            .as_ref()
            .map(|g| g.generation)
            .unwrap_or(0);
        Ok(PreparedConfig {
            base_generation,
            raw,
            value: std::sync::Arc::new(value),
        })
    }

    /// CAS 发布：现行代号 == 准备基线才换代（generation+1）；否则检查赢家
    /// 是否已发布同内容（收敛 = Ok(false)），未收敛 = Superseded。
    pub(crate) fn publish_prepared_config(
        &self,
        prepared: PreparedConfig,
    ) -> Result<bool, PublishSuperseded> {
        let mut slot = self.config_gen.write();
        let cur = slot.as_ref().map(|g| g.generation).unwrap_or(0);
        if cur != prepared.base_generation {
            if slot.as_ref().is_some_and(|g| g.raw == prepared.raw) {
                return Ok(false); // 赢家已发布同内容 = 收敛，无需再发布
            }
            return Err(PublishSuperseded);
        }
        *slot = Some(std::sync::Arc::new(ConfigGeneration {
            generation: cur + 1,
            raw: prepared.raw,
            value: prepared.value,
        }));
        Ok(true)
    }
}
