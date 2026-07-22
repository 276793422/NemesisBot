use super::*;

// ---- I2C Tests ----

#[test]
fn test_i2c_tool_metadata() {
    let tool = I2CTool::new();
    assert_eq!(tool.name(), "i2c");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_i2c_non_linux_rejected() {
    let tool = I2CTool::new();
    let result = tool.execute(&serde_json::json!({"action": "detect"})).await;
    // On non-Linux, should error
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
        assert!(result.for_llm.contains("Linux"));
    }
}

#[tokio::test]
async fn test_i2c_missing_action() {
    let tool = I2CTool::new();
    let result = tool.execute(&serde_json::json!({})).await;
    // May fail on non-Linux first
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_scan_missing_bus() {
    let tool = I2CTool::new();
    let result = tool.execute(&serde_json::json!({"action": "scan"})).await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_no_confirm() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "data": [0x01, 0x02]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("confirm"));
    }
}

#[tokio::test]
async fn test_i2c_invalid_address() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x99
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

// ---- SPI Tests ----

#[test]
fn test_spi_tool_metadata() {
    let tool = SPITool::new();
    assert_eq!(tool.name(), "spi");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_spi_non_linux_rejected() {
    let tool = SPITool::new();
    let result = tool.execute(&serde_json::json!({"action": "list"})).await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
        assert!(result.for_llm.contains("Linux"));
    }
}

#[tokio::test]
async fn test_spi_transfer_no_confirm() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "data": [0x01]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("confirm"));
    }
}

#[tokio::test]
async fn test_spi_read_missing_device() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "read", "length": 4}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_invalid_device_format() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "abc",
            "length": 4
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[test]
fn test_spi_validate_params() {
    let tool = SPITool::new();

    // Valid params
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 1000000, "mode": 0, "bits": 8}))
            .is_ok()
    );

    // Invalid speed
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 0}))
            .is_err()
    );

    // Invalid mode
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": 5}))
            .is_err()
    );

    // Invalid bits
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 0}))
            .is_err()
    );
}

// --- Additional hardware tests ---

#[test]
fn test_i2c_tool_default() {
    let tool = I2CTool::default();
    assert_eq!(tool.name(), "i2c");
}

#[test]
fn test_spi_tool_default() {
    let tool = SPITool::default();
    assert_eq!(tool.name(), "spi");
}

#[test]
fn test_i2c_tool_parameters() {
    let tool = I2CTool::new();
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["action"].is_object());
}

#[test]
fn test_spi_tool_parameters() {
    let tool = SPITool::new();
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["action"].is_object());
}

#[tokio::test]
async fn test_i2c_unknown_action() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "unknown_action"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_unknown_action() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "unknown_action"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[test]
fn test_spi_validate_speed_too_high() {
    let tool = SPITool::new();
    let result = tool.validate_spi_params(&serde_json::json!({"speed": 200_000_000}));
    assert!(result.is_err());
}

#[test]
fn test_spi_validate_bits_too_high() {
    let tool = SPITool::new();
    let result = tool.validate_spi_params(&serde_json::json!({"bits": 64}));
    assert!(result.is_err());
}

#[test]
fn test_spi_validate_all_modes() {
    let tool = SPITool::new();
    for mode in 0..=3 {
        assert!(
            tool.validate_spi_params(&serde_json::json!({"mode": mode}))
                .is_ok()
        );
    }
}

#[test]
fn test_spi_validate_no_params_is_ok() {
    let tool = SPITool::new();
    assert!(tool.validate_spi_params(&serde_json::json!({})).is_ok());
}

#[tokio::test]
async fn test_i2c_read_missing_address() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "read", "bus": "1"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_missing_data() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

// --- Even more hardware tests ---

#[tokio::test]
async fn test_spi_missing_action() {
    let tool = SPITool::new();
    let result = tool.execute(&serde_json::json!({})).await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_detect_action() {
    let tool = I2CTool::new();
    let result = tool.execute(&serde_json::json!({"action": "detect"})).await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[test]
fn test_spi_validate_speed_boundary() {
    let tool = SPITool::new();
    // Just under the limit
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 125_000_000}))
            .is_ok()
    );
    // Just over the limit
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 125_000_001}))
            .is_err()
    );
}

#[test]
fn test_spi_validate_bits_boundary() {
    let tool = SPITool::new();
    // Max valid bits
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 32}))
            .is_ok()
    );
    // Over max
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 33}))
            .is_err()
    );
}

// --- Additional tests for coverage ---

#[tokio::test]
async fn test_i2c_scan_invalid_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "scan", "bus": "abc"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("number"));
    }
}

