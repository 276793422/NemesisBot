//! 分层价目表（A2 在线更新，2026-08-31）。
//!
//! 查表优先级：**用户自定义 > 下载表 > 内置表**（编译期嵌入的 36 模型表
//! 保持离线兜底）。落位约定（对齐 catalog-cache-workspace-data 布局：
//! workspace/data = 派生数据）：
//!
//! ```text
//! {data_dir}/pricing_custom.json               用户自定义条目（可增删改）
//! {data_dir}/model_prices_downloaded.json      在线下载的 LiteLLM 全量转换表
//! {data_dir}/model_prices.meta.json            下载元数据（etag/fetched_at/来源/条数）
//! ```
//!
//! 健壮性契约：任一文件缺失 → 该层为空；**损坏（非法 JSON）→ warn + 该层
//! 置空并继续**（断网/解析失败降级 = 保留现有表继续用旧数据，绝不因价目
//! 表问题拖垮计价/用量链路——成本只是可观测性数据）。写盘全部 tmp+rename
//! 原子替换。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::models::ModelPricing;
use crate::pricing::PricingTable;

const CUSTOM_FILE: &str = "pricing_custom.json";
const DOWNLOADED_FILE: &str = "model_prices_downloaded.json";
const META_FILE: &str = "model_prices.meta.json";

/// 下载元数据（ETag 增量请求 + UI 展示来源/时间）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PricingMeta {
    #[serde(default)]
    pub etag: Option<String>,
    /// Unix 秒。
    #[serde(default)]
    pub fetched_at: Option<i64>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub entry_count: usize,
}

/// 内层三件套 + 查表索引（每次变更整体重建——条目量级千级，重建廉价）。
struct Layers {
    custom: Vec<ModelPricing>,
    downloaded: Option<Vec<ModelPricing>>,
    /// 自定义 model_id → index into custom。
    custom_idx: HashMap<String, usize>,
    /// 自定义 alias → index into custom。
    custom_alias_idx: HashMap<String, usize>,
    /// 下载表 model_id/alias → index。
    downloaded_idx: HashMap<String, usize>,
}

impl Layers {
    fn rebuild(custom: Vec<ModelPricing>, downloaded: Option<Vec<ModelPricing>>) -> Self {
        let mut custom_idx = HashMap::new();
        let mut custom_alias_idx = HashMap::new();
        for (i, p) in custom.iter().enumerate() {
            custom_idx.insert(p.model_id.clone(), i);
            for a in &p.aliases {
                custom_alias_idx.insert(a.clone(), i);
            }
        }
        let mut downloaded_idx = HashMap::new();
        if let Some(entries) = &downloaded {
            for (i, p) in entries.iter().enumerate() {
                downloaded_idx.entry(p.model_id.clone()).or_insert(i);
                for a in &p.aliases {
                    downloaded_idx.entry(a.clone()).or_insert(i);
                }
            }
        }
        Self {
            custom,
            downloaded,
            custom_idx,
            custom_alias_idx,
            downloaded_idx,
        }
    }
}

/// 线程安全分层价目表。gateway/WSAPI/CLI 共享一个实例（DataStore 持有）。
pub struct PricingStore {
    data_dir: PathBuf,
    layers: RwLock<Layers>,
    meta: RwLock<PricingMeta>,
}

