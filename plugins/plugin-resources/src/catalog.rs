//! 内嵌目录装载：OUT_DIR 产物（build.rs 保证存在）→ 进程内索引。
//!
//! lazy 一次解析（OnceLock，~5MB JSON 毫秒级）；解析后全部不可变——
//! 任意线程并发读安全（无 roaming 式单缓冲串行纪律的必要：纯数据查询面）。
//! 解析失败（build.rs 校验被绕过的极端形态）= 诚实空目录：manifest 零资源
//! 条目（宿主 persona 消费臂据此 unavailable）、get 恒 not_found，不 NULL
//! 不崩。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::resource_filter::Catalog;

/// OUT_DIR 产物（build.rs 三路径唯一出口；文件名常量在 resource_filter，
/// include_str! 需字面量路径，此处重复字面量——drift 由 resource_filter::OUT_FILE
/// 单测钉死）。
const EMBEDDED: &str = include_str!(concat!(env!("OUT_DIR"), "/persona_catalog.json"));

pub(crate) struct LoadedCatalog {
    /// 路径 → md 全文。
    pub files: HashMap<String, String>,
    /// manifest JSON（prs_manifest 原样回吐；再序列化稳定形态）。
    pub manifest_json: String,
}

pub(crate) fn catalog() -> &'static LoadedCatalog {
    static C: OnceLock<LoadedCatalog> = OnceLock::new();
    C.get_or_init(|| match serde_json::from_str::<Catalog>(EMBEDDED) {
        Ok(c) => {
            let files = c
                .files
                .into_iter()
                .map(|f| (f.path, f.content))
                .collect::<HashMap<_, _>>();
            let manifest_json = serde_json::to_string(&c.manifest)
                .unwrap_or_else(|_| fallback_manifest("manifest re-serialize failed"));
            LoadedCatalog {
                files,
                manifest_json,
            }
        }
        Err(e) => LoadedCatalog {
            files: HashMap::new(),
            manifest_json: fallback_manifest(&format!("embedded catalog parse failed: {e}")),
        },
    })
}

/// 解析失败兜底 manifest（零资源条目——宿主消费臂诚实 unavailable）。
fn fallback_manifest(reason: &str) -> String {
    serde_json::json!({
        "abi": 1,
        "built_at_utc": "",
        "source": "baseline",
        "embed_source": reason,
        "resources": []
    })
    .to_string()
}
