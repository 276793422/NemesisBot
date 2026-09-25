//! PricingTable 匹配语义补充测试（trim / 空串 / bare-suffix 情况说明）。
//!
//! 情况说明：`by_alias.get(bare)` 第 4 级匹配分支要求「某条目的 alias
//! 不等于任何条目的 model_id」——当前内嵌快照（build.rs 精简 + extras
//! 合并）里所有 alias 都与某个 model_id 重合，该分支在快照数据下
//! 不可达（第 3 级 by_id(bare) 先命中）。这里固化为文档测试：对快照里
//! 真实存在的 alias 走 `vendor/<alias>` 形态能命中（无论经第 3 还是
//! 第 4 级），下载层（PricingStore::lookup）的四级匹配由其自有测试覆盖。

use crate::pricing::PricingTable;

/// `vendor/<alias>` 形态命中：bare-suffix 匹配语义对配置里的
/// `provider/model` 名生效。
#[test]
fn lookup_bare_suffix_resolves_alias() {
    let table = PricingTable::embedded();
    let alias = table
        .entries()
        .iter()
        .flat_map(|e| e.aliases.iter())
        .find(|a| !a.is_empty() && !a.contains('/'))
        .expect("embedded table has an entry with an alias")
        .clone();

    let qualified = format!("some-vendor/{alias}");
    let hit = table.lookup(&qualified).expect("bare-suffix must resolve");
    assert!(
        hit.aliases.iter().any(|a| a == &alias) || hit.model_id == alias,
        "resolved entry should correspond to alias {alias} (got {})",
        hit.model_id
    );
}

/// 带首尾空白的输入 trim 后仍命中；纯空白 → None。
#[test]
fn lookup_trims_whitespace() {
    let table = PricingTable::embedded();
    let id = table.entries()[0].model_id.clone();
    let padded = format!("  {id}  ");
    assert!(table.lookup(&padded).is_some());
    assert!(table.lookup("   ").is_none());
}
