//! FilterChain Default / Debug / Rejected 日志路径补充测试。

use std::sync::Arc;

use super::{Filter, FilterChain, FilterDecision};
use nemesis_types::channel::InboundMessage;

fn dummy_msg(tag: &str) -> InboundMessage {
    InboundMessage {
        channel: "test".to_string(),
        sender_id: "tester".to_string(),
        chat_id: "chat-1".to_string(),
        content: tag.to_string(),
        media: Vec::new(),
        session_key: "agent:main:session:s1".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

/// Default impl 与 new 等价。
#[test]
fn default_chain_equals_new() {
    let chain: FilterChain<InboundMessage> = FilterChain::default();
    assert!(chain.list().is_empty());
}

/// Debug impl 免锁报规模（不触碰过滤器本体）。
#[test]
fn debug_reports_filter_count() {
    struct Noop;
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for Noop {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn priority(&self) -> i32 {
            0
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Pass
        }
    }
    let chain: FilterChain<InboundMessage> = FilterChain::new();
    assert_eq!(format!("{chain:?}"), "FilterChain { filters: 0 }");
    chain.attach(Arc::new(Noop));
    assert_eq!(format!("{chain:?}"), "FilterChain { filters: 1 }");
}

/// Rejected 决策：链终止、拒绝原因透传（触发 reason 格式化 + warn 日志
/// 路径；级别断言见下方 `rejected_decision_logs_warn_intercepted_stays_info`）。
#[tokio::test]
async fn rejected_decision_carries_reason_and_stops_chain() {
    struct Reject;
    struct After;
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for Reject {
        fn name(&self) -> &'static str {
            "rejector"
        }
        fn priority(&self) -> i32 {
            0
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Rejected("history disabled".to_string())
        }
    }
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for After {
        fn name(&self) -> &'static str {
            "after"
        }
        fn priority(&self) -> i32 {
            10
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Pass
        }
    }

    let chain: FilterChain<InboundMessage> = FilterChain::new();
    chain.attach(Arc::new(After));
    chain.attach(Arc::new(Reject));
    let decision = chain.run(&dummy_msg("x")).await;
    assert_eq!(
        decision,
        FilterDecision::Rejected("history disabled".to_string())
    );
}

/// BUG-20261102-321 红绿证：run 可观测性契约（docstring）承诺「Intercepted
/// 记 info、Rejected 记 warn」——修复前实现两分支都 info，warn+ 日志级别
/// 下链终止拒绝事件不可见。本测试以 WARN 级捕获 subscriber 验证：Rejected
/// 必须以 warn 出现、Intercepted 必须保持 info（不得一起升级）。
/// 子进程捕获模式同 `bus_cov_tests::subscribe_log_reports_post_subscription_total`
/// （进程内 tracing 全局 dispatch 已被测试 harness 占用，无法安全换装）。
#[test]
fn rejected_decision_logs_warn_intercepted_stays_info() {
    use std::sync::Mutex as StdMutex;

    struct Reject;
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for Reject {
        fn name(&self) -> &'static str {
            "rejector"
        }
        fn priority(&self) -> i32 {
            0
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Rejected("history disabled".to_string())
        }
    }
    struct Intercept;
    #[async_trait::async_trait]
    impl<T: Send + Sync> Filter<T> for Intercept {
        fn name(&self) -> &'static str {
            "interceptor"
        }
        fn priority(&self) -> i32 {
            0
        }
        async fn inspect(&self, _msg: &T) -> FilterDecision {
            FilterDecision::Intercepted
        }
    }

    if std::env::var_os("NB_FILTER_CAPLOG_CHILD").is_some() {
        struct SharedBuf(Arc<StdMutex<Vec<u8>>>);
        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Arc::new(StdMutex::new(Vec::new()));
        let writer_buf = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN) // 只收 warn+：info 必须被过滤
            .with_ansi(false)
            .with_writer(move || SharedBuf(writer_buf.clone()))
            .finish();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("child runtime");
        tracing::subscriber::with_default(subscriber, || {
            let rejected: FilterChain<InboundMessage> = FilterChain::new();
            rejected.attach(Arc::new(Reject));
            let _ = rt.block_on(rejected.run(&dummy_msg("r")));
            let intercepted: FilterChain<InboundMessage> = FilterChain::new();
            intercepted.attach(Arc::new(Intercept));
            let _ = rt.block_on(intercepted.run(&dummy_msg("i")));
        });
        println!(
            "NB_CAPLOG<<<{}>>>",
            String::from_utf8(buf.lock().unwrap().clone()).unwrap()
        );
        return;
    }

    // 父分支：拉起仅含本测试的子进程，校验捕获日志。
    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .args([
            "filter_cov_tests::rejected_decision_logs_warn_intercepted_stays_info",
            "--exact",
            "--nocapture",
        ])
        .env("NB_FILTER_CAPLOG_CHILD", "1")
        .output()
        .expect("spawn child test process");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let captured = stdout
        .split("NB_CAPLOG<<<")
        .nth(1)
        .and_then(|s| s.split(">>>").next())
        .unwrap_or_default();
    assert!(
        out.status.success(),
        "子进程测试失败：{}\n--- stdout ---\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Rejected 必须 warn 级可见（含拒绝原因）
    assert!(
        captured.contains("WARN") && captured.contains("rejected"),
        "Rejected 决策必须以 WARN 级（含 rejected 字样）出现在日志中\n--- captured ---\n{captured}"
    );
    assert!(
        captured.contains("history disabled"),
        "拒绝原因必须透传进日志\n--- captured ---\n{captured}"
    );
    // Intercepted 必须保持 info：WARN 级 subscriber 下不得出现
    assert!(
        !captured.contains("intercepted（消息未进入扇出）"),
        "Intercepted 决策不得升级为 warn（契约=info）\n--- captured ---\n{captured}"
    );
}
