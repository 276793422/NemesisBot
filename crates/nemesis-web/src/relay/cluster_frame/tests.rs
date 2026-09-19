//! ClusterFrameSink 槽测试（批次六）。

use super::*;

/// 记录入参并回固定 payload 的 mock sink。
struct RecordingSink {
    seen: Mutex<Vec<(String, Value)>>,
    reply: Option<Value>,
}

impl ClusterFrameSink for RecordingSink {
    fn on_cluster_frame(
        &self,
        from_device: &str,
        payload: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send + '_>> {
        self.seen
            .lock()
            .expect("seen 锁中毒")
            .push((from_device.to_string(), payload.clone()));
        let reply = self.reply.clone();
        Box::pin(async move { reply })
    }
}

#[tokio::test]
async fn slot_round_trip_and_default_none() {
    // 缺省 None（一期形态：无 sink → 上行帧 WARN 忽略的判定依据）。
    let slot = ClusterFrameSinkSlot::default();
    assert!(slot.get().is_none());

    // set → get → 调用（Arc 克隆同源，入参回读）。
    let sink = Arc::new(RecordingSink {
        seen: Mutex::new(Vec::new()),
        reply: Some(serde_json::json!({"id": "r-1", "ok": true})),
    });
    slot.set(sink.clone());
    let got = slot.get().expect("set 后应可取回");
    let out = got
        .on_cluster_frame("bridge-x", serde_json::json!({"id": "q-1"}))
        .await;
    assert_eq!(out, Some(serde_json::json!({"id": "r-1", "ok": true})));
    // 值提取即 drop guard——clippy await_holding_lock（后续仍有 await）。
    let first = sink.seen.lock().unwrap().first().cloned();
    assert_eq!(
        first,
        Some(("bridge-x".to_string(), serde_json::json!({"id": "q-1"})))
    );

    // None 回复（宿主决定静默）也要透传。
    let quiet = Arc::new(RecordingSink {
        seen: Mutex::new(Vec::new()),
        reply: None,
    });
    slot.set(quiet);
    assert_eq!(
        slot.get()
            .unwrap()
            .on_cluster_frame("bridge-y", serde_json::json!(null))
            .await,
        None
    );
}
