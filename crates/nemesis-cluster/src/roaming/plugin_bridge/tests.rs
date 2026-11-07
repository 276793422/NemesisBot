//! `effective_rendezvous_state` merge 规则钉死（观测性）：
//! 插件段缺席/旧插件 = 宿主快照原样；从未跑 tick = 无失败可报；
//! 最后动作失败 = 失败形态（registry_url 插件自带兜底链）；最后动作成功 =
//! 宿主快照（members_sink 同拍写入，成功语义未变）。

use super::{RegistrySyncStatus, effective_rendezvous_state};
use crate::roaming::rendezvous::RendezvousClientState;

fn host_state(ok: bool, sync_ms: u64, members: usize, err: &str) -> RendezvousClientState {
    RendezvousClientState {
        registry_url: "http://vps.example.com:3341".into(),
        last_sync_ok: ok,
        last_sync_ms: sync_ms,
        member_count: members,
        last_error: err.into(),
    }
}

fn plugin_status(
    url: &str,
    fail_streak: u64,
    last_error: &str,
    last_ok_ms: u64,
    last_fail_ms: u64,
    last_member_count: usize,
) -> RegistrySyncStatus {
    RegistrySyncStatus {
        registry_url: url.into(),
        fail_streak,
        last_error: last_error.into(),
        last_ok_ms,
        last_fail_ms,
        last_member_count,
    }
}

#[test]
fn plugin_absent_keeps_host_snapshot() {
    // 旧插件（无 registry_sync 段）/循环未挂 → 宿主快照原样（旧行为不回归）。
    let host = Some(host_state(true, 1_000, 4, ""));
    assert_eq!(effective_rendezvous_state(host.clone(), None), host);
    // 宿主快照缺席（循环未挂的 形态）也保持 None。
    assert_eq!(effective_rendezvous_state(None, None), None);
}

#[test]
fn plugin_never_ticked_keeps_host_snapshot() {
    // 刚 start 未到首拍（两毫秒位都为 0）= 无失败可报，宿主快照原样。
    let host = Some(host_state(true, 1_000, 4, ""));
    let p = plugin_status("http://vps.example.com:3341", 0, "", 0, 0, 0);
    assert_eq!(
        effective_rendezvous_state(host, Some(p.clone())),
        Some(host_state(true, 1_000, 4, ""))
    );
    // 宿主快照缺席（首启即失败前窗口）同样不产失败形态。
    assert_eq!(effective_rendezvous_state(None, Some(p)), None);
}

#[test]
fn last_action_success_keeps_host_snapshot() {
    // 最后动作成功（ok_ms >= fail_ms，含「曾经失败后恢复」形态）：失败过
    // （fail_ms>0、streak 清零）但已恢复 → 成功形态，宿主快照原样。
    let host = Some(host_state(true, 5_000, 4, ""));
    let p = plugin_status("http://vps.example.com:3341", 0, "", 5_000, 3_000, 4);
    assert_eq!(
        effective_rendezvous_state(host.clone(), Some(p)),
        host,
        "恢复后必须回到成功形态（否则失败徽标粘死）"
    );
}

#[test]
fn failure_after_success_uses_plugin_error_and_host_members() {
    // 先成功后失败：last_fail_ms > last_ok_ms → 失败形态；registry_url 插件
    // 自带、member_count 用宿主最后已知值、last_sync_ms 取失败时刻。
    let host = Some(host_state(true, 1_000, 4, ""));
    let p = plugin_status(
        "http://vps.example.com:3341",
        3,
        "register 被拒（HTTP 401）",
        1_000,
        9_000,
        4,
    );
    let got = effective_rendezvous_state(host, Some(p)).expect("失败形态必须产出");
    assert!(!got.last_sync_ok);
    assert_eq!(got.last_error, "register 被拒（HTTP 401）");
    assert_eq!(got.last_sync_ms, 9_000);
    assert_eq!(got.member_count, 4, "member_count 保留宿主最后已知值");
    assert_eq!(got.registry_url, "http://vps.example.com:3341");
}

#[test]
fn failure_from_birth_falls_back_to_plugin_fields() {
    // 从出生就失败（宿主快照 None）：registry_url 插件自带，member_count =
    // 插件 last_member_count（从未成功即 0）；registry_url 为空（异常形态）
    // 时兜底空串不猜。
    let p = plugin_status(
        "http://127.0.0.1:9",
        7,
        "register 失败: connect",
        0,
        12_000,
        0,
    );
    let got = effective_rendezvous_state(None, Some(p)).expect("失败形态必须产出");
    assert!(!got.last_sync_ok);
    assert_eq!(got.registry_url, "http://127.0.0.1:9");
    assert_eq!(got.member_count, 0);
    assert_eq!(got.last_sync_ms, 12_000);
    // 宿主快照在但插件 URL 为空（旧插件形态）：registry_url 回落宿主。
    let p2 = plugin_status("", 1, "register 失败", 0, 12_000, 0);
    let got2 = effective_rendezvous_state(Some(host_state(true, 1_000, 2, "")), Some(p2))
        .expect("失败形态必须产出");
    assert_eq!(
        got2.registry_url, "http://vps.example.com:3341",
        "插件 URL 空 = 宿主兜底"
    );
    assert_eq!(got2.member_count, 2);
}

