//! `config_port_to_u16` 端口归一语义（BUG-20261109-015 回归锁）：config
//! 端口字段是 i64（flexible 反序列化兼容历史字符串形态），越域值必须
//! loud 拒绝而非裸 `as u16` 静默回绕（70000→4464、-1→65535）。

use super::config_port_to_u16;

#[test]
fn in_range_ports_pass_through() {
    // 0（显式随机口语义）/ 默认端口 / 域上界全部直通。
    assert_eq!(config_port_to_u16(0, "f").unwrap(), 0);
    assert_eq!(config_port_to_u16(1, "f").unwrap(), 1);
    assert_eq!(config_port_to_u16(49000, "f").unwrap(), 49000);
    assert_eq!(config_port_to_u16(65535, "f").unwrap(), 65535);
}

#[test]
fn out_of_range_values_are_loudly_rejected() {
    // 负值裸 cast 会回绕成大端口（-1→65535）——必须 Err。
    let err = config_port_to_u16(-1, "gateway.port").unwrap_err();
    assert_eq!(err.field, "gateway.port");
    assert_eq!(err.value, -1);
    // 70000 裸 cast 会回绕成 4464（恰好穿过下游哨兵闸）——必须 Err。
    let err = config_port_to_u16(70000, "channels.web.port").unwrap_err();
    assert_eq!(err.field, "channels.web.port");
    assert_eq!(err.value, 70000);
    // i64 域上界同理。
    let err = config_port_to_u16(i64::MAX, "f").unwrap_err();
    assert_eq!(err.value, i64::MAX);
    // 错误 Display 自带 field 与值（手编 config 的诊断入口）。
    let msg = config_port_to_u16(70000, "channels.websocket.port")
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("channels.websocket.port"),
        "Display 应含字段路径: {msg}"
    );
    assert!(msg.contains("70000"), "Display 应含越域值: {msg}");
}

#[test]
fn error_implements_std_error_for_anyhow_propagation() {
    // 消费点（gateway 装配，anyhow::Result）靠 From<E: std::error::Error>
    // 盲转换传播——类型系统层面钉死该 impl 存在。
    fn assert_std_error<E: std::error::Error>(_: &E) {}
    assert_std_error(&config_port_to_u16(-1, "f").unwrap_err());
}
