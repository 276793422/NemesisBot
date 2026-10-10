//! MemoryStore 后端契约测试（S6）：双生产后端（LocalStore / TfIdfLocalStore）
//! 共享同一套语义契约，换后端不破语义。
//!
//! 本模块只提供**通用契约驱动**（泛型后端 + 断言），不含任何 `#[test]` 入口；
//! 各后端的实例化与挂接在 `store/tests.rs` 与 `local_store/tests.rs`。
//!
//! 契约边界（明确**不**锁的部分，属各后端本地语义，由各自测试文件钉死）：
//! - `store` 重复 id：LocalStore 追加（Vec 语义，同 id 双条目）、TfIdfLocalStore
//!   替换（Map 语义）——两族行为相反，更新入口请用 `update`。
//! - `query` / `list` 的 `limit = 0`：LocalStore 视为 take(0)（空结果）、
//!   TfIdfLocalStore 视为默认档（query→10 / list→不限）。
//! - `list` 排序：LocalStore 保持插入序、TfIdfLocalStore 按 created_at 倒序。
//! - `delete` 的归档 sidecar：TfIdfLocalStore 专属（archive-on-forget）。
//!
//! 所以本契约只断言两后端**必须一致**的面：roundtrip / query 匹配与排序 /
//! 类型过滤 / limit 截断（limit>0）/ update 原位替换与缺失即插入 / delete
//! 幂等性 / list 成员与分页。

use crate::store::MemoryStore;
use crate::types::{Entry, MemoryType};

/// 构造固定 id 的测试条目（契约需要可预测的 id 做增删改断言）。
fn entry_with_id(id: &str, typ: MemoryType, content: &str) -> Entry {
    let mut e = Entry::new(typ, content.to_string());
    e.id = id.to_string();
    e
}

/// store 返回条目自身 id；get 按 id 取回同一条目。
pub async fn contract_store_get_roundtrip<S: MemoryStore>(s: &S) {
    let e = entry_with_id("ct-1", MemoryType::LongTerm, "alpha beta unique");
    let id = s.store(e).await.unwrap();
    assert_eq!(id, "ct-1", "store 必须原样返回条目 id");
    let got = s.get("ct-1").await.unwrap().expect("取回应命中");
    assert_eq!(got.content, "alpha beta unique");
    assert_eq!(got.typ, MemoryType::LongTerm);
}

/// get 不存在的 id 返回 None（不是 Err）。
pub async fn contract_get_missing_is_none<S: MemoryStore>(s: &S) {
    assert!(s.get("ct-never").await.unwrap().is_none());
}

/// delete 命中返回 true 且条目从 get/query/list 消失；再删返回 false（幂等）。
pub async fn contract_delete_true_then_false<S: MemoryStore>(s: &S) {
    s.store(entry_with_id("ct-d", MemoryType::Daily, "deletable marker"))
        .await
        .unwrap();
    assert!(s.delete("ct-d").await.unwrap(), "首删必须 true");
    assert!(s.get("ct-d").await.unwrap().is_none());
    let res = s.query("deletable", None, 10).await.unwrap();
    assert_eq!(res.total, 0, "删除后 query 不得再命中");
    assert!(!s.delete("ct-d").await.unwrap(), "二删必须 false");
}

/// query：命中词匹配含该词的条目、不含则零命中；空 query / 纯标点 query 不炸。
pub async fn contract_query_match_and_miss<S: MemoryStore>(s: &S) {
    s.store(entry_with_id(
        "ct-q1",
        MemoryType::LongTerm,
        "kumquat preserve",
    ))
    .await
    .unwrap();
    s.store(entry_with_id(
        "ct-q2",
        MemoryType::LongTerm,
        "dragonfruit jam",
    ))
    .await
    .unwrap();

    let hit = s.query("kumquat", None, 10).await.unwrap();
    assert_eq!(hit.total, 1, "单词命中恰一条");
    assert_eq!(hit.entries[0].entry.id, "ct-q1");

    let miss = s.query("pomegranate", None, 10).await.unwrap();
    assert_eq!(miss.total, 0, "无交集词零命中");
    assert!(miss.entries.is_empty());

    for odd in ["", "!!!"] {
        let res = s.query(odd, None, 10).await.unwrap();
        assert_eq!(res.total, 0, "query {:?} 必须零命中不炸", odd);
    }
}

/// query 类型过滤：只返回指定 MemoryType 的条目。
pub async fn contract_query_type_filter<S: MemoryStore>(s: &S) {
    s.store(entry_with_id("ct-t1", MemoryType::LongTerm, "saffron rice"))
        .await
        .unwrap();
    s.store(entry_with_id("ct-t2", MemoryType::ShortTerm, "saffron tea"))
        .await
        .unwrap();

    let res = s
        .query("saffron", Some(MemoryType::ShortTerm), 10)
        .await
        .unwrap();
    assert_eq!(res.total, 1, "类型过滤后恰一条");
    assert_eq!(res.entries[0].entry.id, "ct-t2");
    assert_eq!(res.entries[0].entry.typ, MemoryType::ShortTerm);
}