#[test]
fn plugin_registry_sync_section_deserializes() {
    // 插件 registry_sync 段投影反序列化（缺字段 default——前向兼容）。
    let raw = serde_json::json!({
        "registry_url": "http://vps.example.com:3341",
        "fail_streak": 2,
        "last_error": "register 失败: timeout",
        "last_ok_ms": 1_000,
        "last_fail_ms": 31_000,
        "last_member_count": 2,
    });
    let p: RegistrySyncStatus = serde_json::from_value(raw).expect("段反序列化");
    assert_eq!(p.fail_streak, 2);
    assert_eq!(p.last_member_count, 2);
    // 旧插件无段：Full 形态解析 = None（宿主回落旧行为）。
    #[derive(serde::Deserialize)]
    struct Full {
        #[serde(default)]
        registry_sync: Option<RegistrySyncStatus>,
    }
    let f: Full = serde_json::from_str(r#"{"ok":true,"running":false}"#).expect("旧插件响应");
    assert!(f.registry_sync.is_none());
}

// ---------------------------------------------------------------------------
// （三期）：TrustGateDialer 吊销 veto + 放行透传
// ---------------------------------------------------------------------------

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{RoamingDialer, TrustGateDialer, extract_iroh_hex};
use crate::roaming::trust::TrustBook;

/// 记录型假 dialer：dial 计数 + 固定成功响应（veto 测试断言「零调用」）。
#[derive(Debug, Clone, Default)]
struct RecordingDialer {
    dial_calls: Arc<AtomicUsize>,
    close_calls: Arc<AtomicUsize>,
}

impl RoamingDialer for RecordingDialer {
    fn dial(
        &self,
        _addr: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, u64), String>> + Send>>
    {
        let calls = self.dial_calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok((45678, 7))
        })
    }

    fn close(&self, _conn_id: u64) {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// hex 提取四形态：裸 hex / @relay 完整形态 / 大写小写不归一（veto 层归一）/
/// 非 iroh 形态 None。
#[test]
fn extract_iroh_hex_forms() {
    let hex = "ab".repeat(32);
    assert_eq!(extract_iroh_hex(&format!("iroh:{hex}")).unwrap(), hex);
    assert_eq!(
        extract_iroh_hex(&format!("iroh:{hex}@https://relay.example.com")).unwrap(),
        hex
    );
    assert!(
        extract_iroh_hex("192.168.1.5:9500").is_none(),
        "TCP 形态无信任面"
    );
    assert!(extract_iroh_hex("iroh:").is_none(), "空 hex 不产条目");
    assert!(extract_iroh_hex("").is_none());
}

/// revoked 命中：诚实 Err + 内层拨号**零调用**（连接根本不建立）。
#[tokio::test]
async fn trust_gate_veto_blocks_inner_dial() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let revoked = "cc".repeat(32);
    let mut book = TrustBook::from_data(crate::roaming::trust::TrustBookData::default());
    book.revoke(&revoked);
    book.save(tmp.path()).expect("save");

    let inner = RecordingDialer::default();
    let gate = TrustGateDialer {
        workspace: tmp.path().to_path_buf(),
        inner: inner.clone(),
    };
    let err = gate
        .dial(&format!("iroh:{revoked}@https://relay.example.com"))
        .await
        .expect_err("revoked 必须 veto");
    assert!(err.contains("吊销"), "错误串应说明吊销语义: {err}");
    assert_eq!(inner.dial_calls.load(Ordering::SeqCst), 0, "内层零调用");
}

/// 非 revoked（unknown / 簿缺席）：透传内层（Ok 形态），close 直透传。
#[tokio::test]
async fn trust_gate_non_revoked_passthrough() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 激活簿 + revoked 别人：unknown 不 veto（出站臂只防主动撞吊销节点）。
    let mut book = TrustBook::from_data(crate::roaming::trust::TrustBookData::default());
    book.revoke(&"cc".repeat(32));
    book.save(tmp.path()).expect("save");

    let inner = RecordingDialer::default();
    let gate = TrustGateDialer {
        workspace: tmp.path().to_path_buf(),
        inner: inner.clone(),
    };
    let ok = gate.dial(&format!("iroh:{}", "dd".repeat(32))).await;
    assert_eq!(ok.unwrap(), (45678, 7), "unknown 透传");
    assert_eq!(inner.dial_calls.load(Ordering::SeqCst), 1);
    gate.close(7);
    assert_eq!(inner.close_calls.load(Ordering::SeqCst), 1, "close 直透传");

    // 簿缺席（regime 未激活）：同样透传。
    let tmp2 = tempfile::tempdir().expect("tempdir");
    let inner2 = RecordingDialer::default();
    let gate2 = TrustGateDialer {
        workspace: tmp2.path().to_path_buf(),
        inner: inner2.clone(),
    };
    let ok2 = gate2.dial(&format!("iroh:{}", "ee".repeat(32))).await;
    assert_eq!(ok2.unwrap(), (45678, 7), "簿缺席 = 未启用 = 放行");
    assert_eq!(inner2.dial_calls.load(Ordering::SeqCst), 1);
}

/// TCP 形态地址无信任面：永远透传（trust 簿只管辖 iroh 叠加层）。
#[tokio::test]
async fn trust_gate_tcp_form_has_no_trust_face() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut book = TrustBook::from_data(crate::roaming::trust::TrustBookData::default());
    book.revoke(&"cc".repeat(32));
    book.save(tmp.path()).expect("save");

    let inner = RecordingDialer::default();
    let gate = TrustGateDialer {
        workspace: tmp.path().to_path_buf(),
        inner: inner.clone(),
    };
    let ok = gate.dial("192.168.1.5:9500").await;
    assert_eq!(ok.unwrap(), (45678, 7), "TCP 形态不经 trust 闸");
    assert_eq!(inner.dial_calls.load(Ordering::SeqCst), 1);
}
