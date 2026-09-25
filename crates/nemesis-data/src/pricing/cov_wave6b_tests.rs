//! pricing.rs 覆盖率收尾（Wave6B）：`all_pricing` 便捷包装（handlers 用）
//! 的执行。
//!
//! 情况说明（承接 cov_tests）：`by_alias.get(bare)` 第 4 级匹配（110 行）
//! 在内嵌快照数据下不可达（所有 alias 与某 model_id 重合，第 3 级
//! by_id(bare) 先命中），表构造器私有无法注入自定义数据——维持豁免。

use crate::pricing::all_pricing;

/// all_pricing 返回内嵌全表（116-118 entries() + 190-192 包装体）。
#[test]
fn all_pricing_returns_embedded_table() {
    let all = all_pricing();
    assert!(!all.is_empty(), "内嵌价目表不得为空");
    assert!(
        all.iter().all(|p| !p.model_id.is_empty()),
        "条目必须有 model_id"
    );
}
