//! 隐藏验收：同一 ContextV7 下，乱序到达的时间戳仍必须产出按生成顺序
//! 单调递增的 UUIDv7（跨毫秒回退 + 同毫秒内计数器两种形态都不得乱序）。
use uuid::{ContextV7, Timestamp, Uuid};

#[test]
fn v7_ordering_survives_out_of_order_timestamps() {
    let ctx = ContextV7::new();
    let a = Uuid::new_v7(Timestamp::from_unix(&ctx, 100, 0));
    let b = Uuid::new_v7(Timestamp::from_unix(&ctx, 99, 2_000_000));
    let c = Uuid::new_v7(Timestamp::from_unix(&ctx, 100, 1_000_000));
    assert!(a < b, "earlier-generated (older-second, later nanos) must still sort after: a={a} b={b}");
    assert!(b < c, "same-context ordering must stay monotonic: b={b} c={c}");
}
