//! cors.rs AGT 覆盖率批次（2026-09-25）：CORSManager::new 成功路径上的
//! tracing::info! 字段求值（130）。tracing 宏在无 subscriber 时短路——
//! `path = %config_path.display()` 的 DisplayValue 构造（即 display() 调用
//! 本身）只有挂上 always-interest 订阅者才会执行；用 with_default 线程局部
//! 作用域订阅者确定性触发（不碰全局槽，防并行竞态）。
//!
//! 结构性豁免（见报告）：
//! - 328（cors_layer_from_manager 的 if cfg.allow_localhost 收括号）：体
//!   （324-327 push localhost 端口对）有正计数，lcov 收括号归因伪零。

use super::*;

/// 最小 INFO 级订阅者：默认 register_callsite = always()，字段求值必达；
/// 事件体 no-op（覆盖率只要求求值路径执行，不要求观测内容）。
struct AgtInfoSubscriber;

impl tracing::Subscriber for AgtInfoSubscriber {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.level() <= &tracing::Level::INFO
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

#[test]
fn agt_manager_new_logs_loaded_config_path() {
    let dir = tempfile::tempdir().unwrap();

    // 首建分支：配置不存在 → default 落盘，info! 字段在订阅者下求值。
    let fresh = dir.path().join("cors.agt.json");
    let mgr =
        tracing::subscriber::with_default(AgtInfoSubscriber, || CORSManager::new(&fresh).unwrap());
    assert!(fresh.exists(), "缺省配置应原子落盘");
    assert!(mgr.config().allow_localhost);

    // 已存在分支：load_from_file 同样在订阅者下走一遍。
    let mgr2 =
        tracing::subscriber::with_default(AgtInfoSubscriber, || CORSManager::new(&fresh).unwrap());
    assert_eq!(mgr2.list_origins(), Vec::<String>::new());
}