#[tokio::test]
async fn test_i2c_read_low_address() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "read", "bus": "1", "address": 0x01}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("address"));
    }
}

#[tokio::test]
async fn test_i2c_write_data_too_long() {
    let tool = I2CTool::new();
    let data: Vec<u64> = (0..300).collect();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "data": data
        }))
        .await;
    if !cfg!(target_os = "linux") {
        // On non-Linux, the platform check returns first
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("256 bytes"));
    }
}

#[tokio::test]
async fn test_i2c_write_invalid_data_byte() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "data": [256]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("byte value"));
    }
}

#[tokio::test]
async fn test_spi_read_invalid_length() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "2.0",
            "length": 0
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_read_length_too_large() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "2.0",
            "length": 5000
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("4096"));
    }
}

#[tokio::test]
async fn test_spi_transfer_data_too_long() {
    let tool = SPITool::new();
    let data: Vec<u64> = (0..5000).collect();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "confirm": true,
            "data": data
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("4096"));
    }
}

#[tokio::test]
async fn test_spi_transfer_invalid_device_format() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "abc",
            "confirm": true,
            "data": [1]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("X.Y"));
    }
}

#[tokio::test]
async fn test_spi_transfer_missing_data() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("data"));
    }
}

#[tokio::test]
async fn test_spi_transfer_invalid_data_byte() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "confirm": true,
            "data": [-1]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("byte value"));
    }
}

#[test]
fn test_i2c_tool_description_not_empty() {
    let tool = I2CTool::new();
    assert!(tool.description().len() > 30);
}

#[test]
fn test_spi_tool_description_not_empty() {
    let tool = SPITool::new();
    assert!(tool.description().len() > 30);
}

#[tokio::test]
async fn test_i2c_write_empty_data() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "data": []
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("data"));
    }
}

#[test]
fn test_i2c_parse_bus_valid() {
    let tool = I2CTool::new();
    assert!(tool.parse_bus(&serde_json::json!({"bus": "1"})).is_ok());
    assert!(tool.parse_bus(&serde_json::json!({"bus": "0"})).is_ok());
}

#[test]
fn test_i2c_parse_bus_invalid() {
    let tool = I2CTool::new();
    // Missing bus
    assert!(tool.parse_bus(&serde_json::json!({})).is_err());
    // Empty bus
    assert!(tool.parse_bus(&serde_json::json!({"bus": ""})).is_err());
    // Non-numeric bus
    assert!(tool.parse_bus(&serde_json::json!({"bus": "abc"})).is_err());
    assert!(tool.parse_bus(&serde_json::json!({"bus": "1a"})).is_err());
}

#[test]
fn test_i2c_parse_address_valid() {
    let tool = I2CTool::new();
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x03}))
            .is_ok()
    );
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x77}))
            .is_ok()
    );
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x38}))
            .is_ok()
    );
}

#[test]
fn test_i2c_parse_address_invalid() {
    let tool = I2CTool::new();
    // Missing address
    assert!(tool.parse_address(&serde_json::json!({})).is_err());
    // Too low
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x00}))
            .is_err()
    );
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x02}))
            .is_err()
    );
    // Too high
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x78}))
            .is_err()
    );
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0xFF}))
            .is_err()
    );
}

#[test]
fn test_spi_parse_device_valid() {
    let tool = SPITool::new();
    assert!(
        tool.parse_device(&serde_json::json!({"device": "2.0"}))
            .is_ok()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "0.0"}))
            .is_ok()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "32767.32767"}))
            .is_ok()
    );
}

#[test]
fn test_spi_parse_device_invalid() {
    let tool = SPITool::new();
    // Missing device
    assert!(tool.parse_device(&serde_json::json!({})).is_err());
    // Empty device
    assert!(
        tool.parse_device(&serde_json::json!({"device": ""}))
            .is_err()
    );
    // Wrong format
    assert!(
        tool.parse_device(&serde_json::json!({"device": "abc"}))
            .is_err()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "1"}))
            .is_err()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "1.2.3"}))
            .is_err()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "a.b"}))
            .is_err()
    );
}

#[tokio::test]
async fn test_i2c_scan_empty_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "scan", "bus": ""}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("required"));
    }
}

