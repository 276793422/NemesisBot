//! Bus Benchmarks — Concurrent + Performance benchmarks for nemesis-bus.
//!
//! Scenarios ported from Go test/performance suite:
//!   1. Concurrent Message Publishing  (10 publishers x 100 messages)
//!   2. Concurrent Subscribers         (5 subscribers x 20 messages)
//!   3. High Frequency Processing      (1000 messages with timing)
//!   4. Concurrent Mixed Operations    (5 publishers + 3 subscribers)
//!   5. Stress Test                    (10,000 messages)
//!   6. Burst Traffic                  (5 bursts x 100 messages)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemesis_bus::MessageBus;
use nemesis_types::channel::{InboundMessage, OutboundMessage};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_inbound(channel: &str, sender_id: &str, chat_id: &str, content: &str) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: format!("{}:{}", channel, chat_id),
        correlation_id: String::new(),
        metadata: HashMap::new(),
    }
}

fn make_outbound(channel: &str, chat_id: &str, content: &str) -> OutboundMessage {
    OutboundMessage {
        channel: channel.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        message_type: String::new(),
    }
}

struct BenchResult {
    name: String,
    total_messages: u64,
    elapsed: Duration,
    passed: bool,
    detail: String,
}

impl BenchResult {
    fn msgs_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs == 0.0 {
            return f64::INFINITY;
        }
        self.total_messages as f64 / secs
    }

    fn print(&self) {
        let status = if self.passed { "PASS" } else { "FAIL" };
        println!("  [{}] {}", status, self.name);
        println!("         Messages : {}", self.total_messages);
        println!("         Elapsed  : {:.3} ms", self.elapsed.as_secs_f64() * 1000.0);
        println!("         Throughput: {:.0} msg/s", self.msgs_per_sec());
        if !self.detail.is_empty() {
            println!("         Detail   : {}", self.detail);
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmark 1: Concurrent Message Publishing
//   10 publishers, 100 messages each (1000 total). Measure messages/second.
// ---------------------------------------------------------------------------

async fn bench_concurrent_publishing() -> BenchResult {
    const PUBLISHERS: usize = 10;
    const MSGS_PER_PUBLISHER: usize = 100;
    const TOTAL: usize = PUBLISHERS * MSGS_PER_PUBLISHER;

    // Use larger capacity to avoid drops during the burst.
    let bus = Arc::new(MessageBus::with_capacity(TOTAL + 256));

    // Subscribe *before* publishing so messages are not dropped.
    let mut rx = bus.subscribe_inbound();

    let start = Instant::now();
    let mut handles = Vec::with_capacity(PUBLISHERS);

    for pub_id in 0..PUBLISHERS {
        let b = bus.clone();
        handles.push(tokio::spawn(async move {
            for msg_id in 0..MSGS_PER_PUBLISHER {
                b.publish_inbound(make_inbound(
                    "bench",
                    &format!("pub-{}", pub_id),
                    "chat-1",
                    &format!("msg-{}-{}", pub_id, msg_id),
                ));
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Drain all messages from the receiver.
    let mut received = 0u64;
    for _ in 0..TOTAL {
        match rx.recv().await {
            Ok(_) => received += 1,
            Err(_) => break,
        }
    }

    let elapsed = start.elapsed();
    let passed = received == TOTAL as u64;

    BenchResult {
        name: "Concurrent Publishing (10 x 100)".to_string(),
        total_messages: received,
        elapsed,
        passed,
        detail: format!(
            "expected={}, received={}, dropped={}",
            TOTAL,
            received,
            bus.dropped_inbound()
        ),
    }
}

// ---------------------------------------------------------------------------
// Benchmark 2: Concurrent Subscribers
//   5 subscribers, 20 messages each. Track min/max/avg received.
// ---------------------------------------------------------------------------

async fn bench_concurrent_subscribers() -> BenchResult {
    const SUBSCRIBERS: usize = 5;
    const MSG_COUNT: usize = 20;

    let bus = Arc::new(MessageBus::with_capacity(MSG_COUNT + 256));

    // Each subscriber counts how many messages it receives.
    let counts: Vec<Arc<AtomicU64>> = (0..SUBSCRIBERS)
        .map(|_| Arc::new(AtomicU64::new(0)))
        .collect();

    let mut recv_handles = Vec::with_capacity(SUBSCRIBERS);

    for i in 0..SUBSCRIBERS {
        let mut rx = bus.subscribe_inbound();
        let counter = counts[i].clone();
        recv_handles.push(tokio::spawn(async move {
            for _ in 0..MSG_COUNT {
                match rx.recv().await {
                    Ok(_) => {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
            }
        }));
    }

    // Give receivers a moment to be ready.
    tokio::time::sleep(Duration::from_millis(10)).await;

    let start = Instant::now();
    for i in 0..MSG_COUNT {
        bus.publish_inbound(make_inbound(
            "bench",
            "sender",
            "chat",
            &format!("msg-{}", i),
        ));
    }

    // Wait for all receivers to finish.
    for h in recv_handles {
        let _ = h.await;
    }
    let elapsed = start.elapsed();

    let received_counts: Vec<u64> = counts.iter().map(|c| c.load(Ordering::Relaxed)).collect();
    let min = received_counts.iter().min().copied().unwrap_or(0);
    let max = received_counts.iter().max().copied().unwrap_or(0);
    let avg = received_counts.iter().sum::<u64>() as f64 / received_counts.len() as f64;

    // Broadcast: every subscriber should receive every message.
    let passed = min == MSG_COUNT as u64;

    BenchResult {
        name: "Concurrent Subscribers (5 x 20)".to_string(),
        total_messages: received_counts.iter().sum(),
        elapsed,
        passed,
        detail: format!(
            "min={}, max={}, avg={:.1}, expected_per_sub={}",
            min, max, avg, MSG_COUNT
        ),
    }
}

// ---------------------------------------------------------------------------
// Benchmark 3: High Frequency Processing
//   1000 messages with timing.
// ---------------------------------------------------------------------------

async fn bench_high_frequency() -> BenchResult {
    const MSG_COUNT: usize = 1000;

    let bus = Arc::new(MessageBus::with_capacity(MSG_COUNT + 256));
    let mut rx = bus.subscribe_inbound();

    // Receiver task: count messages.
    let received = Arc::new(AtomicU64::new(0));
    let recv_count = received.clone();
    let recv_handle = tokio::spawn(async move {
        for _ in 0..MSG_COUNT {
            match rx.recv().await {
                Ok(_) => {
                    recv_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    // Give receiver a moment to start.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let start = Instant::now();
    for i in 0..MSG_COUNT {
        bus.publish_inbound(make_inbound("bench", "sender", "chat", &format!("hf-{}", i)));
    }
    let publish_elapsed = start.elapsed();

    // Wait for receiver to finish.
    let _ = recv_handle.await;
    let total_elapsed = start.elapsed();

    let received_count = received.load(Ordering::Relaxed);
    let passed = received_count == MSG_COUNT as u64;

    BenchResult {
        name: "High Frequency Processing (1000 msg)".to_string(),
        total_messages: received_count,
        elapsed: total_elapsed,
        passed,
        detail: format!(
            "publish_only={:.3}ms, total={:.3}ms",
            publish_elapsed.as_secs_f64() * 1000.0,
            total_elapsed.as_secs_f64() * 1000.0,
        ),
    }
}

// ---------------------------------------------------------------------------
// Benchmark 4: Concurrent Mixed Operations
//   5 publishers + 3 subscribers concurrently, 50 messages per publisher.
//   The bus is broadcast, so each subscriber receives every message (250 total).
// ---------------------------------------------------------------------------

async fn bench_concurrent_mixed() -> BenchResult {
    const PUBLISHERS: usize = 5;
    const SUBSCRIBERS: usize = 3;
    const MSGS_PER_PUB: usize = 50;
    const TOTAL_PUBLISHED: usize = PUBLISHERS * MSGS_PER_PUB;
    const _TOTAL_EXPECTED: usize = SUBSCRIBERS * TOTAL_PUBLISHED;

    let bus = Arc::new(MessageBus::with_capacity(TOTAL_PUBLISHED + 256));

    // Track per-subscriber counts.
    let sub_counts: Vec<Arc<AtomicU64>> = (0..SUBSCRIBERS)
        .map(|_| Arc::new(AtomicU64::new(0)))
        .collect();

    // Spawn subscriber tasks.
    let mut sub_handles = Vec::with_capacity(SUBSCRIBERS);
    for i in 0..SUBSCRIBERS {
        let mut rx = bus.subscribe_inbound();
        let counter = sub_counts[i].clone();
        sub_handles.push(tokio::spawn(async move {
            for _ in 0..TOTAL_PUBLISHED {
                match rx.recv().await {
                    Ok(_) => {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
            }
        }));
    }

    // Give subscribers a moment to be ready.
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Spawn publisher tasks.
    let start = Instant::now();
    let mut pub_handles = Vec::with_capacity(PUBLISHERS);
    for pub_id in 0..PUBLISHERS {
        let b = bus.clone();
        pub_handles.push(tokio::spawn(async move {
            for msg_id in 0..MSGS_PER_PUB {
                b.publish_inbound(make_inbound(
                    "bench",
                    &format!("pub-{}", pub_id),
                    "chat",
                    &format!("mixed-{}-{}", pub_id, msg_id),
                ));
            }
        }));
    }

    // Wait for all publishers to finish.
    for h in pub_handles {
        h.await.unwrap();
    }

    // Wait for all subscribers to finish.
    for h in sub_handles {
        let _ = h.await;
    }
    let elapsed = start.elapsed();

    let received_per_sub: Vec<u64> = sub_counts
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .collect();
    let total_received: u64 = received_per_sub.iter().sum();
    let min = received_per_sub.iter().min().copied().unwrap_or(0);
    let max = received_per_sub.iter().max().copied().unwrap_or(0);

    // Each subscriber should have received exactly TOTAL_PUBLISHED messages.
    let passed = min == TOTAL_PUBLISHED as u64;

    BenchResult {
        name: "Concurrent Mixed (5 pub + 3 sub)".to_string(),
        total_messages: total_received,
        elapsed,
        passed,
        detail: format!(
            "published={}, per_sub=[{:?}], min={}, max={}, expected_per_sub={}",
            TOTAL_PUBLISHED,
            received_per_sub,
            min,
            max,
            TOTAL_PUBLISHED
        ),
    }
}

// ---------------------------------------------------------------------------
// Benchmark 5: Stress Test — 10,000 messages.
// ---------------------------------------------------------------------------

async fn bench_stress_test() -> BenchResult {
    const MSG_COUNT: usize = 10_000;

    let bus = Arc::new(MessageBus::with_capacity(MSG_COUNT + 512));
    let mut rx = bus.subscribe_inbound();

    // Receiver task.
    let received = Arc::new(AtomicU64::new(0));
    let recv_count = received.clone();
    let recv_handle = tokio::spawn(async move {
        for _ in 0..MSG_COUNT {
            match rx.recv().await {
                Ok(_) => {
                    recv_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    // Give receiver a moment.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let start = Instant::now();
    for i in 0..MSG_COUNT {
        bus.publish_inbound(make_inbound(
            "stress",
            "sender",
            "chat",
            &format!("stress-{}", i),
        ));
    }
    let publish_elapsed = start.elapsed();

    let _ = recv_handle.await;
    let total_elapsed = start.elapsed();

    let received_count = received.load(Ordering::Relaxed);
    let passed = received_count == MSG_COUNT as u64;

    BenchResult {
        name: "Stress Test (10,000 msg)".to_string(),
        total_messages: received_count,
        elapsed: total_elapsed,
        passed,
        detail: format!(
            "publish_only={:.3}ms, total={:.3}ms, dropped={}",
            publish_elapsed.as_secs_f64() * 1000.0,
            total_elapsed.as_secs_f64() * 1000.0,
            bus.dropped_inbound()
        ),
    }
}

// ---------------------------------------------------------------------------
// Benchmark 6: Burst Traffic
//   5 bursts of 100 messages each, 10ms between bursts.
// ---------------------------------------------------------------------------

async fn bench_burst_traffic() -> BenchResult {
    const BURSTS: usize = 5;
    const MSGS_PER_BURST: usize = 100;
    const TOTAL: usize = BURSTS * MSGS_PER_BURST;

    let bus = Arc::new(MessageBus::with_capacity(TOTAL + 256));
    let mut rx = bus.subscribe_inbound();

    // Receiver task.
    let received = Arc::new(AtomicU64::new(0));
    let recv_count = received.clone();
    let recv_handle = tokio::spawn(async move {
        for _ in 0..TOTAL {
            match rx.recv().await {
                Ok(_) => {
                    recv_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    // Give receiver a moment.
    tokio::time::sleep(Duration::from_millis(5)).await;

    let start = Instant::now();
    for burst_id in 0..BURSTS {
        for msg_id in 0..MSGS_PER_BURST {
            bus.publish_inbound(make_inbound(
                "burst",
                "sender",
                "chat",
                &format!("burst-{}-{}", burst_id, msg_id),
            ));
        }
        // Sleep between bursts (but not after the last one).
        if burst_id < BURSTS - 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let publish_elapsed = start.elapsed();

    let _ = recv_handle.await;
    let total_elapsed = start.elapsed();

    let received_count = received.load(Ordering::Relaxed);
    let passed = received_count == TOTAL as u64;

    BenchResult {
        name: "Burst Traffic (5 x 100, 10ms gap)".to_string(),
        total_messages: received_count,
        elapsed: total_elapsed,
        passed,
        detail: format!(
            "publish_only={:.3}ms, total={:.3}ms, dropped={}",
            publish_elapsed.as_secs_f64() * 1000.0,
            total_elapsed.as_secs_f64() * 1000.0,
            bus.dropped_inbound()
        ),
    }
}

// ---------------------------------------------------------------------------
// Bonus: Outbound bus benchmark
//   Verifies outbound path also performs well (1000 messages).
// ---------------------------------------------------------------------------

async fn bench_outbound_throughput() -> BenchResult {
    const MSG_COUNT: usize = 1000;

    let bus = Arc::new(MessageBus::with_capacity(MSG_COUNT + 256));
    let mut rx = bus.subscribe_outbound();

    let received = Arc::new(AtomicU64::new(0));
    let recv_count = received.clone();
    let recv_handle = tokio::spawn(async move {
        for _ in 0..MSG_COUNT {
            match rx.recv().await {
                Ok(_) => {
                    recv_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(5)).await;

    let start = Instant::now();
    for i in 0..MSG_COUNT {
        bus.publish_outbound(make_outbound(
            "bench",
            "chat",
            &format!("out-{}", i),
        ));
    }

    let _ = recv_handle.await;
    let elapsed = start.elapsed();

    let received_count = received.load(Ordering::Relaxed);
    let passed = received_count == MSG_COUNT as u64;

    BenchResult {
        name: "Outbound Throughput (1000 msg)".to_string(),
        total_messages: received_count,
        elapsed,
        passed,
        detail: format!(
            "throughput={:.0} msg/s, dropped={}",
            received_count as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
            bus.dropped_outbound()
        ),
    }
}

// ---------------------------------------------------------------------------
// Bonus: Bidirectional concurrent test
//   Inbound and outbound on the same bus simultaneously.
// ---------------------------------------------------------------------------

async fn bench_bidirectional() -> BenchResult {
    const INBOUND_COUNT: usize = 500;
    const OUTBOUND_COUNT: usize = 500;

    let bus = Arc::new(MessageBus::with_capacity(1024));

    // Inbound receiver.
    let inbound_received = Arc::new(AtomicU64::new(0));
    let in_count = inbound_received.clone();
    let mut in_rx = bus.subscribe_inbound();
    let in_handle = tokio::spawn(async move {
        for _ in 0..INBOUND_COUNT {
            match in_rx.recv().await {
                Ok(_) => {
                    in_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    // Outbound receiver.
    let outbound_received = Arc::new(AtomicU64::new(0));
    let out_count = outbound_received.clone();
    let mut out_rx = bus.subscribe_outbound();
    let out_handle = tokio::spawn(async move {
        for _ in 0..OUTBOUND_COUNT {
            match out_rx.recv().await {
                Ok(_) => {
                    out_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    tokio::time::sleep(Duration::from_millis(5)).await;

    let start = Instant::now();

    // Concurrent publishing on both directions.
    let bus_in = bus.clone();
    let bus_out = bus.clone();
    let (in_pub, out_pub) = tokio::join!(
        tokio::spawn(async move {
            for i in 0..INBOUND_COUNT {
                bus_in.publish_inbound(make_inbound(
                    "bench", "sender", "chat", &format!("in-{}", i),
                ));
            }
        }),
        tokio::spawn(async move {
            for i in 0..OUTBOUND_COUNT {
                bus_out.publish_outbound(make_outbound(
                    "bench", "chat", &format!("out-{}", i),
                ));
            }
        })
    );

    let _ = in_pub.unwrap();
    let _ = out_pub.unwrap();

    let _ = in_handle.await;
    let _ = out_handle.await;

    let elapsed = start.elapsed();
    let in_count = inbound_received.load(Ordering::Relaxed);
    let out_count = outbound_received.load(Ordering::Relaxed);
    let total = in_count + out_count;
    let passed = in_count == INBOUND_COUNT as u64 && out_count == OUTBOUND_COUNT as u64;

    BenchResult {
        name: "Bidirectional (500 in + 500 out)".to_string(),
        total_messages: total,
        elapsed,
        passed,
        detail: format!(
            "inbound={}/{}, outbound={}/{}, dropped_in={}, dropped_out={}",
            in_count,
            INBOUND_COUNT,
            out_count,
            OUTBOUND_COUNT,
            bus.dropped_inbound(),
            bus.dropped_outbound()
        ),
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    println!("=============================================================");
    println!("  NemesisBot Message Bus — Concurrent & Performance Benchmarks");
    println!("=============================================================");
    println!();

    let benchmarks: Vec<BenchResult> = vec![
        bench_concurrent_publishing().await,
        bench_concurrent_subscribers().await,
        bench_high_frequency().await,
        bench_concurrent_mixed().await,
        bench_stress_test().await,
        bench_burst_traffic().await,
        bench_outbound_throughput().await,
        bench_bidirectional().await,
    ];

    let mut all_passed = true;
    let mut total_messages: u64 = 0;

    for result in &benchmarks {
        result.print();
        println!();
        if !result.passed {
            all_passed = false;
        }
        total_messages += result.total_messages;
    }

    println!("-------------------------------------------------------------");
    println!("  Summary");
    println!("-------------------------------------------------------------");
    println!("  Benchmarks : {}", benchmarks.len());
    println!("  Total msg  : {}", total_messages);
    println!(
        "  Overall    : {}",
        if all_passed { "ALL PASSED" } else { "SOME FAILED" }
    );
    println!("-------------------------------------------------------------");

    if all_passed {
        println!();
        std::process::exit(0);
    } else {
        println!();
        std::process::exit(1);
    }
}
