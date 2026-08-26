//! S7 冲刺覆盖测试：systray.rs 的 tracing 惰性字段求值。
//!
//! `SystemTray::new` 里的 `tracing::info!(title = ..., menu_count = ...)`
//! 字段表达式只在事件被某个 subscriber 启用时才求值；测试环境默认没有
//! subscriber，所以那行字段表达式（含 `config.menu_items.len()`）从未
//! 执行。这里装一个启用一切的线程本地 subscriber 让它真正跑一遍。
//! （真托盘/真窗口创建路径属结构性不可达，见 S7 报告。）

use super::*;

#[test]
fn s7_system_tray_new_evaluates_lazy_tracing_fields() {
    struct EnableAllSubscriber;
    impl tracing::Subscriber for EnableAllSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
        fn event(&self, _event: &tracing::Event<'_>) {}
        fn enter(&self, _span: &tracing::Id) {}
        fn exit(&self, _span: &tracing::Id) {}
    }

    let _guard = tracing::subscriber::set_default(EnableAllSubscriber);

    let tray = SystemTray::new(TrayConfig::default());
    // 默认配置 5 个菜单项；同时 info! 的 menu_count 字段表达式已求值。
    assert_eq!(tray.menu_count(), 5);
}