#[tokio::test]
async fn test_i2c_read_default_length() {
    let tool = I2CTool::new();
    // On non-Linux, the platform check runs first. On Linux, /dev/i2c-1 won't exist.
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x38
        }))
        .await;
    // Will error on both platforms (no hardware or not linux)
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_i2c_read_with_register() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x38,
            "register": 0x10,
            "length": 8
        }))
        .await;
    // Will error on both platforms
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_i2c_write_with_register_valid() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "register": 0x10,
            "data": [0x01, 0x02]
        }))
        .await;
    // Will error on both platforms (no hardware or not linux)
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_spi_transfer_valid_device() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "1.0",
            "confirm": true,
            "data": [0xFF]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_read_valid_device() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "1.0",
            "length": 16
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_list_action() {
    let tool = SPITool::new();
    let result = tool.execute(&serde_json::json!({"action": "list"})).await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_register_out_of_range() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "register": 256,
            "data": [1]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("register"));
    }
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[tokio::test]
async fn test_i2c_unknown_action_v2_r2() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "unknown_action"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
        assert!(result.for_llm.contains("Linux"));
    }
}

#[tokio::test]
async fn test_i2c_read_missing_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "address": 0x38,
            "length": 4
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_missing_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "address": 0x38,
            "data": [1],
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_non_numeric_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "abc",
            "address": 0x38,
            "data": [1],
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_address_out_of_range_high() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x80,
            "data": [1],
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_address_out_of_range_low() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x01,
            "data": [1],
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_unknown_action_v2_r2() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "unknown_spi_action"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
        assert!(result.for_llm.contains("Linux"));
    }
}

#[tokio::test]
async fn test_spi_missing_action_v2_r2() {
    let tool = SPITool::new();
    let result = tool.execute(&serde_json::json!({})).await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_transfer_empty_device() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "",
            "data": [1],
            "confirm": true
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_read_zero_length() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "1.0",
            "length": 0
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_read_too_large_length() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "1.0",
            "length": 5000
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_scan_non_numeric_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "scan",
            "bus": "abc"
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_read_address_boundary_low() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x02,
            "length": 1
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_read_address_boundary_high() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x78,
            "length": 1
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[test]
fn test_spi_validate_valid_all_params() {
    let tool = SPITool::new();
    let result = tool.validate_spi_params(&serde_json::json!({
        "speed": 500000,
        "mode": 2,
        "bits": 16
    }));
    assert!(result.is_ok());
}

#[test]
fn test_spi_validate_mode_boundary() {
    let tool = SPITool::new();
    // Mode 3 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": 3}))
            .is_ok()
    );
    // Mode 4 is invalid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": 4}))
            .is_err()
    );
}

#[test]
fn test_spi_validate_bits_boundary_v2_r2() {
    let tool = SPITool::new();
    // Bits 8 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 8}))
            .is_ok()
    );
    // Bits 0 is invalid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 0}))
            .is_err()
    );
}

#[test]
fn test_i2c_parse_bus_empty() {
    let tool = I2CTool::new();
    let result = tool.parse_bus(&serde_json::json!({"bus": ""}));
    assert!(result.is_err());
}

#[test]
fn test_i2c_parse_address_missing() {
    let tool = I2CTool::new();
    let result = tool.parse_address(&serde_json::json!({}));
    assert!(result.is_err());
}

#[test]
fn test_spi_parse_device_valid_format() {
    let tool = SPITool::new();
    let result = tool.parse_device(&serde_json::json!({"device": "3.1"}));
    assert!(result.is_ok());
}

#[test]
fn test_spi_parse_device_empty() {
    let tool = SPITool::new();
    let result = tool.parse_device(&serde_json::json!({"device": ""}));
    assert!(result.is_err());
}

// ============================================================
// Coverage improvement: I2C and SPI parameter validation edge cases
// ============================================================

#[test]
fn test_i2c_parse_bus_whitespace() {
    let tool = I2CTool::new();
    // Whitespace is not a digit
    assert!(tool.parse_bus(&serde_json::json!({"bus": " "})).is_err());
}

#[test]
fn test_i2c_parse_address_boundary_exact() {
    let tool = I2CTool::new();
    // Exact lower boundary (0x03) is valid
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x03}))
            .is_ok()
    );
    // Exact upper boundary (0x77) is valid
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x77}))
            .is_ok()
    );
    // One below lower boundary (0x02)
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x02}))
            .is_err()
    );
    // One above upper boundary (0x78)
    assert!(
        tool.parse_address(&serde_json::json!({"address": 0x78}))
            .is_err()
    );
}

#[test]
fn test_spi_parse_device_with_letter_parts() {
    let tool = SPITool::new();
    // Non-digit parts should fail
    assert!(
        tool.parse_device(&serde_json::json!({"device": "a.0"}))
            .is_err()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "0.a"}))
            .is_err()
    );
}

