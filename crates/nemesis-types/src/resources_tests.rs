//! resources 模块契约测试——类型形状 / 注册表 / 三态语义。

use crate::resources::*;
use serde_json::{Value, json};

/// 最低配置的 Global provider 测试替身。
struct StubGlobal {
    meta: ProviderMeta,
    view: CategoryView,
}

impl GlobalResourceProvider for StubGlobal {
    fn meta(&self) -> ProviderMeta {
        self.meta
    }
    fn collect(&self, _ctx: &ResourceCtx) -> CategoryView {
        self.view.clone()
    }
}

/// 最低配置的 PerLoop provider 测试替身（顺带验证 loop_api 可达）。
struct StubPerLoop {
    meta: ProviderMeta,
}

impl PerLoopResourceProvider for StubPerLoop {
    fn meta(&self) -> ProviderMeta {
        self.meta
    }
    fn collect(&self, _ctx: &ResourceCtx, loop_api: &dyn SessionLoopApi) -> CategoryView {
        // 契约：聚合面 resolve 后必然下发 loop_api——这里消费它证明通路。
        let pend = loop_api.inbox_pending("agent:main:session:s1");
        CategoryView::ok(
            pend.0 + pend.1,
            json!({ "context": loop_api.context_status("agent:main:session:s1") }),
        )
    }
}

/// SessionLoopApi 测试替身。
struct StubLoopApi;

impl SessionLoopApi for StubLoopApi {
    fn context_status(&self, _session_key: &str) -> Option<Value> {
        Some(json!({ "pct": 42 }))
    }
    fn inbox_pending(&self, _session_key: &str) -> (usize, usize) {
        (1, 2)
    }
    fn work_timeline(&self, _session_key: &str) -> Option<Value> {
        None
    }
    fn queue_peek(&self, _session_key: &str) -> Option<Value> {
        None
    }
    fn checkpoint_summary(&self, _session_key: &str) -> Vec<CheckpointSummary> {
        vec![CheckpointSummary {
            turn: 3,
            time: "2026-11-17T00:00:00Z".into(),
            prompt: "p".into(),
            session: Some("agent:main:session:s1".into()),
        }]
    }
}

fn stub_meta(id: &'static str, order: u32, transient: bool) -> ProviderMeta {
    ProviderMeta::new(id, id, order, transient)
}

#[test]
fn provider_meta_const_new_and_copy() {
    let m = stub_meta("cron", 30, false);
    assert_eq!(m.category_id, "cron");
    assert_eq!(m.order, 30);
    assert!(!m.transient);
    // Copy 语义：赋值后原值仍可用。
    let m2 = m;
    assert_eq!(m.category_id, m2.category_id);
}

#[test]
fn category_view_three_states() {
    let ok = CategoryView::ok(3, json!({ "k": "v" }));
    assert_eq!(ok.status.as_str(), "ok");
    assert_eq!(ok.total, 3);

    let empty = CategoryView::empty();
    assert_eq!(empty.status.as_str(), "empty");
    assert_eq!(empty.total, 0);

    let un = CategoryView::unavailable("子系统未装配");
    assert_eq!(un.status.as_str(), "unavailable");
    assert_eq!(un.total, 0);
    assert_eq!(un.status.unavailable_reason(), Some("子系统未装配"));
    assert_eq!(empty.status.unavailable_reason(), None);
}

#[test]
fn category_view_serialization_roundtrip() {
    let ok = CategoryView::ok(1, json!({ "rows": [1, 2] }));
    let v = serde_json::to_value(&ok).unwrap();
    assert_eq!(v["status"]["kind"], "ok");
    assert_eq!(v["total"], 1);
    let back: CategoryView = serde_json::from_value(v).unwrap();
    assert_eq!(back, ok);

    let un = CategoryView::unavailable("理由");
    let v = serde_json::to_value(&un).unwrap();
    assert_eq!(v["status"]["kind"], "unavailable");
    assert_eq!(v["status"]["reason"], "理由");
}

#[test]
fn resource_provider_meta_delegates_both_variants() {
    let g = ResourceProvider::Global(Box::new(StubGlobal {
        meta: stub_meta("cron", 30, false),
        view: CategoryView::empty(),
    }));
    assert_eq!(g.meta().category_id, "cron");
    assert_eq!(g.meta().order, 30);

    let p = ResourceProvider::PerLoop(Box::new(StubPerLoop {
        meta: stub_meta("execution", 0, true),
    }));
    assert_eq!(p.meta().category_id, "execution");
    assert!(p.meta().transient);
}

#[test]
fn registry_preserves_insertion_order_and_reports_empty() {
    let mut reg = SessionResourceRegistry::new();
    assert!(reg.is_empty());

    reg.register(ResourceProvider::Global(Box::new(StubGlobal {
        meta: stub_meta("b_cat", 20, false),
        view: CategoryView::empty(),
    })))
    .register(ResourceProvider::PerLoop(Box::new(StubPerLoop {
        meta: stub_meta("a_cat", 10, true),
    })))
    .register(ResourceProvider::Global(Box::new(StubGlobal {
        meta: stub_meta("c_cat", 30, false),
        view: CategoryView::empty(),
    })));

    assert!(!reg.is_empty());
    // 保序 = 插入序（展示排序由响应侧按 meta.order 做，注册表不重排）。
    let ids: Vec<&str> = reg
        .providers()
        .iter()
        .map(|p| p.meta().category_id)
        .collect();
    assert_eq!(ids, vec!["b_cat", "a_cat", "c_cat"]);
    let orders: Vec<u32> = reg.providers().iter().map(|p| p.meta().order).collect();
    assert_eq!(orders, vec![20, 10, 30]);
}

#[test]
fn per_loop_provider_receives_loop_api_via_collect() {
    let p = StubPerLoop {
        meta: stub_meta("execution", 0, true),
    };
    let ctx = ResourceCtx {
        session_key: "agent:main:session:s1",
        bare_sid: "s1",
        workspace_root: std::path::Path::new("/tmp/ws"),
    };
    let view = p.collect(&ctx, &StubLoopApi);
    // (steer=1, follow_up=2) → total=3；context_status 透传进 summary。
    assert_eq!(view.total, 3);
    assert_eq!(view.summary["context"]["pct"], 42);
}

#[test]
fn checkpoint_summary_serialization_roundtrip() {
    let s = CheckpointSummary {
        turn: 7,
        time: "t".into(),
        prompt: "回退点".into(),
        session: Some("agent:main:session:s1".into()),
    };
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["turn"], 7);
    assert_eq!(v["session"], "agent:main:session:s1");
    let back: CheckpointSummary = serde_json::from_value(v).unwrap();
    assert_eq!(back.session.as_deref(), Some("agent:main:session:s1"));
}