impl PricingStore {
    /// 打开（或初始化）分层价目表。`data_dir` 即 workspace/data 目录；
    /// 目录不存在自动创建。文件损坏按层降级（见模块注释契约）。
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| format!("pricing store create_dir_all: {e}"))?;

        let custom = std::fs::read_to_string(data_dir.join(CUSTOM_FILE))
            .ok()
            .and_then(|raw| match serde_json::from_str::<Vec<ModelPricing>>(&raw) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!("[PricingStore] {CUSTOM_FILE} 损坏，忽略自定义层: {e}");
                    None
                }
            })
            .unwrap_or_default();

        let downloaded = std::fs::read_to_string(data_dir.join(DOWNLOADED_FILE))
            .ok()
            .and_then(|raw| match serde_json::from_str::<Vec<ModelPricing>>(&raw) {
                Ok(v) if !v.is_empty() => Some(v),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!("[PricingStore] {DOWNLOADED_FILE} 损坏，忽略下载层: {e}");
                    None
                }
            });

        let meta = std::fs::read_to_string(data_dir.join(META_FILE))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            layers: RwLock::new(Layers::rebuild(custom, downloaded)),
            meta: RwLock::new(meta),
        })
    }

    /// 分层查表（查表优先级：自定义 > 下载 > 内置）。匹配语义与
    /// [`PricingTable::lookup`] 一致：model_id / alias / bare-suffix 四级。
    pub fn lookup(&self, model: &str) -> Option<ModelPricing> {
        let m = model.trim();
        if m.is_empty() {
            return None;
        }
        let bare = m.rsplit('/').next().unwrap_or(m);
        let layers = self.layers.read().map_err(|e| e.to_string()).ok()?;
        // 自定义层。
        for key in [m, bare] {
            if let Some(&i) = layers.custom_idx.get(key) {
                return Some(layers.custom[i].clone());
            }
            if let Some(&i) = layers.custom_alias_idx.get(key) {
                return Some(layers.custom[i].clone());
            }
        }
        // 下载层。
        for key in [m, bare] {
            if let Some(&i) = layers.downloaded_idx.get(key)
                && let Some(entries) = &layers.downloaded {
                    return Some(entries[i].clone());
                }
        }
        // 内置层（兜底，永在）。
        PricingTable::embedded().lookup(m).cloned()
    }

    /// 分层计价（[`crate::compute_cost_usd`] 的分层版）。查不到 → 0.0。
    pub fn compute_cost_usd(
        &self,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> f64 {
        match self.lookup(model) {
            Some(p) => crate::cost_from_pricing(
                &p,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            ),
            None => 0.0,
        }
    }

    /// 分层计价的分项版（A3 明细表用）。查不到 → `None`（调用方记
    /// 空 `pricing_model` + 全 0 分项——「未命中」与「命中免费条目」
    /// 在明细行上可区分）。
    pub fn compute_cost_breakdown(
        &self,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> Option<crate::CostBreakdown> {
        self.lookup(model).map(|p| {
            crate::cost_breakdown_from_pricing(
                &p,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            )
        })
    }

    /// 用户自定义条目（全量，UI 编辑列表用）。
    pub fn list_custom(&self) -> Vec<ModelPricing> {
        self.layers
            .read()
            .map(|l| l.custom.clone())
            .unwrap_or_default()
    }

    /// 下载表条目（全量，`None` = 尚未下载过）。
    pub fn list_downloaded(&self) -> Option<Vec<ModelPricing>> {
        self.layers.read().ok().and_then(|l| l.downloaded.clone())
    }

    /// 下载元数据快照。
    pub fn meta(&self) -> PricingMeta {
        self.meta.read().map(|m| m.clone()).unwrap_or_default()
    }

    /// 用下载结果整体替换下载层（全量替换——LiteLLM 表有增有删，diff 合并
    /// 反而留死条目）。tmp+rename 原子写；meta 同步更新。
    pub fn replace_downloaded(
        &self,
        entries: Vec<ModelPricing>,
        meta: PricingMeta,
    ) -> Result<(), String> {
        write_json_atomic(&self.data_dir.join(DOWNLOADED_FILE), &entries)?;
        write_json_atomic(&self.data_dir.join(META_FILE), &meta)?;

        let mut layers = self.layers.write().map_err(|e| e.to_string())?;
        *layers = Layers::rebuild(
            std::mem::take(&mut layers.custom),
            Some(entries),
        );
        *self.meta.write().map_err(|e| e.to_string())? = meta;
        Ok(())
    }

    /// 记录一次失败的下载尝试（ETag 保留旧值供下次增量；fetched_at 刷新
    /// 让 UI 能展示「上次尝试时间」）。只写 meta，不动数据层。
    pub fn record_failed_fetch(&self, source_url: &str) -> Result<(), String> {
        let mut meta = self.meta();
        meta.source_url = Some(source_url.to_string());
        meta.fetched_at = Some(chrono::Local::now().timestamp());
        write_json_atomic(&self.data_dir.join(META_FILE), &meta)?;
        *self.meta.write().map_err(|e| e.to_string())? = meta;
        Ok(())
    }

    /// 新增/更新自定义条目（按 model_id 幂等 upsert）。
    pub fn upsert_custom(&self, entry: ModelPricing) -> Result<(), String> {
        {
            let mut layers = self.layers.write().map_err(|e| e.to_string())?;
            let mut custom = std::mem::take(&mut layers.custom);
            match custom.iter_mut().find(|p| p.model_id == entry.model_id) {
                Some(slot) => *slot = entry,
                None => custom.push(entry),
            }
            write_json_atomic(&self.data_dir.join(CUSTOM_FILE), &custom)?;
            *layers = Layers::rebuild(custom, layers.downloaded.take());
        }
        Ok(())
    }

    /// 删除自定义条目。返回是否真的删了。
    pub fn remove_custom(&self, model_id: &str) -> Result<bool, String> {
        let mut layers = self.layers.write().map_err(|e| e.to_string())?;
        let custom = std::mem::take(&mut layers.custom);
        let before = custom.len();
        let custom: Vec<ModelPricing> =
            custom.into_iter().filter(|p| p.model_id != model_id).collect();
        let removed = custom.len() != before;
        if removed {
            write_json_atomic(&self.data_dir.join(CUSTOM_FILE), &custom)?;
            *layers = Layers::rebuild(custom, layers.downloaded.take());
        } else {
            // 没删东西也要把 take 走的还回去。
            *layers = Layers::rebuild(custom, layers.downloaded.take());
        }
        Ok(removed)
    }
}

/// tmp + rename 原子写（同目录 tmp 文件保证同分区 rename）。
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(value).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&tmp, raw).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    // Windows 上 rename 目标已存在会失败——先删（存在性容忍）。
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

#[cfg(test)]
mod tests;
