//! endpoint 投影单测（复查二轮②）：`NetObservation` serialize wire 形态
//! 钉死——`qad_v4` rename 是插件↔宿主两侧键名契约的插件侧锚。宿主桥侧
//! deserialize 对未知键**静默**兜 default false（无 loud 防御），键名漂移
//! 的防线 = 本测试（插件 wire 形态）+ 宿主侧 net_projection 三态测试
//! （plugin_bridge/tests.rs）——任一侧漂移即对应测试红。

use crate::endpoint::NetObservation;

/// wire 形态逐键钉死：`{"qad_v4": bool, "observed_v4": str|null}`——字段
/// 改名/rename 丢失即本测试红（否则宿主静默读 false，QAD 死活不可观测）。
#[test]
fn net_observation_serialize_wire_key_is_qad_v4() {
    let hit = NetObservation {
        udp_v4: true,
        observed_v4: Some("36.110.60.241:34287".to_string()),
    };
    assert_eq!(
        serde_json::to_value(&hit).expect("serialize"),
        serde_json::json!({"qad_v4": true, "observed_v4": "36.110.60.241:34287"}),
        "wire 键名 = qad_v4（rename 契约——插件侧形态与宿主投影同构）"
    );
    let miss = NetObservation {
        udp_v4: false,
        observed_v4: None,
    };
    assert_eq!(
        serde_json::to_value(&miss).expect("serialize"),
        serde_json::json!({"qad_v4": false, "observed_v4": null}),
        "未到账形态：observed_v4 = null（非缺键——宿主 Option 语义依赖显式 null/有值）"
    );
}