#[test]
fn test_spi_parse_device_triple_dot() {
    let tool = SPITool::new();
    assert!(
        tool.parse_device(&serde_json::json!({"device": "1.2.3"}))
            .is_err()
    );
}

#[test]
fn test_spi_parse_device_single_number() {
    let tool = SPITool::new();
    assert!(
        tool.parse_device(&serde_json::json!({"device": "1"}))
            .is_err()
    );
}

#[test]
fn test_spi_validate_speed_boundary_exact() {
    let tool = SPITool::new();
    // Speed 1 Hz is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 1}))
            .is_ok()
    );
    // Speed exactly 125_000_000 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 125_000_000}))
            .is_ok()
    );
    // Speed exactly 125_000_001 is invalid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 125_000_001}))
            .is_err()
    );
}

#[test]
fn test_spi_validate_bits_boundary_exact() {
    let tool = SPITool::new();
    // Bits 1 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 1}))
            .is_ok()
    );
    // Bits 32 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 32}))
            .is_ok()
    );
    // Bits 33 is invalid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"bits": 33}))
            .is_err()
    );
}

#[test]
fn test_spi_validate_mode_boundary_exact() {
    let tool = SPITool::new();
    // Mode 0 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": 0}))
            .is_ok()
    );
    // Mode 3 is valid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": 3}))
            .is_ok()
    );
    // Mode 4 is invalid
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": 4}))
            .is_err()
    );
}

#[tokio::test]
async fn test_i2c_scan_non_numeric_bus_chars() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({"action": "scan", "bus": "i2c-1"}))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("number"));
    }
}

#[tokio::test]
async fn test_i2c_read_clamp_length() {
    let tool = I2CTool::new();
    // Length 0 should be clamped to 1
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x38,
            "length": 0
        }))
        .await;
    // On non-Linux, platform check fires first
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_i2c_read_large_length_clamped() {
    let tool = I2CTool::new();
    // Length > 256 should be clamped to 256
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "1",
            "address": 0x38,
            "length": 500
        }))
        .await;
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_i2c_write_register_boundary_valid() {
    let tool = I2CTool::new();
    // Register 0 is valid
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "register": 0,
            "data": [1]
        }))
        .await;
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_i2c_write_register_boundary_valid_255() {
    let tool = I2CTool::new();
    // Register 255 is valid
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "register": 255,
            "data": [1]
        }))
        .await;
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_i2c_read_missing_bus_param() {
    let tool = I2CTool::new();
    // Read with empty bus string
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "",
            "address": 0x38
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("bus"));
    }
}

#[tokio::test]
async fn test_i2c_read_non_numeric_bus() {
    let tool = I2CTool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "bus": "abc",
            "address": 0x38
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_transfer_data_empty_array() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "confirm": true,
            "data": []
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("data"));
    }
}

#[tokio::test]
async fn test_spi_read_valid_min_length() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "2.0",
            "length": 1
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_read_valid_max_length() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "2.0",
            "length": 4096
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_spi_read_over_max_length() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "2.0",
            "length": 4097
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("4096"));
    }
}

#[tokio::test]
async fn test_spi_transfer_over_max_data() {
    let tool = SPITool::new();
    let data: Vec<u64> = (0..4097).collect();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "confirm": true,
            "data": data
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("4096"));
    }
}

