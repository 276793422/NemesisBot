//! rate_limit 单测（#25 W1）：令牌桶容量/补给/并发闸/开关语义。

use super::*;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

fn limiter(qps: u32, burst: u32, max_streams: u32) -> GatewayRateLimiter {
    GatewayRateLimiter::new(nemesis_config::RateLimitSettings {
        enabled: true,
        qps,
        burst,
        max_concurrent_streams: max_streams,
    })
}

#[test]
fn burst_capacity_admits_then_exhausts() {
    let l = limiter(1, 3, 0);
    assert!(l.allow_request());
    assert!(l.allow_request());
    assert!(l.allow_request());
    // 桶空：即便 qps>0，瞬间再取必拒（补给依赖墙钟流逝）。
    assert!(!l.allow_request());
}

#[test]
fn refill_recovers_tokens_over_time() {
    let l = limiter(20, 2, 0); // qps=20 → 100ms 补给 2 令牌；桶容 2
    assert!(l.allow_request()); // 2→1
    assert!(l.allow_request()); // 1→0
    assert!(!l.allow_request(), "桶空后瞬时再取必拒");
    sleep(Duration::from_millis(120));
    assert!(l.allow_request(), "120ms@20qps 补给 ~2.4 → 桶容钳 2");
    assert!(l.allow_request());
    assert!(!l.allow_request(), "补给耗尽后再取必拒");
}

#[test]
fn disabled_limiter_passes_everything() {
    let l = GatewayRateLimiter::new(nemesis_config::RateLimitSettings {
        enabled: false,
        qps: 1,
        burst: 1,
        max_concurrent_streams: 1,
    });
    for _ in 0..50 {
        assert!(l.allow_request(), "disabled 不计费");
        assert!(l.try_enter_stream(), "disabled 并发闸不生效");
    }
    assert_eq!(l.inflight_streams(), 0, "disabled 入场不加计数");
}

#[test]
fn stream_concurrency_gate_counts_and_releases() {
    let l = Arc::new(limiter(100, 100, 2));
    assert!(l.try_enter_stream());
    assert!(l.try_enter_stream());
    assert!(!l.try_enter_stream(), "第 3 路并发超限");
    assert_eq!(l.inflight_streams(), 2);
    // 通过 StreamLease Drop 归还（模拟流结束/断连——middleware 侧租约
    // 挂在 body 流闭包环境里，流 drop 时走同一路径）。
    drop(StreamLease {
        limiter: Some(l.clone()),
    });
    assert_eq!(l.inflight_streams(), 1, "租约 drop 归还一路");
    assert!(l.try_enter_stream(), "归还后可再入场");
}

#[test]
fn zero_max_streams_means_unlimited() {
    let l = limiter(1, 1, 0);
    for _ in 0..10 {
        assert!(l.try_enter_stream(), "0 = 不限并发");
    }
    assert_eq!(l.inflight_streams(), 10);
}