/// query limit 截断（limit > 0）：entries 截到 limit，total 保留全量命中数。
pub async fn contract_query_limit_respected_total_kept<S: MemoryStore>(s: &S) {
    for i in 0..4 {
        s.store(entry_with_id(
            &format!("ct-l{i}"),
            MemoryType::LongTerm,
            "marigold field",
        ))
        .await
        .unwrap();
    }
    let res = s.query("marigold", None, 2).await.unwrap();
    assert_eq!(res.total, 4, "total 必须是全量命中数");
    assert_eq!(res.entries.len(), 2, "entries 截到 limit");
}

/// query 排序：全 token 重叠文档必须排在部分重叠文档之前（两后端共享的
/// 相关性下界——LocalStore 词重叠率 / TfIdf 余弦相似度在此场景同序）。
pub async fn contract_query_ranks_full_overlap_first<S: MemoryStore>(s: &S) {
    s.store(entry_with_id(
        "ct-r1",
        MemoryType::LongTerm,
        "obsidian shard",
    ))
    .await
    .unwrap();
    s.store(entry_with_id(
        "ct-r2",
        MemoryType::LongTerm,
        "obsidian forge",
    ))
    .await
    .unwrap();
    let res = s.query("obsidian shard", None, 10).await.unwrap();
    assert_eq!(res.total, 2);
    assert_eq!(
        res.entries[0].entry.id, "ct-r1",
        "双 token 全重叠文档必须第一"
    );
    assert!(
        res.entries[0].score > res.entries[1].score,
        "全重叠得分必须严格高于部分重叠"
    );
}

/// query 覆盖 tags：仅 tag 命中（正文不含查询词）也必须可检索。
pub async fn contract_query_tags_searchable<S: MemoryStore>(s: &S) {
    let mut e = entry_with_id("ct-g1", MemoryType::LongTerm, "unrelated body text");
    e.tags = vec!["zephyrblade".to_string()];
    s.store(e).await.unwrap();

    let res = s.query("zephyrblade", None, 10).await.unwrap();
    assert_eq!(res.total, 1, "tag 命中必须可检索");
    assert_eq!(res.entries[0].entry.id, "ct-g1");
}

/// update 原位替换：同 id 更新后 get 反映新内容、条目总数不变、旧内容不可再检索。
pub async fn contract_update_replaces_in_place<S: MemoryStore>(s: &S) {
    s.store(entry_with_id(
        "ct-u",
        MemoryType::LongTerm,
        "old quasar value",
    ))
    .await
    .unwrap();
    s.update(entry_with_id(
        "ct-u",
        MemoryType::LongTerm,
        "new pulsar value",
    ))
    .await
    .unwrap();

    let got = s.get("ct-u").await.unwrap().expect("更新后仍存在");
    assert_eq!(got.content, "new pulsar value");

    let all = s.list(None, 100, 0).await.unwrap();
    assert_eq!(all.len(), 1, "update 不得产生重复条目");

    let res = s.query("quasar", None, 10).await.unwrap();
    assert_eq!(res.total, 0, "旧内容不得再被检索到");
}

/// update 缺失即插入：对不存在的 id 执行 update 等价于 store。
pub async fn contract_update_missing_inserts<S: MemoryStore>(s: &S) {
    s.update(entry_with_id(
        "ct-ui",
        MemoryType::Daily,
        "inserted via update",
    ))
    .await
    .unwrap();
    assert!(s.get("ct-ui").await.unwrap().is_some());
    let all = s.list(None, 100, 0).await.unwrap();
    assert_eq!(all.len(), 1);
}

/// list：类型过滤只留目标类型；分页 skip/take 语义正确（不断言跨页顺序——
/// 两后端排序不同，见模块头注释）。
pub async fn contract_list_type_filter_and_pagination<S: MemoryStore>(s: &S) {
    s.store(entry_with_id("ct-p1", MemoryType::LongTerm, "long amber"))
        .await
        .unwrap();
    s.store(entry_with_id("ct-p2", MemoryType::ShortTerm, "short amber"))
        .await
        .unwrap();
    s.store(entry_with_id("ct-p3", MemoryType::LongTerm, "long jade"))
        .await
        .unwrap();

    let longs = s.list(Some(MemoryType::LongTerm), 100, 0).await.unwrap();
    assert_eq!(longs.len(), 2);
    assert!(longs.iter().all(|e| e.typ == MemoryType::LongTerm));

    let page1 = s.list(None, 2, 0).await.unwrap();
    assert_eq!(page1.len(), 2);
    let page2 = s.list(None, 2, 2).await.unwrap();
    assert_eq!(page2.len(), 1, "5 条内 3 条数据第二页剩 1");
    // 两页并起来必须恰好覆盖全量（成员面断言，不依赖顺序）。
    let mut ids: Vec<&str> = page1
        .iter()
        .chain(page2.iter())
        .map(|e| e.id.as_str())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["ct-p1", "ct-p2", "ct-p3"]);

    let beyond = s.list(None, 10, 99).await.unwrap();
    assert!(beyond.is_empty(), "offset 越界必须空");
}

/// list 空库：空结果不炸。
pub async fn contract_list_empty_store<S: MemoryStore>(s: &S) {
    assert!(s.list(None, 10, 0).await.unwrap().is_empty());
}

/// close 幂等成功。
pub async fn contract_close_ok<S: MemoryStore>(s: &S) {
    s.close().await.unwrap();
}