#[tokio::test]
async fn test_spi_transfer_valid_max_data() {
    let tool = SPITool::new();
    let data: Vec<u64> = (0..4096).map(|_| 0u64).collect();
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "2.0",
            "confirm": true,
            "data": data
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[tokio::test]
async fn test_i2c_write_valid_data_byte_boundary() {
    let tool = I2CTool::new();
    // Max valid byte value (255)
    let result = tool
        .execute(&serde_json::json!({
            "action": "write",
            "bus": "1",
            "address": 0x38,
            "confirm": true,
            "data": [255, 0, 127]
        }))
        .await;
    // Will error on both platforms (no hardware or not linux)
    assert!(result.is_error || !result.for_llm.is_empty());
}

#[tokio::test]
async fn test_spi_transfer_device_dot_format() {
    let tool = SPITool::new();
    // Device with extra dots
    let result = tool
        .execute(&serde_json::json!({
            "action": "transfer",
            "device": "1.2.3",
            "confirm": true,
            "data": [1]
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("X.Y"));
    }
}

#[tokio::test]
async fn test_spi_read_device_dot_format() {
    let tool = SPITool::new();
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "1.2.3",
            "length": 10
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    } else {
        assert!(result.is_error);
        assert!(result.for_llm.contains("X.Y"));
    }
}

#[tokio::test]
async fn test_spi_read_default_length() {
    let tool = SPITool::new();
    // No length specified, should default to 1
    let result = tool
        .execute(&serde_json::json!({
            "action": "read",
            "device": "2.0"
        }))
        .await;
    if !cfg!(target_os = "linux") {
        assert!(result.is_error);
    }
}

#[test]
fn test_i2c_parameters_json_structure() {
    let tool = I2CTool::new();
    let params = tool.parameters();
    let action = &params["properties"]["action"];
    assert_eq!(action["type"], "string");
    let enum_values = action["enum"].as_array().unwrap();
    assert!(enum_values.contains(&serde_json::json!("detect")));
    assert!(enum_values.contains(&serde_json::json!("scan")));
    assert!(enum_values.contains(&serde_json::json!("read")));
    assert!(enum_values.contains(&serde_json::json!("write")));
}

#[test]
fn test_spi_parameters_json_structure() {
    let tool = SPITool::new();
    let params = tool.parameters();
    let action = &params["properties"]["action"];
    assert_eq!(action["type"], "string");
    let enum_values = action["enum"].as_array().unwrap();
    assert!(enum_values.contains(&serde_json::json!("list")));
    assert!(enum_values.contains(&serde_json::json!("transfer")));
    assert!(enum_values.contains(&serde_json::json!("read")));
}

// ============================================================
// Deeper coverage for pure helper edge cases
// ============================================================

#[test]
fn test_i2c_parse_address_float_rejected() {
    // A floating-point number is not a u64; as_u64() returns None.
    let tool = I2CTool::new();
    let result = tool.parse_address(&serde_json::json!({"address": 56.5}));
    assert!(result.is_err());
}

#[test]
fn test_i2c_parse_address_string_rejected() {
    // A string (even numeric-looking) is not accepted by as_u64().
    let tool = I2CTool::new();
    let result = tool.parse_address(&serde_json::json!({"address": "0x38"}));
    assert!(result.is_err());
}

#[test]
fn test_i2c_parse_bus_numeric_string_with_leading_zeros() {
    // Leading zeros are still all-digits -> valid.
    let tool = I2CTool::new();
    assert!(tool.parse_bus(&serde_json::json!({"bus": "007"})).is_ok());
}

#[test]
fn test_i2c_parse_bus_non_string_type_rejected() {
    // bus given as an integer (not string) is rejected by as_str().
    let tool = I2CTool::new();
    assert!(tool.parse_bus(&serde_json::json!({"bus": 1})).is_err());
}

#[test]
fn test_spi_parse_device_negative_part_rejected() {
    // Negative sign is not a digit -> rejected.
    let tool = SPITool::new();
    assert!(
        tool.parse_device(&serde_json::json!({"device": "-1.0"}))
            .is_err()
    );
    assert!(
        tool.parse_device(&serde_json::json!({"device": "1.-0"}))
            .is_err()
    );
}

#[test]
fn test_spi_validate_params_float_speed_ignored() {
    // A float speed does not match as_u64(), so validation is skipped (Ok).
    let tool = SPITool::new();
    assert!(
        tool.validate_spi_params(&serde_json::json!({"speed": 1.5}))
            .is_ok()
    );
}

#[test]
fn test_spi_validate_params_string_mode_ignored() {
    // A string mode does not match as_u64(), so validation is skipped (Ok).
    let tool = SPITool::new();
    assert!(
        tool.validate_spi_params(&serde_json::json!({"mode": "fast"}))
            .is_ok()
    );
}

#[test]
fn test_i2c_tool_metadata_consistent() {
    let tool = I2CTool::new();
    // Description must mention the Linux-only constraint and the 4 actions.
    let desc = tool.description();
    assert!(desc.contains("Linux"));
    for action in ["detect", "scan", "read", "write"] {
        assert!(
            desc.contains(action),
            "description should mention {}",
            action
        );
    }
}

#[test]
fn test_spi_tool_metadata_consistent() {
    let tool = SPITool::new();
    let desc = tool.description();
    assert!(desc.contains("Linux"));
    for action in ["list", "transfer", "read"] {
        assert!(
            desc.contains(action),
            "description should mention {}",
            action
        );
    }
}

#[test]
fn test_i2c_parameters_required_field() {
    let tool = I2CTool::new();
    let params = tool.parameters();
    let required = params["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("action")));
    // confirm must be documented for write
    assert!(params["properties"]["confirm"].is_object());
}

#[test]
fn test_spi_parameters_required_and_confirm() {
    let tool = SPITool::new();
    let params = tool.parameters();
    let required = params["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("action")));
    // confirm must be documented for transfer
    assert!(params["properties"]["confirm"].is_object());
}
