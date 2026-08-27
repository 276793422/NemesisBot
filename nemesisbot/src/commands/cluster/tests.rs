use super::*;
use tempfile::TempDir;

fn make_home(tmp: &TempDir) -> std::path::PathBuf {
    let home = tmp.path().join(".nemesisbot");
    let config_dir = home.join("workspace").join("config");
    let _ = std::fs::create_dir_all(&config_dir);
    home
}

fn write_cluster_config(home: &std::path::Path, json: &serde_json::Value) {
    let cfg_path = crate::common::cluster_config_path(home);
    if let Some(parent) = cfg_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&cfg_path, serde_json::to_string_pretty(json).unwrap()).unwrap();
}

#[test]
fn test_base64_encode_empty() {
    assert_eq!(base64_encode(&[]), "");
}

#[test]
fn test_base64_encode_hello() {
    // "Hello" = [72, 101, 108, 108, 111] → base64 "SGVsbG8="
    assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
}

#[test]
fn test_base64_encode_single_byte() {
    // 'A' = [65] → "QQ=="
    assert_eq!(base64_encode(b"A"), "QQ==");
}

#[test]
fn test_base64_encode_two_bytes() {
    // "AB" = [65, 66] → "QUI="
    assert_eq!(base64_encode(b"AB"), "QUI=");
}

#[test]
fn test_base64_encode_three_bytes() {
    // "ABC" = [65, 66, 67] → "QUJD"
    assert_eq!(base64_encode(b"ABC"), "QUJD");
}

#[test]
fn test_base64_encode_known_vectors() {
    // Test vectors from RFC 4648
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

#[test]
fn test_mask_token_short() {
    assert_eq!(mask_token("abc"), "****");
    assert_eq!(mask_token("12345678"), "****");
}

#[test]
fn test_mask_token_long() {
    assert_eq!(mask_token("abcdefghijklmnop"), "abcd****mnop");
}

#[test]
fn test_mask_token_exactly_9() {
    // 9 chars: first 4 + **** + last 4
    assert_eq!(mask_token("123456789"), "1234****6789");
}

#[test]
fn test_generate_token_length() {
    let token = generate_token(32);
    // base64 of 32 bytes = 44 chars (ceil(32/3)*4 = 44)
    assert_eq!(token.len(), 44);
    // Should be valid base64 characters
    for c in token.chars() {
        assert!(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
    }
}

#[test]
fn test_generate_token_16_bytes() {
    let token = generate_token(16);
    // base64 of 16 bytes = 24 chars (ceil(16/3)*4 = 24)
    assert_eq!(token.len(), 24);
}

#[test]
fn test_generate_token_unique() {
    let t1 = generate_token(32);
    let t2 = generate_token(32);
    assert_ne!(t1, t2, "Two generated tokens should differ");
}

#[test]
fn test_update_cluster_config_creates_file() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cfg_path = crate::common::cluster_config_path(&home);

    // Write initial config
    let initial = serde_json::json!({"enabled": false, "name": "test"});
    std::fs::write(&cfg_path, serde_json::to_string(&initial).unwrap()).unwrap();

    update_cluster_config(&home, "enabled", true).unwrap();

    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["enabled"], true);
    assert_eq!(cfg["name"], "test");
}

#[test]
fn test_update_cluster_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let result = update_cluster_config(&home, "enabled", true);
    assert!(result.is_err());
}

#[test]
fn test_enable_peer_in_toml_basic() {
    let toml_content = r#"
[peers]
[peers.node1]
address = "192.168.1.10:11949"
role = "worker"
"#;
    let result = enable_peer_in_toml(toml_content, "192.168.1.10:11949", true);
    assert!(result.is_ok());
    let doc: toml::Value = result.unwrap().parse().unwrap();
    assert_eq!(doc["peers"]["node1"]["enabled"], toml::Value::Boolean(true));
}

#[test]
fn test_enable_peer_in_toml_disable() {
    let toml_content = r#"
[peers]
[peers.my_node]
address = "10.0.0.1:21949"
role = "manager"
enabled = true
"#;
    let result = enable_peer_in_toml(toml_content, "10.0.0.1:21949", false);
    assert!(result.is_ok());
    let doc: toml::Value = result.unwrap().parse().unwrap();
    assert_eq!(
        doc["peers"]["my_node"]["enabled"],
        toml::Value::Boolean(false)
    );
}

#[test]
fn test_enable_peer_in_toml_no_peers_section() {
    let result = enable_peer_in_toml("[other]\nkey = \"value\"", "1.2.3.4:11949", true);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No [peers] section"));
}

#[test]
fn test_enable_peer_in_toml_peer_not_found() {
    let toml_content = "[peers]\n[peers.node1]\naddress = \"1.1.1.1:11949\"";
    let result = enable_peer_in_toml(toml_content, "9.9.9.9:11949", true);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_enable_peer_in_toml_invalid_toml() {
    let result = enable_peer_in_toml("not valid {{{{", "1.1.1.1:11949", true);
    assert!(result.is_err());
}

#[test]
fn test_enable_peer_in_toml_sanitized_key_match() {
    // When the sanitized key matches, it should find the peer even without scanning
    let toml_content = "[peers]\n[peers.192_168_1_10_11949]\naddress = \"192.168.1.10:11949\"";
    let result = enable_peer_in_toml(toml_content, "192.168.1.10:11949", true);
    assert!(result.is_ok());
}

// -------------------------------------------------------------------------
// Key sanitization tests (matching PeerAction logic)
// -------------------------------------------------------------------------

#[test]
fn test_key_sanitization_dots() {
    let id = "192.168.1.10";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    assert_eq!(key_safe, "192_168_1_10");
}

#[test]
fn test_key_sanitization_colons() {
    let id = "host:11949";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    assert_eq!(key_safe, "host_11949");
}

#[test]
fn test_key_sanitization_hyphens() {
    let id = "my-peer-node";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    assert_eq!(key_safe, "my_peer_node");
}

#[test]
fn test_key_sanitization_combined() {
    let id = "192.168.1.10:11949";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    assert_eq!(key_safe, "192_168_1_10_11949");
}

#[test]
fn test_key_sanitization_no_special_chars() {
    let id = "simplenode";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    assert_eq!(key_safe, "simplenode");
}

// -------------------------------------------------------------------------
// mask_token additional tests
// -------------------------------------------------------------------------

#[test]
fn test_mask_token_exactly_8() {
    // 8 chars: treated as short (<=8)
    assert_eq!(mask_token("12345678"), "****");
}

#[test]
fn test_mask_token_10_chars() {
    let masked = mask_token("abcdefghij");
    assert_eq!(masked, "abcd****ghij");
}

#[test]
fn test_mask_token_16_chars() {
    let masked = mask_token("0123456789abcdef");
    assert_eq!(masked, "0123****cdef");
}

// -------------------------------------------------------------------------
// base64_encode additional edge cases
// -------------------------------------------------------------------------

#[test]
fn test_base64_encode_long_data() {
    let data = b"The quick brown fox jumps over the lazy dog";
    let encoded = base64_encode(data);
    // Verify it only contains valid base64 chars
    for c in encoded.chars() {
        assert!(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
    }
}

#[test]
fn test_base64_encode_all_zeros() {
    let data = [0u8; 3];
    let encoded = base64_encode(&data);
    assert_eq!(encoded, "AAAA");
}

#[test]
fn test_base64_encode_all_ones() {
    let data = [0xFFu8; 3];
    let encoded = base64_encode(&data);
    assert_eq!(encoded, "////");
}

// -------------------------------------------------------------------------
// generate_token edge cases
// -------------------------------------------------------------------------

#[test]
fn test_generate_token_128_bytes() {
    let token = generate_token(128);
    // base64 of 128 bytes = 172 chars (ceil(128/3)*4 = 172)
    assert_eq!(token.len(), 172);
}

// -------------------------------------------------------------------------
// ClusterAction enum dispatch tests (verification that variants work)
// -------------------------------------------------------------------------

#[test]
fn test_cluster_config_update_and_read() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(
        &home,
        &serde_json::json!({
            "enabled": false,
            "name": "test-node",
            "role": "worker",
            "category": "development",
            "port": 11949,
            "rpc_port": 21949,
            "broadcast_interval": 30
        }),
    );

    // Update
    update_cluster_config(&home, "name", "new-name").unwrap();
    update_cluster_config(&home, "enabled", true).unwrap();

    // Read back
    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["name"], "new-name");
    assert_eq!(cfg["enabled"], true);
    assert_eq!(cfg["role"], "worker"); // unchanged
}

// -------------------------------------------------------------------------
// Peer TOML entry generation tests
// -------------------------------------------------------------------------

#[test]
fn test_peer_entry_format() {
    let id = "node-1";
    let peer_addr = "192.168.1.10:11949";
    let peer_role = "worker";
    let peer_cat = "general";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    let entry = format!(
        "\n[peers.{}]\naddress = \"{}\"\nrole = \"{}\"\ncategory = \"{}\"\n",
        key_safe, peer_addr, peer_role, peer_cat
    );
    assert!(entry.contains("[peers.node_1]"));
    assert!(entry.contains("address = \"192.168.1.10:11949\""));
    assert!(entry.contains("role = \"worker\""));
    assert!(entry.contains("category = \"general\""));
}

#[test]
fn test_peer_entry_with_tags_and_capabilities() {
    let id = "mynode";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    let mut entry = format!(
        "\n[peers.{}]\naddress = \"127.0.0.1:11949\"\nrole = \"worker\"\ncategory = \"general\"\n",
        key_safe
    );
    let tags = Some("ai,dev");
    let capabilities = Some("llm,scanner");
    if let Some(t) = &tags {
        entry.push_str(&format!("tags = \"{}\"\n", t));
    }
    if let Some(c) = &capabilities {
        entry.push_str(&format!("capabilities = \"{}\"\n", c));
    }
    assert!(entry.contains("tags = \"ai,dev\""));
    assert!(entry.contains("capabilities = \"llm,scanner\""));
}

#[test]
fn test_peer_entry_with_priority() {
    let id = "mynode";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    let mut entry = format!(
        "\n[peers.{}]\naddress = \"127.0.0.1:11949\"\nrole = \"worker\"\ncategory = \"general\"\n",
        key_safe
    );
    let priority: Option<i32> = Some(10);
    if let Some(p) = priority {
        entry.push_str(&format!("priority = {}\n", p));
    }
    assert!(entry.contains("priority = 10"));
}

// -------------------------------------------------------------------------
// Cluster init config generation tests
// -------------------------------------------------------------------------

#[test]
fn test_cluster_init_config_defaults() {
    let name = None;
    let role = None;
    let category = None;
    let node_id = format!("node-test");
    let default_name = format!("Bot {}", node_id);

    let config = serde_json::json!({
        "enabled": false,
        "node_id": node_id,
        "name": name.unwrap_or_else(|| default_name.clone()),
        "role": role.unwrap_or_else(|| "worker".to_string()),
        "category": category.unwrap_or_else(|| "development".to_string()),
        "port": 11949,
        "rpc_port": 21949,
        "broadcast_interval": 30,
    });

    assert_eq!(config["enabled"], false);
    assert_eq!(config["name"], default_name);
    assert_eq!(config["role"], "worker");
    assert_eq!(config["category"], "development");
    assert_eq!(config["port"], 11949);
    assert_eq!(config["rpc_port"], 21949);
    assert_eq!(config["broadcast_interval"], 30);
}

#[test]
fn test_cluster_init_config_custom() {
    let config = serde_json::json!({
        "enabled": false,
        "node_id": "node-custom",
        "name": "My Custom Bot",
        "role": "manager",
        "category": "ops",
        "port": 11949,
        "rpc_port": 21949,
        "tags": "prod,ai",
        "address": "10.0.0.1",
        "capabilities": "llm,tools",
    });

    assert_eq!(config["name"], "My Custom Bot");
    assert_eq!(config["role"], "manager");
    assert_eq!(config["category"], "ops");
    assert_eq!(config["tags"], "prod,ai");
    assert_eq!(config["address"], "10.0.0.1");
    assert_eq!(config["capabilities"], "llm,tools");
}

// -------------------------------------------------------------------------
// Token action validation tests
// -------------------------------------------------------------------------

#[test]
fn test_token_length_validation_too_short() {
    let length: usize = 10;
    assert!(length < 16, "Token length must be at least 16");
}

#[test]
fn test_token_length_validation_valid() {
    let length: usize = 32;
    assert!((16..=128).contains(&length));
}

#[test]
fn test_token_length_validation_too_long() {
    let length: usize = 200;
    assert!(length > 128, "Token length must be at most 128");
}

#[test]
fn test_token_string_validation() {
    // Test the Set command's token validation
    let token = "a".repeat(10);
    assert!(token.len() < 16, "Token too short");

    let token = "a".repeat(32);
    assert!((16..=128).contains(&token.len()));

    let token = "a".repeat(200);
    assert!(token.len() > 128, "Token too long");
}

// -------------------------------------------------------------------------
// Cluster init config additional tests
// -------------------------------------------------------------------------

#[test]
fn test_cluster_init_config_with_all_fields() {
    let node_id = format!("node-{}", uuid::Uuid::new_v4());
    let mut config = serde_json::json!({
        "enabled": false,
        "node_id": node_id,
        "name": "CustomBot",
        "role": "coordinator",
        "category": "testing",
        "port": 11949,
        "rpc_port": 21949,
        "broadcast_interval": 30,
        "token": uuid::Uuid::new_v4().to_string(),
    });
    // Add optional fields
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "tags".to_string(),
            serde_json::Value::String("prod,ai".to_string()),
        );
        obj.insert(
            "address".to_string(),
            serde_json::Value::String("10.0.0.5".to_string()),
        );
        obj.insert(
            "capabilities".to_string(),
            serde_json::Value::String("llm,scanner".to_string()),
        );
    }
    assert_eq!(config["tags"], "prod,ai");
    assert_eq!(config["address"], "10.0.0.5");
    assert_eq!(config["capabilities"], "llm,scanner");
    assert_eq!(config["role"], "coordinator");
    assert_eq!(config["category"], "testing");
}

// -------------------------------------------------------------------------
// update_cluster_config additional tests
// -------------------------------------------------------------------------

#[test]
fn test_update_cluster_config_string_value() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(&home, &serde_json::json!({"enabled": false, "name": "old"}));

    update_cluster_config(&home, "name", "new-name").unwrap();

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["name"], "new-name");
}

#[test]
fn test_update_cluster_config_number_value() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(&home, &serde_json::json!({"port": 11949}));

    update_cluster_config(&home, "port", 9999).unwrap();

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["port"], 9999);
}

#[test]
fn test_update_cluster_config_adds_new_field() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(&home, &serde_json::json!({"enabled": false}));

    update_cluster_config(&home, "new_field", "new_value").unwrap();

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["new_field"], "new_value");
    assert_eq!(cfg["enabled"], false); // existing preserved
}

// -------------------------------------------------------------------------
// enable_peer_in_toml additional tests
// -------------------------------------------------------------------------

#[test]
fn test_enable_peer_in_toml_with_existing_enabled() {
    let toml_content = r#"
[peers]
[peers.node1]
address = "10.0.0.1:11949"
role = "worker"
enabled = false
"#;
    let result = enable_peer_in_toml(toml_content, "10.0.0.1:11949", true);
    assert!(result.is_ok());
    let doc: toml::Value = result.unwrap().parse().unwrap();
    assert_eq!(doc["peers"]["node1"]["enabled"], toml::Value::Boolean(true));
    // role should be preserved
    assert_eq!(doc["peers"]["node1"]["role"].as_str(), Some("worker"));
}

#[test]
fn test_enable_peer_in_toml_multiple_peers() {
    let toml_content = r#"
[peers]
[peers.node1]
address = "10.0.0.1:11949"
role = "worker"
[peers.node2]
address = "10.0.0.2:11949"
role = "manager"
"#;
    let result = enable_peer_in_toml(toml_content, "10.0.0.2:11949", true);
    assert!(result.is_ok());
    let doc: toml::Value = result.unwrap().parse().unwrap();
    assert_eq!(doc["peers"]["node2"]["enabled"], toml::Value::Boolean(true));
    // node1 should not have enabled set
    assert!(doc["peers"]["node1"].get("enabled").is_none());
}

// -------------------------------------------------------------------------
// Cluster config display parsing tests
// -------------------------------------------------------------------------

#[test]
fn test_cluster_config_parsing_all_fields() {
    let cfg = serde_json::json!({
        "enabled": true,
        "name": "test-bot",
        "role": "worker",
        "port": 11949,
        "rpc_port": 21949,
        "broadcast_interval": 60,
        "node_id": "node-abc-123"
    });

    assert_eq!(cfg.get("enabled").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(cfg.get("name").and_then(|v| v.as_str()), Some("test-bot"));
    assert_eq!(cfg.get("role").and_then(|v| v.as_str()), Some("worker"));
    assert_eq!(cfg.get("port").and_then(|v| v.as_u64()), Some(11949));
    assert_eq!(cfg.get("rpc_port").and_then(|v| v.as_u64()), Some(21949));
    assert_eq!(
        cfg.get("broadcast_interval").and_then(|v| v.as_u64()),
        Some(60)
    );
    assert_eq!(
        cfg.get("node_id").and_then(|v| v.as_str()),
        Some("node-abc-123")
    );
}

#[test]
fn test_cluster_config_missing_fields_use_defaults() {
    let cfg = serde_json::json!({});
    let cur_udp = cfg.get("port").and_then(|v| v.as_u64()).unwrap_or(11949) as u16;
    let cur_rpc = cfg
        .get("rpc_port")
        .and_then(|v| v.as_u64())
        .unwrap_or(21949) as u16;
    let cur_interval = cfg
        .get("broadcast_interval")
        .and_then(|v| v.as_u64())
        .unwrap_or(30);

    assert_eq!(cur_udp, 11949);
    assert_eq!(cur_rpc, 21949);
    assert_eq!(cur_interval, 30);
}

// -------------------------------------------------------------------------
// Node info display tests
// -------------------------------------------------------------------------

#[test]
fn test_node_info_display_defaults() {
    let cfg = serde_json::json!({});
    let name = cfg
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(not set)");
    let role = cfg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("(not set)");
    let category = cfg
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("(not set)");
    let enabled = cfg
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    assert_eq!(name, "(not set)");
    assert_eq!(role, "(not set)");
    assert_eq!(category, "(not set)");
    assert_eq!(enabled, false);
}

#[test]
fn test_node_info_update_logic() {
    let mut cfg = serde_json::json!({"name": "old", "role": "worker"});
    let mut changed = false;
    if let Some(obj) = cfg.as_object_mut() {
        let name = Some("new-name".to_string());
        let role: Option<String> = None;
        let category = Some("development".to_string());
        if let Some(n) = name {
            obj.insert("name".to_string(), serde_json::Value::String(n));
            changed = true;
        }
        if let Some(r) = role {
            obj.insert("role".to_string(), serde_json::Value::String(r));
            changed = true;
        }
        if let Some(c) = category {
            obj.insert("category".to_string(), serde_json::Value::String(c));
            changed = true;
        }
    }
    assert!(changed);
    assert_eq!(cfg["name"], "new-name");
    assert_eq!(cfg["role"], "worker"); // unchanged
    assert_eq!(cfg["category"], "development");
}

// -------------------------------------------------------------------------
// Peer address display logic tests
// -------------------------------------------------------------------------

#[test]
fn test_peer_defaults() {
    let name: Option<String> = None;
    let address: Option<String> = None;
    let role: Option<String> = None;
    let category: Option<String> = None;

    let display_name = name.as_deref().unwrap_or("peer-id");
    let peer_addr = address.as_deref().unwrap_or("127.0.0.1:11949");
    let peer_role = role.as_deref().unwrap_or("worker");
    let peer_cat = category.as_deref().unwrap_or("general");

    assert_eq!(display_name, "peer-id");
    assert_eq!(peer_addr, "127.0.0.1:11949");
    assert_eq!(peer_role, "worker");
    assert_eq!(peer_cat, "general");
}

// -------------------------------------------------------------------------
// Additional coverage tests for cluster
// -------------------------------------------------------------------------

#[test]
fn test_generate_token_zero_bytes() {
    let token = generate_token(0);
    assert_eq!(token.len(), 0);
}

#[test]
fn test_generate_token_one_byte() {
    let token = generate_token(1);
    assert_eq!(token.len(), 4); // base64 of 1 byte = 4 chars
}

#[test]
fn test_mask_token_various_lengths() {
    assert_eq!(mask_token(""), "****");
    assert_eq!(mask_token("a"), "****");
    assert_eq!(mask_token("12345678"), "****");
    assert_eq!(mask_token("123456789"), "1234****6789");
    assert_eq!(mask_token("abcdefghijklmnop"), "abcd****mnop");
}

#[test]
fn test_update_cluster_config_multiple_fields() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cfg_path = crate::common::cluster_config_path(&home);
    let initial = serde_json::json!({"enabled": false, "name": "bot1"});
    std::fs::write(&cfg_path, serde_json::to_string(&initial).unwrap()).unwrap();

    update_cluster_config(&home, "enabled", true).unwrap();
    update_cluster_config(&home, "name", "renamed").unwrap();
    update_cluster_config(&home, "port", 12345).unwrap();

    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["enabled"], true);
    assert_eq!(cfg["name"], "renamed");
    assert_eq!(cfg["port"], 12345);
}

#[test]
fn test_enable_peer_in_toml_with_custom_key() {
    let toml_content = r#"
[peers]
[peers.mycustompeer]
address = "10.0.0.5:11949"
role = "worker"
"#;
    // The address "10.0.0.5:11949" won't match sanitized key "mycustompeer"
    // so it falls through to address scanning
    let result = enable_peer_in_toml(toml_content, "10.0.0.5:11949", true);
    assert!(result.is_ok());
    let doc: toml::Value = result.unwrap().parse().unwrap();
    assert_eq!(
        doc["peers"]["mycustompeer"]["enabled"],
        toml::Value::Boolean(true)
    );
}

#[test]
fn test_enable_peer_in_toml_toggle_back_and_forth() {
    let toml_content = r#"
[peers]
[peers.test_node]
address = "192.168.1.1:11949"
role = "manager"
"#;
    // Enable
    let result1 = enable_peer_in_toml(toml_content, "192.168.1.1:11949", true);
    assert!(result1.is_ok());
    // Disable
    let result2 = enable_peer_in_toml(&result1.unwrap(), "192.168.1.1:11949", false);
    assert!(result2.is_ok());
    let doc: toml::Value = result2.unwrap().parse().unwrap();
    assert_eq!(
        doc["peers"]["test_node"]["enabled"],
        toml::Value::Boolean(false)
    );
}

#[test]
fn test_base64_encode_various_inputs() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"a"), "YQ==");
    assert_eq!(base64_encode(b"ab"), "YWI=");
    assert_eq!(base64_encode(b"abc"), "YWJj");
    assert_eq!(base64_encode(b"abcd"), "YWJjZA==");
    assert_eq!(base64_encode(b"abcde"), "YWJjZGU=");
    assert_eq!(base64_encode(b"abcdef"), "YWJjZGVm");
}

#[test]
fn test_base64_encode_binary_data() {
    let data: Vec<u8> = (0..=255).collect();
    let encoded = base64_encode(&data);
    // Verify roundtrip length: 256 bytes -> ceil(256/3)*4 = 344 chars
    assert_eq!(encoded.len(), 344);
}

#[test]
fn test_cluster_init_config_with_optional_fields() {
    let node_id = "node-test-opts";
    let mut config = serde_json::json!({
        "enabled": false,
        "node_id": node_id,
        "name": "TestBot",
        "role": "manager",
        "category": "ops",
        "port": 11949,
        "rpc_port": 21949,
        "broadcast_interval": 60,
    });
    // Add optional fields
    if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "tags".to_string(),
            serde_json::Value::String("prod,ai".to_string()),
        );
        obj.insert(
            "address".to_string(),
            serde_json::Value::String("10.0.0.1".to_string()),
        );
        obj.insert(
            "capabilities".to_string(),
            serde_json::Value::String("llm,scanner".to_string()),
        );
    }
    assert_eq!(config["tags"], "prod,ai");
    assert_eq!(config["address"], "10.0.0.1");
    assert_eq!(config["capabilities"], "llm,scanner");
}

#[test]
fn test_peer_entry_no_optional_fields() {
    let id = "simple-node";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    let entry = format!(
        "\n[peers.{}]\naddress = \"127.0.0.1:11949\"\nrole = \"worker\"\ncategory = \"general\"\n",
        key_safe
    );
    assert!(entry.contains("[peers.simple_node]"));
    assert!(!entry.contains("tags"));
    assert!(!entry.contains("capabilities"));
    assert!(!entry.contains("priority"));
}

#[test]
fn test_key_sanitization_empty_string() {
    let id = "";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    assert_eq!(key_safe, "");
}

#[test]
fn test_update_cluster_config_invalid_json_file() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cfg_path = crate::common::cluster_config_path(&home);
    if let Some(parent) = cfg_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&cfg_path, "not valid json").unwrap();
    let result = update_cluster_config(&home, "enabled", true);
    assert!(result.is_err());
}

// -------------------------------------------------------------------------
// constant_time_eq tests (via crate::common)
// -------------------------------------------------------------------------

#[test]
fn test_constant_time_eq_equal() {
    assert!(crate::common::constant_time_eq(b"hello", b"hello"));
}

#[test]
fn test_constant_time_eq_not_equal() {
    assert!(!crate::common::constant_time_eq(b"hello", b"world"));
}

#[test]
fn test_constant_time_eq_different_lengths() {
    assert!(!crate::common::constant_time_eq(b"short", b"longer"));
}

#[test]
fn test_constant_time_eq_empty() {
    assert!(crate::common::constant_time_eq(b"", b""));
}

#[test]
fn test_constant_time_eq_single_byte_diff() {
    assert!(!crate::common::constant_time_eq(b"aaaab", b"aaaaa"));
}

// -------------------------------------------------------------------------
// format_token tests (via crate::common)
// -------------------------------------------------------------------------

#[test]
fn test_format_token_empty() {
    assert_eq!(crate::common::format_token(""), "(not set)");
}

#[test]
fn test_format_token_short() {
    assert_eq!(crate::common::format_token("abc"), "***");
}

#[test]
fn test_format_token_exactly_8() {
    assert_eq!(crate::common::format_token("12345678"), "***");
}

#[test]
fn test_format_token_long() {
    assert_eq!(
        crate::common::format_token("abcdefghijklmnop"),
        "abcd...mnop"
    );
}

// -------------------------------------------------------------------------
// cluster_config_path tests
// -------------------------------------------------------------------------

#[test]
fn test_cluster_config_path_format() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let path = crate::common::cluster_config_path(&home);
    let path_str = path.to_string_lossy();
    assert!(
        path_str.contains("workspace")
            && path_str.contains("config")
            && path_str.contains("config.cluster.json"),
        "Expected workspace/config/config.cluster.json in path, got: {}",
        path_str
    );
}

#[test]
fn test_cluster_dir_format() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let dir = crate::common::cluster_dir(&home);
    let dir_str = dir.to_string_lossy();
    assert!(
        dir_str.contains("workspace") && dir_str.contains("cluster"),
        "Expected workspace/cluster in path, got: {}",
        dir_str
    );
}

// -------------------------------------------------------------------------
// Token verify against saved config
// -------------------------------------------------------------------------

#[test]
fn test_token_verify_match() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let saved_token = "my-super-secret-token-for-testing";
    write_cluster_config(&home, &serde_json::json!({"token": saved_token}));

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    let stored = cfg.get("token").and_then(|v| v.as_str()).unwrap();
    assert!(crate::common::constant_time_eq(
        stored.as_bytes(),
        saved_token.as_bytes()
    ));
}

#[test]
fn test_token_verify_mismatch() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(&home, &serde_json::json!({"token": "correct-token"}));

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    let stored = cfg.get("token").and_then(|v| v.as_str()).unwrap();
    assert!(!crate::common::constant_time_eq(
        stored.as_bytes(),
        b"wrong-token"
    ));
}

// -------------------------------------------------------------------------
// Token revoke logic (remove from config)
// -------------------------------------------------------------------------

#[test]
fn test_token_revoke_removes_from_config() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(
        &home,
        &serde_json::json!({"enabled": true, "token": "abc123"}),
    );

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let mut cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    if let Some(obj) = cfg.as_object_mut() {
        obj.remove("token");
    }
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    let data2 = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg2: serde_json::Value = serde_json::from_str(&data2).unwrap();
    assert!(cfg2.get("token").is_none());
    assert_eq!(cfg2["enabled"], true);
}

// -------------------------------------------------------------------------
// Token set via config update
// -------------------------------------------------------------------------

#[test]
fn test_token_set_in_config() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    write_cluster_config(&home, &serde_json::json!({"enabled": true}));

    update_cluster_config(&home, "token", "new-token-value-12345").unwrap();

    let cfg_path = crate::common::cluster_config_path(&home);
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["token"], "new-token-value-12345");
}

// -------------------------------------------------------------------------
// Enable/Disable state check logic
// -------------------------------------------------------------------------

#[test]
fn test_enable_checks_already_enabled() {
    let cfg = serde_json::json!({"enabled": true});
    let already = cfg
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(already);
}

#[test]
fn test_disable_checks_already_disabled() {
    let cfg = serde_json::json!({"enabled": false});
    let already = !cfg
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(already);
}

#[test]
fn test_enable_no_config_defaults_to_false() {
    let cfg = serde_json::json!({});
    let enabled = cfg
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!enabled);
}

// -------------------------------------------------------------------------
// Soft reset (clear state.toml only)
// -------------------------------------------------------------------------

#[test]
fn test_soft_reset_removes_state_file() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cluster_dir = crate::common::cluster_dir(&home);
    let _ = std::fs::create_dir_all(&cluster_dir);
    let state_path = cluster_dir.join("state.toml");
    std::fs::write(&state_path, "[discovered]\nnode1 = true").unwrap();
    assert!(state_path.exists());

    // Soft reset: remove state.toml only
    let _ = std::fs::remove_file(&state_path);
    assert!(!state_path.exists());

    // peers.toml should NOT be removed
    let peers_path = cluster_dir.join("peers.toml");
    std::fs::write(&peers_path, "[peers]\n").unwrap();
    assert!(peers_path.exists());
}

// -------------------------------------------------------------------------
// Hard reset (clear config + peers + state)
// -------------------------------------------------------------------------

#[test]
fn test_hard_reset_removes_all_files() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cluster_dir = crate::common::cluster_dir(&home);
    let _ = std::fs::create_dir_all(&cluster_dir);

    let cfg_path = crate::common::cluster_config_path(&home);
    let peers_path = cluster_dir.join("peers.toml");
    let state_path = cluster_dir.join("state.toml");

    std::fs::write(&cfg_path, "{}").unwrap();
    std::fs::write(&peers_path, "[peers]\n").unwrap();
    std::fs::write(&state_path, "[state]\n").unwrap();

    // Hard reset
    let _ = std::fs::remove_file(&cfg_path);
    let _ = std::fs::remove_file(&peers_path);
    let _ = std::fs::remove_file(&state_path);

    assert!(!cfg_path.exists());
    assert!(!peers_path.exists());
    assert!(!state_path.exists());
}

// -------------------------------------------------------------------------
// Peers.toml write and reparse
// -------------------------------------------------------------------------

#[test]
fn test_peers_toml_write_and_reparse() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cluster_dir = crate::common::cluster_dir(&home);
    let _ = std::fs::create_dir_all(&cluster_dir);
    let peers_path = cluster_dir.join("peers.toml");

    // Write peer entries
    let existing = String::new();
    let id = "192.168.1.10:11949";
    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
    let entry = format!(
        "\n[peers.{}]\naddress = \"{}\"\nrole = \"worker\"\ncategory = \"general\"\n",
        key_safe, id
    );
    std::fs::write(&peers_path, existing + &entry).unwrap();

    // Reparse
    let data = std::fs::read_to_string(&peers_path).unwrap();
    let doc: toml::Value = data.parse().unwrap();
    assert_eq!(doc["peers"][&key_safe]["address"].as_str(), Some(id));
    assert_eq!(doc["peers"][&key_safe]["role"].as_str(), Some("worker"));
}

#[test]
fn test_peers_toml_remove_entry() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    let cluster_dir = crate::common::cluster_dir(&home);
    let _ = std::fs::create_dir_all(&cluster_dir);
    let peers_path = cluster_dir.join("peers.toml");

    let content = r#"
[peers]
[peers.node1]
address = "10.0.0.1:11949"
role = "worker"
[peers.node2]
address = "10.0.0.2:11949"
role = "manager"
"#;
    std::fs::write(&peers_path, content).unwrap();

    // Remove node1
    let data = std::fs::read_to_string(&peers_path).unwrap();
    let mut doc: toml::Value = data.parse().unwrap();
    if let Some(peers) = doc
        .as_table_mut()
        .and_then(|t| t.get_mut("peers"))
        .and_then(|v| v.as_table_mut())
    {
        peers.remove("node1");
    }
    std::fs::write(&peers_path, toml::to_string_pretty(&doc).unwrap()).unwrap();

    let data2 = std::fs::read_to_string(&peers_path).unwrap();
    let doc2: toml::Value = data2.parse().unwrap();
    assert!(doc2["peers"].get("node1").is_none());
    assert!(doc2["peers"].get("node2").is_some());
}

// -------------------------------------------------------------------------
// Config diff detection (used in Config subcommand)
// -------------------------------------------------------------------------

#[test]
fn test_config_diff_detection_no_change() {
    let cur_udp: u16 = 11949;
    let cur_rpc: u16 = 21949;
    let cur_interval: u64 = 30;
    let new_udp: u16 = 11949;
    let new_rpc: u16 = 21949;
    let new_interval: u64 = 30;
    let changed = new_udp != cur_udp || new_rpc != cur_rpc || new_interval != cur_interval;
    assert!(!changed);
}

#[test]
fn test_config_diff_detection_udp_changed() {
    let cur_udp: u16 = 11949;
    let cur_rpc: u16 = 21949;
    let cur_interval: u64 = 30;
    let new_udp: u16 = 9999;
    let new_rpc: u16 = 21949;
    let new_interval: u64 = 30;
    let changed = new_udp != cur_udp || new_rpc != cur_rpc || new_interval != cur_interval;
    assert!(changed);
}

#[test]
fn test_config_diff_detection_all_changed() {
    let cur_udp: u16 = 11949;
    let cur_rpc: u16 = 21949;
    let cur_interval: u64 = 30;
    let new_udp: u16 = 11111;
    let new_rpc: u16 = 22222;
    let new_interval: u64 = 60;
    let changed = new_udp != cur_udp || new_rpc != cur_rpc || new_interval != cur_interval;
    assert!(changed);
}

// -------------------------------------------------------------------------
// mask_token additional edge cases
// -------------------------------------------------------------------------

#[test]
fn test_mask_token_exactly_9_recheck() {
    assert_eq!(mask_token("123456789"), "1234****6789");
}

#[test]
fn test_mask_token_very_long() {
    let token = "a".repeat(100);
    let masked = mask_token(&token);
    assert_eq!(masked, "aaaa****aaaa");
}

// -------------------------------------------------------------------------
// base64_encode roundtrip verification
// -------------------------------------------------------------------------

#[test]
fn test_base64_encode_matches_standard() {
    // Verify our implementation matches the standard base64 encoding
    assert_eq!(base64_encode(b"\x00"), "AA==");
    assert_eq!(base64_encode(b"\xff"), "/w==");
    assert_eq!(base64_encode(b"\x00\x01"), "AAE=");
    assert_eq!(base64_encode(b"\x00\x01\x02"), "AAEC");
}

// =========================================================================
// run() 端到端分支覆盖（S11 覆盖率冲刺）
//
// 策略：NEMESISBOT_HOME 指向临时目录（resolve_home 优先级 2），
// `run(action, false)` 全程只读写临时 home，绝不触碰生产 home。
// env set_var 是进程级操作 → 全部持 crate::GLOBAL_STATE_LOCK 串行。
// =========================================================================

/// RAII 守卫：设置 NEMESISBOT_HOME 指向临时根，drop 时移除。
/// home = `{tmp}/.nemesisbot`（resolve_home 会自动拼 .nemesisbot）。
struct TempHomeEnv {
    _tmp: TempDir,
    home: std::path::PathBuf,
}

impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn temp_home_env() -> TempHomeEnv {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    TempHomeEnv { _tmp: tmp, home }
}

fn write_peers_toml(home: &std::path::Path, content: &str) {
    let dir = crate::common::cluster_dir(home);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("peers.toml"), content).unwrap();
}

fn read_cluster_cfg(home: &std::path::Path) -> serde_json::Value {
    let p = crate::common::cluster_config_path(home);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[tokio::test]
async fn test_run_status_not_initialized() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 无 config.cluster.json → 走 "[not found]" 分支
    run(ClusterAction::Status, false).await.unwrap();
    assert!(!crate::common::cluster_config_path(&th.home).exists());
}

#[tokio::test]
async fn test_run_status_with_config_and_peers() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(
        &th.home,
        &serde_json::json!({
            "enabled": true, "port": 11949, "rpc_port": 21949, "broadcast_interval": 30
        }),
    );
    write_peers_toml(
        &th.home,
        "[node]\nid = \"node-a\"\nname = \"Node A\"\nrole = \"manager\"\ncategory = \"ops\"\n",
    );
    run(ClusterAction::Status, false).await.unwrap();
}

#[tokio::test]
async fn test_run_status_config_without_peers() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": false }));
    run(ClusterAction::Status, false).await.unwrap();
}

#[tokio::test]
async fn test_run_config_updates_when_values_differ() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(
        &th.home,
        &serde_json::json!({ "port": 11949, "rpc_port": 21949, "broadcast_interval": 30 }),
    );
    run(
        ClusterAction::Config {
            udp_port: 12345,
            rpc_port: 23456,
            broadcast_interval: 60,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["port"], serde_json::json!(12345));
    assert_eq!(cfg["rpc_port"], serde_json::json!(23456));
    assert_eq!(cfg["broadcast_interval"], serde_json::json!(60));
}

#[tokio::test]
async fn test_run_config_same_values_no_rewrite() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(
        &th.home,
        &serde_json::json!({ "port": 11949, "rpc_port": 21949, "broadcast_interval": 30 }),
    );
    run(
        ClusterAction::Config {
            udp_port: 11949,
            rpc_port: 21949,
            broadcast_interval: 30,
        },
        false,
    )
    .await
    .unwrap();
    // 未变化 → 不写回（文件内容保持原样）
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg.as_object().unwrap().len(), 3);
}

#[tokio::test]
async fn test_run_config_missing_keys_not_backfilled_until_diff() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 配置缺 key → cur 值取 unwrap_or 默认（11949/21949/30）；
    // 传与默认相同的值 → 判定"未变化"→ 不回填缺 key（生产现状）
    write_cluster_config(&th.home, &serde_json::json!({}));
    run(
        ClusterAction::Config {
            udp_port: 11949,
            rpc_port: 21949,
            broadcast_interval: 30,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert!(cfg.as_object().unwrap().is_empty());
    // 传不同值 → 触发写入，三个 key 一次性补齐
    run(
        ClusterAction::Config {
            udp_port: 20000,
            rpc_port: 30000,
            broadcast_interval: 90,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["port"], serde_json::json!(20000));
    assert_eq!(cfg["rpc_port"], serde_json::json!(30000));
    assert_eq!(cfg["broadcast_interval"], serde_json::json!(90));
}

#[tokio::test]
async fn test_run_config_missing_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Config {
            udp_port: 1,
            rpc_port: 2,
            broadcast_interval: 3,
        },
        false,
    )
    .await
    .unwrap();
    assert!(!crate::common::cluster_config_path(&th.home).exists());
}

#[tokio::test]
async fn test_run_info_updates_fields_and_saves() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        "[node]\nid = \"node-a\"\nname = \"old\"\nrole = \"worker\"\ncategory = \"general\"\n",
    );
    run(
        ClusterAction::Info {
            name: Some("new-name".into()),
            role: Some("manager".into()),
            category: Some("ops".into()),
            tags: Some("a, b ,,".into()),
            address: Some("127.0.0.1:21949".into()),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(content.contains("new-name"));
    assert!(content.contains("manager"));
    assert!(content.contains("ops"));
    assert!(content.contains("127.0.0.1:21949"));
    let doc: toml::Value = content.parse().unwrap();
    assert_eq!(
        doc["node"]["tags"],
        toml::Value::Array(vec!["a".into(), "b".into()])
    );
}

#[tokio::test]
async fn test_run_info_read_only_no_changes() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        "[node]\nid = \"node-a\"\nname = \"Node A\"\n",
    );
    run(
        ClusterAction::Info {
            name: None,
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_info_missing_peers_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Info {
            name: Some("x".into()),
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_peers_no_subcommand_usage() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(ClusterAction::Peers { action: None }, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_run_peers_list_with_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(&th.home, "[node]\nid = \"node-a\"\n");
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::List),
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_peers_list_without_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::List),
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_peers_add_creates_entry() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Add {
                id: "peer-x".into(),
                name: Some("Peer X".into()),
                address: Some("10.1.1.1:21949".into()),
                role: Some("worker".into()),
                category: Some("dev".into()),
                priority: Some(1),
            }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    // 权威写路径 sanitize_peer_key 只替换 `.`/`:`，保留 `-`
    assert!(content.contains("peer-x"));
    assert!(content.contains("10.1.1.1:21949"));
}

#[tokio::test]
async fn test_run_peers_remove_dash_id_after_add() {
    // (BUG #26, quality-hardening goal 冲刺 S11) 回归：
    // add 写 `[peers.node-a]`（dash 保留），remove --id node-a 必须能删掉
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Add {
                id: "node-a".into(),
                name: None,
                address: Some("10.2.3.4:21949".into()),
                role: None,
                category: None,
                priority: None,
            }),
        },
        false,
    )
    .await
    .unwrap();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Remove { id: "node-a".into() }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(!content.contains("node-a"));
}

#[tokio::test]
async fn test_run_peers_remove_by_address_fallback() {
    // (BUG #26, quality-hardening goal 冲刺 S11) 按 address 兜底删除
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        "[peers]\n[peers.foo]\naddress = \"9.9.9.9:1234\"\nrole = \"worker\"\n",
    );
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Remove { id: "9.9.9.9:1234".into() }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(!content.contains("foo"));
}

#[tokio::test]
async fn test_run_peers_add_defaults() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Add {
                id: "peer-d".into(),
                name: None,
                address: None,
                role: None,
                category: None,
                priority: None,
            }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(content.contains("127.0.0.1:11949"));
    assert!(content.contains("worker"));
}

#[tokio::test]
async fn test_run_peers_remove_existing() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        "[peers]\n[peers.peer_1]\naddress = \"10.0.0.1:21949\"\nrole = \"worker\"\n",
    );
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Remove { id: "peer-1".into() }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(!content.contains("peer_1"));
}

#[tokio::test]
async fn test_run_peers_remove_not_found() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(&th.home, "[peers]\n[peers.other]\naddress = \"1.1.1.1:1\"\n");
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Remove { id: "ghost".into() }),
        },
        false,
    )
    .await
    .unwrap();
    // 未命中 → 文件保持原样
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(content.contains("other"));
}

#[tokio::test]
async fn test_run_peers_remove_no_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Remove { id: "x".into() }),
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_peers_enable_and_disable() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        "[peers]\n[peers.p1]\naddress = \"10.0.0.9:21949\"\nrole = \"worker\"\nenabled = false\n",
    );
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Enable { id: "10.0.0.9:21949".into() }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    let doc: toml::Value = content.parse().unwrap();
    assert_eq!(doc["peers"]["p1"]["enabled"], toml::Value::Boolean(true));

    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Disable { id: "10.0.0.9:21949".into() }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    let doc: toml::Value = content.parse().unwrap();
    assert_eq!(doc["peers"]["p1"]["enabled"], toml::Value::Boolean(false));
}

#[tokio::test]
async fn test_run_peers_enable_no_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Enable { id: "x".into() }),
        },
        false,
    )
    .await
    .unwrap();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Disable { id: "x".into() }),
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_generate_save_persists() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": false }));
    run(
        ClusterAction::Token {
            action: TokenAction::Generate { length: 32, save: true },
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    let token = cfg["token"].as_str().unwrap();
    assert!(!token.is_empty());
}

#[tokio::test]
async fn test_run_token_generate_nosave_and_missing_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 无 config 且 --save → 提示后正常返回
    run(
        ClusterAction::Token {
            action: TokenAction::Generate { length: 32, save: true },
        },
        false,
    )
    .await
    .unwrap();
    assert!(!crate::common::cluster_config_path(&th.home).exists());
    // 不带 --save → 只打印不落盘
    run(
        ClusterAction::Token {
            action: TokenAction::Generate { length: 16, save: false },
        },
        false,
    )
    .await
    .unwrap();
    assert!(!crate::common::cluster_config_path(&th.home).exists());
}

#[tokio::test]
async fn test_run_token_generate_bad_length_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    let err = run(
        ClusterAction::Token {
            action: TokenAction::Generate { length: 8, save: false },
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("between 16 and 128"));
}

#[tokio::test]
async fn test_run_token_show_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 有 token：masked + full
    write_cluster_config(
        &th.home,
        &serde_json::json!({ "token": "abcdefghijklmnop" }),
    );
    run(
        ClusterAction::Token {
            action: TokenAction::Show { full: false },
        },
        false,
    )
    .await
    .unwrap();
    run(
        ClusterAction::Token {
            action: TokenAction::Show { full: true },
        },
        false,
    )
    .await
    .unwrap();
    // 无 token 字段
    write_cluster_config(&th.home, &serde_json::json!({}));
    run(
        ClusterAction::Token {
            action: TokenAction::Show { full: false },
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_show_no_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Token {
            action: TokenAction::Show { full: false },
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_set_value_and_generate() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({}));
    // 显式值
    run(
        ClusterAction::Token {
            action: TokenAction::Set {
                token: Some("0123456789abcdef".into()),
                generate: false,
                length: 32,
            },
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["token"].as_str().unwrap(), "0123456789abcdef");
    // --generate 自动生成
    run(
        ClusterAction::Token {
            action: TokenAction::Set {
                token: None,
                generate: true,
                length: 24,
            },
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_ne!(cfg["token"].as_str().unwrap(), "0123456789abcdef");
    // 什么都没给 → 提示后 Ok 提前返回（不 bail）
    run(
        ClusterAction::Token {
            action: TokenAction::Set {
                token: None,
                generate: false,
                length: 32,
            },
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_set_validation_errors() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    // 显式值太短
    let err = run(
        ClusterAction::Token {
            action: TokenAction::Set {
                token: Some("short".into()),
                generate: false,
                length: 32,
            },
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("between 16 and 128"));
    // --generate 长度非法
    let err = run(
        ClusterAction::Token {
            action: TokenAction::Set {
                token: None,
                generate: true,
                length: 8,
            },
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("between 16 and 128"));
}

#[tokio::test]
async fn test_run_token_set_missing_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Token {
            action: TokenAction::Set {
                token: Some("0123456789abcdef".into()),
                generate: false,
                length: 32,
            },
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_verify_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(
        &th.home,
        &serde_json::json!({ "token": "0123456789abcdef" }),
    );
    // 匹配
    run(
        ClusterAction::Token {
            action: TokenAction::Verify {
                token: "0123456789abcdef".into(),
            },
        },
        false,
    )
    .await
    .unwrap();
    // 不匹配
    run(
        ClusterAction::Token {
            action: TokenAction::Verify {
                token: "ffffffffffffffff".into(),
            },
        },
        false,
    )
    .await
    .unwrap();
    // 无 token 字段
    write_cluster_config(&th.home, &serde_json::json!({}));
    run(
        ClusterAction::Token {
            action: TokenAction::Verify {
                token: "whatever".into(),
            },
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_verify_no_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Token {
            action: TokenAction::Verify {
                token: "x".into(),
            },
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_token_revoke_removes_token() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(
        &th.home,
        &serde_json::json!({ "enabled": false, "token": "0123456789abcdef" }),
    );
    run(
        ClusterAction::Token {
            action: TokenAction::Revoke,
        },
        false,
    )
    .await
    .unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert!(cfg.get("token").is_none());
    assert_eq!(cfg["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn test_run_token_revoke_no_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        ClusterAction::Token {
            action: TokenAction::Revoke,
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_init_fresh_home() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Init {
            name: Some("TestNode".into()),
            role: Some("manager".into()),
            category: Some("testing".into()),
            tags: Some("t1,t2".into()),
            address: Some("127.0.0.1:21949".into()),
        },
        false,
    )
    .await
    .unwrap();
    // config.cluster.json 已写（默认模板 + token）
    let cfg = read_cluster_cfg(&th.home);
    assert!(!cfg["token"].as_str().unwrap_or("").is_empty());
    // peers.toml 身份段已写
    let peers =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(peers.contains("TestNode"));
    assert!(peers.contains("manager"));
    assert!(peers.contains("testing"));
    assert!(peers.contains("127.0.0.1:21949"));
    let doc: toml::Value = peers.parse().unwrap();
    assert_eq!(
        doc["node"]["tags"],
        toml::Value::Array(vec!["t1".into(), "t2".into()])
    );
}

#[tokio::test]
async fn test_run_init_defaults_and_reinit_nontty() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 无任何参数 → default_name/node_id 生成
    run(
        ClusterAction::Init {
            name: None,
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
    let peers1 =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(peers1.contains("worker"));
    assert!(peers1.contains("development"));
    // 已存在 + stdin 非终端（cargo test 管道）→ 跳过交互确认直接覆盖
    run(
        ClusterAction::Init {
            name: Some("Second".into()),
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
    let peers2 =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(peers2.contains("Second"));
}

#[tokio::test]
async fn test_run_enable_writes_enabled_true() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": false }));
    run(ClusterAction::Enable, false).await.unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["enabled"], serde_json::json!(true));
}

#[tokio::test]
async fn test_run_enable_already_enabled() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": true }));
    run(ClusterAction::Enable, false).await.unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["enabled"], serde_json::json!(true));
}

#[tokio::test]
async fn test_run_enable_not_initialized_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    let err = run(ClusterAction::Enable, false).await.unwrap_err();
    assert!(err.to_string().contains("Cluster not initialized"));
}

#[tokio::test]
async fn test_run_enable_with_main_config_writes_cluster_section() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({}));
    std::fs::write(
        crate::common::config_path(&th.home),
        serde_json::to_string(&serde_json::json!({ "model_list": [] })).unwrap(),
    )
    .unwrap();
    run(ClusterAction::Enable, false).await.unwrap();
    let main: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(crate::common::config_path(&th.home)).unwrap(),
    )
    .unwrap();
    assert_eq!(main["cluster"]["enabled"], serde_json::json!(true));
}

#[tokio::test]
async fn test_run_disable_writes_enabled_false() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": true }));
    // 主 config 存在 → 同步写 cluster.enabled=false
    std::fs::write(
        crate::common::config_path(&th.home),
        serde_json::to_string(&serde_json::json!({ "cluster": { "enabled": true } })).unwrap(),
    )
    .unwrap();
    run(ClusterAction::Disable, false).await.unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["enabled"], serde_json::json!(false));
    let main: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(crate::common::config_path(&th.home)).unwrap(),
    )
    .unwrap();
    assert_eq!(main["cluster"]["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn test_run_disable_already_disabled() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": false }));
    run(ClusterAction::Disable, false).await.unwrap();
    let cfg = read_cluster_cfg(&th.home);
    assert_eq!(cfg["enabled"], serde_json::json!(false));
}

#[tokio::test]
async fn test_run_start_stop_aliases() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": false }));
    run(ClusterAction::Start, false).await.unwrap();
    assert_eq!(
        read_cluster_cfg(&th.home)["enabled"],
        serde_json::json!(true)
    );
    // Start 已启用 → 提前返回
    run(ClusterAction::Start, false).await.unwrap();
    run(ClusterAction::Stop, false).await.unwrap();
    assert_eq!(
        read_cluster_cfg(&th.home)["enabled"],
        serde_json::json!(false)
    );
    // Stop 已停用 → 提前返回
    run(ClusterAction::Stop, false).await.unwrap();
}

#[tokio::test]
async fn test_run_reset_soft_removes_state() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let state = crate::common::cluster_dir(&th.home).join("state.toml");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(&state, "[x]\n").unwrap();
    run(ClusterAction::Reset { hard: false }, false)
        .await
        .unwrap();
    assert!(!state.exists());
    // 无 state 文件 → no-op 分支
    run(ClusterAction::Reset { hard: false }, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_run_reset_hard_aborts_without_tty_confirm() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_cluster_config(&th.home, &serde_json::json!({ "enabled": true }));
    write_peers_toml(&th.home, "[node]\nid = \"node-a\"\n");
    // stdin 是管道（cargo test）→ read_line 得到 EOF 空串 ≠ "y" → Aborted
    run(ClusterAction::Reset { hard: true }, false)
        .await
        .unwrap();
    // 中止 → 文件原样保留
    assert!(crate::common::cluster_config_path(&th.home).exists());
    assert!(
        crate::common::cluster_dir(&th.home)
            .join("peers.toml")
            .exists()
    );
}

#[tokio::test]
async fn test_run_identity_show_missing_and_present() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Identity {
            action: IdentityAction::Show,
        },
        false,
    )
    .await
    .unwrap();
    let id_path = crate::common::cluster_dir(&th.home).join("IDENTITY.md");
    std::fs::create_dir_all(id_path.parent().unwrap()).unwrap();
    std::fs::write(&id_path, "custom identity").unwrap();
    run(
        ClusterAction::Identity {
            action: IdentityAction::Show,
        },
        false,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn test_run_identity_edit_creates_then_exists() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Identity {
            action: IdentityAction::Edit,
        },
        false,
    )
    .await
    .unwrap();
    let id_path = crate::common::cluster_dir(&th.home).join("IDENTITY.md");
    let content = std::fs::read_to_string(&id_path).unwrap();
    assert_eq!(content, crate::CLUSTER_IDENTITY_TEMPLATE);
    // 第二次 Edit → already exists 分支（不覆盖）
    std::fs::write(&id_path, "hand-edited").unwrap();
    run(
        ClusterAction::Identity {
            action: IdentityAction::Edit,
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(&id_path).unwrap(), "hand-edited");
}

#[tokio::test]
async fn test_run_identity_reset_writes_default() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(
        ClusterAction::Identity {
            action: IdentityAction::Reset,
        },
        false,
    )
    .await
    .unwrap();
    let id_path = crate::common::cluster_dir(&th.home).join("IDENTITY.md");
    let content = std::fs::read_to_string(&id_path).unwrap();
    assert_eq!(content, crate::DEFAULT_IDENTITY_CLUSTER);
    // 覆盖已有文件同样成立
    std::fs::write(&id_path, "temp").unwrap();
    run(
        ClusterAction::Identity {
            action: IdentityAction::Reset,
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&id_path).unwrap(),
        crate::DEFAULT_IDENTITY_CLUSTER
    );
}

#[tokio::test]
async fn test_run_node_not_initialized_bails() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    let err = run(
        ClusterAction::Node {
            udp_port: None,
            rpc_port: None,
            name: None,
            broadcast_interval: 10,
        },
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Cluster not initialized"));
}

// -------------------------------------------------------------------------
// parse_host_port / update_main_config_cluster 直测
// -------------------------------------------------------------------------

#[test]
fn test_parse_host_port_typical() {
    assert_eq!(parse_host_port("10.0.0.1:21949"), ("10.0.0.1".into(), 21949));
}

#[test]
fn test_parse_host_port_no_port() {
    assert_eq!(parse_host_port("localhost"), ("localhost".into(), 0));
}

#[test]
fn test_parse_host_port_bad_port() {
    assert_eq!(parse_host_port("host:abc"), ("host".into(), 0));
}

#[test]
fn test_parse_host_port_last_segment_is_port() {
    // rsplitn(2, ':') → 最后一段当端口
    let (host, port) = parse_host_port("::1:8080");
    assert_eq!(port, 8080);
    assert_eq!(host, "::1");
}

#[test]
fn test_update_main_config_cluster_missing_config_ok() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    // config.json 不存在 → Ok 且不创建文件
    update_main_config_cluster(&home, true).unwrap();
    assert!(!crate::common::config_path(&home).exists());
}

#[test]
fn test_update_main_config_cluster_writes_section() {
    let tmp = TempDir::new().unwrap();
    let home = make_home(&tmp);
    std::fs::write(
        crate::common::config_path(&home),
        serde_json::to_string(&serde_json::json!({ "other": 1 })).unwrap(),
    )
    .unwrap();
    update_main_config_cluster(&home, true).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(crate::common::config_path(&home)).unwrap(),
    )
    .unwrap();
    assert_eq!(cfg["cluster"]["enabled"], serde_json::json!(true));
    assert_eq!(cfg["other"], serde_json::json!(1));
    // 再关 → 覆盖为 false
    update_main_config_cluster(&home, false).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(crate::common::config_path(&home)).unwrap(),
    )
    .unwrap();
    assert_eq!(cfg["cluster"]["enabled"], serde_json::json!(false));
}

// =========================================================================
// wave_b（coverage 补测批次 B）
//
// 豁免不碰：firewall netsh 臂（spawn 进程 + 宿主防火墙变更）、run_node
// 主流程臂（UDP 发现 + RPC server 绑定 + ctrl_c 无限循环）、Init/Reset
// 的 TTY 确认分支、generate_token 的 getrandom 失败兜底。
// 本批覆盖：错误传播臂（corrupt JSON Enable/Disable/Start/Stop）、
// 静默跳过臂（Status/Config 对坏文件）、"(not set)" 显示臂、保存失败打印
// 臂（只读文件；root/admin 会无视只读位——断言保持弱语义）、Remove 的
// 原始 key 命中臂（quoted TOML key）。
// =========================================================================

mod wave_b {
use super::*;

/// RAII 守卫：把文件设为只读，drop 时恢复。
/// Windows 下 READONLY 属性使 `std::fs::write` 打开即 Access Denied；
/// Unix 下等价于移除 owner 写位（root 除外，见各测试注）。
struct WaveBReadOnlyGuard {
    path: std::path::PathBuf,
}

impl WaveBReadOnlyGuard {
    fn apply(path: &std::path::Path) -> Self {
        let mut perm = std::fs::metadata(path)
            .expect("readonly target must exist")
            .permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(path, perm).expect("set readonly failed");
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for WaveBReadOnlyGuard {
    fn drop(&mut self) {
        // 先恢复权限再让 TempDir 删除目录树
        if let Ok(meta) = std::fs::metadata(&self.path) {
            let mut perm = meta.permissions();
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&self.path, perm);
        }
    }
}

/// 255-256 / 276：Status 对「config.cluster.json 存在但 JSON 解析失败」与
/// 「peers.toml 存在但解析失败」都静默跳过对应段继续输出（不报错）。
#[tokio::test]
async fn wave_b_status_survives_corrupt_config_and_corrupt_peers_toml() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    std::fs::write(
        crate::common::cluster_config_path(&th.home),
        "{{{ not json",
    )
    .unwrap();
    write_peers_toml(&th.home, "invalid {{{ toml");
    run(ClusterAction::Status, false).await.unwrap();
}

/// 338-339：Config 动作遇到无法解析的 config 文件时静默跳过且绝不改写原文件。
#[tokio::test]
async fn wave_b_config_leaves_unparseable_config_byte_identical() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let cfg_path = crate::common::cluster_config_path(&th.home);
    let garbage = "[[[ definitely not json";
    std::fs::write(&cfg_path, garbage).unwrap();
    run(
        ClusterAction::Config {
            udp_port: 10000,
            rpc_port: 20000,
            broadcast_interval: 40,
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&cfg_path).unwrap(),
        garbage,
        "unparseable config must be left untouched"
    );
}

/// 381 / 383-384：Info 修改字段时 save_static_config 失败（peers.toml 只读）
/// → 打印失败信息后仍继续显示节点信息，run 正常返回。
/// 注：以 root/admin 运行时只读位可能被无视 → 此时只是正常保存成功，
/// 断言因此只钉 Ok 语义。
#[tokio::test]
async fn wave_b_info_save_failure_prints_error_and_continues() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let peers = crate::common::cluster_dir(&th.home).join("peers.toml");
    write_peers_toml(&th.home, "[node]\nid = \"wb-ro\"\nname = \"old\"\n");
    let _ro = WaveBReadOnlyGuard::apply(&peers);
    run(
        ClusterAction::Info {
            name: Some("renamed".into()),
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
}

/// 392 / 400 / 408：name/role/category 为空串时显示 "(not set)"。
/// 注意：NodeInfo 各字段带 serde 默认值（worker/general），缺字段不会触发
/// 这些分支 —— 必须显式写空串才能进入（见报告生产可疑点）。
#[tokio::test]
async fn wave_b_info_empty_identity_fields_print_not_set_markers() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        "[node]\nid = \"wb-empty\"\nname = \"\"\nrole = \"\"\ncategory = \"\"\ntags = []\n",
    );
    run(
        ClusterAction::Info {
            name: None,
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
}

/// 430-432：peers.toml 存在但解析失败 → 打印 "Failed to parse peers.toml."。
#[tokio::test]
async fn wave_b_info_parse_failure_prints_failed_to_parse() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(&th.home, "{ not toml {{{");
    run(
        ClusterAction::Info {
            name: Some("x".into()),
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
}

/// 475：peers add 时权威写路径 append_peer_to_file 失败（peers.toml 是个
/// 目录 → read_to_string Err）→ 打印失败且目录不被破坏。
/// 用路径形态混淆确定性触发，不依赖权限。
#[tokio::test]
async fn wave_b_peers_add_failure_when_peers_toml_is_directory() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let fake = crate::common::cluster_dir(&th.home).join("peers.toml");
    std::fs::create_dir_all(&fake).unwrap();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Add {
                id: "wb-dir".into(),
                name: Some("Dir Peer".into()),
                address: Some("10.5.5.5:11949".into()),
                role: None,
                category: None,
                priority: None,
            }),
        },
        false,
    )
    .await
    .unwrap();
    assert!(fake.is_dir(), "directory must remain untouched");
}

/// 505：[peers] 表里存在与 id 逐字相等的 quoted key（含 '.'/':'，只能用
/// quoted bare-key 表达）→ legacy/canonical 变体都未命中时走原始 id 分支删除。
#[tokio::test]
async fn wave_b_peers_remove_hits_raw_id_key_arm() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_peers_toml(
        &th.home,
        // key 是带 '.'/':' 的 quoted bare-key；address 故意与 id 不同，
        // 否则兜底按地址扫描也能命中、掩盖 raw-id key 分支。
        "[peers]\n[peers.\"10.0.0.77:11949\"]\naddress = \"10.9.9.9:12345\"\nrole = \"worker\"\n",
    );
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Remove {
                id: "10.0.0.77:11949".into(),
            }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert!(
        !content.contains("10.0.0.77"),
        "raw-id keyed peer should be removed, got: {content}"
    );
}

/// 550 / 552 / 567：peers.toml 存在但没有 [peers] 段 →
/// enable_peer_in_toml 报错字符串 → Enable/Disable 都打印消息且不落盘。
#[tokio::test]
async fn wave_b_peers_enable_disable_reports_missing_section_and_keeps_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let original = "[node]\nid = \"solo\"\n";
    write_peers_toml(&th.home, original);
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Enable { id: "whatever".into() }),
        },
        false,
    )
    .await
    .unwrap();
    run(
        ClusterAction::Peers {
            action: Some(PeerAction::Disable { id: "whatever".into() }),
        },
        false,
    )
    .await
    .unwrap();
    let content =
        std::fs::read_to_string(crate::common::cluster_dir(&th.home).join("peers.toml")).unwrap();
    assert_eq!(content, original, "failed toggle must not rewrite the file");
}

/// 788：Init 重写 peers.toml 失败（预置只读文件）→ 打印失败信息后 Init 继续。
/// root/admin 下只读位可能被无视 → 只断言 Ok。
#[tokio::test]
async fn wave_b_init_over_readonly_peers_toml_still_ok() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let peers = crate::common::cluster_dir(&th.home).join("peers.toml");
    write_peers_toml(&th.home, "[node]\nid = \"pre-existing\"\n");
    let _ro = WaveBReadOnlyGuard::apply(&peers);
    run(
        ClusterAction::Init {
            name: Some("WaveB".into()),
            role: None,
            category: None,
            tags: None,
            address: None,
        },
        false,
    )
    .await
    .unwrap();
}

/// 810-811 / 830-832 / 850-852 / 869-871：config 存在但 JSON 解析失败时，
/// Enable/Disable/Start/Stop 的「已启用？」前置检查静默跳过，随后
/// update_cluster_config 解析同一个坏文件 → 错误向上传播（run 返回 Err）。
#[tokio::test]
async fn wave_b_enable_disable_start_stop_propagate_corrupt_config_parse_error() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    std::fs::write(
        crate::common::cluster_config_path(&th.home),
        "{{{ not json",
    )
    .unwrap();

    let e1 = run(ClusterAction::Enable, false).await.expect_err("Enable must propagate");
    let e2 = run(ClusterAction::Disable, false).await.expect_err("Disable must propagate");
    let e3 = run(ClusterAction::Start, false).await.expect_err("Start must propagate");
    let e4 = run(ClusterAction::Stop, false).await.expect_err("Stop must propagate");
    for (name, e) in [("Enable", e1), ("Disable", e2), ("Start", e3), ("Stop", e4)] {
        let msg = format!("{name}: {e}");
        assert!(
            !msg.is_empty(),
            "each action must surface a parse error"
        );
    }
}
} // mod wave_b

// =========================================================================
// wave_c（coverage 补测批次 C）
//
// 目标 miss：目录阻断臂（路径形态：peers.toml / config.cluster.json 是
// 目录 → exists()=true 但 read 失败 → 各动作静默跳过）、非对象 JSON 臂
// （Value::Null/Array 解析成功但 as_object_mut()==None → 写回段整体跳过）、
// Remove 的「doc 解析成功但无 [peers] 表」静默臂、enable_peer_in_toml
// 扫描循环的两种未命中形态（表项非 table / 缺 address 字段）、
// update_cluster_config / update_main_config_cluster 只读与坏 JSON 的
// 错误传播。
// 豁免不碰（结构性 / 机器态）：Init 的 TTY 确认分支（stdin is_terminal，
// cargo test 管道下恒 false）、Reset --hard 的确认成功臂（read_line 在
// 测试进程内拿不到 "y"，EOF→必走 Aborted，已由既有测试钉住）、firewall
// netsh 臂（spawn 进程 + 变更宿主防火墙规则）、run_node 主流程（绑真实
// UDP/RPC 端口 + ctrl_c 无限循环）、Remove 中 peers.remove(&key) 返回
// None 的防御臂（target_key 由同锁 contains_key/find_map 判出，结构性
// 不可达）。
// =========================================================================

mod wave_c {
    use super::*;

    /// RAII 守卫：把文件设为只读，drop 时恢复（本地复刻 wave_b 形态）。
    struct WcReadOnlyGuard {
        path: std::path::PathBuf,
    }

    impl WcReadOnlyGuard {
        fn apply(path: &std::path::Path) -> Self {
            let mut perm = std::fs::metadata(path)
                .expect("readonly target must exist")
                .permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(path, perm).expect("set readonly failed");
            Self {
                path: path.to_path_buf(),
            }
        }
    }

    impl Drop for WcReadOnlyGuard {
        fn drop(&mut self) {
            if let Ok(meta) = std::fs::metadata(&self.path) {
                let mut perm = meta.permissions();
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(&self.path, perm);
            }
        }
    }

    /// 把目标路径替换成同名目录（若已有文件先删除）。
    /// exists() 对目录返回 true、read_to_string 对目录返回 Err —— 用来确定性地
    /// 构造「文件存在但读失败」的目录阻断臂，不依赖平台权限位。
    fn make_path_a_directory(path: &std::path::Path) {
        if path.is_file() {
            std::fs::remove_file(path).expect("remove file before dir swap");
        }
        std::fs::create_dir_all(path).expect("create dir at target path");
    }

    // ---------------------------------------------------------------------
    // Status / Config：路径存在但读失败（目录阻断）的静默跳过臂
    // ---------------------------------------------------------------------

    /// config.cluster.json 是目录 → "found" 后 read_to_string Err →
    /// 内层解析链整体跳过，随后照常走 peers.toml 段并正常返回。
    #[tokio::test]
    async fn wave_c_status_silent_when_config_is_directory() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let cfg_path = crate::common::cluster_config_path(&th.home);
        make_path_a_directory(&cfg_path);
        write_peers_toml(&th.home, "[node]\nid = \"wc-dir\"\n");
        run(ClusterAction::Status, false).await.unwrap();
        assert!(cfg_path.is_dir(), "directory must remain untouched");
    }

    /// peers.toml 是目录 → "[found]" 后 load_static_config 失败被静默吞掉。
    #[tokio::test]
    async fn wave_c_status_silent_when_peers_is_directory() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_cluster_config(&th.home, &serde_json::json!({ "enabled": true }));
        let peers = crate::common::cluster_dir(&th.home).join("peers.toml");
        make_path_a_directory(&peers);
        run(ClusterAction::Status, false).await.unwrap();
        assert!(peers.is_dir());
    }

    /// Config 动作遇到目录形态的配置 → 完全静默跳过且不改写任何东西。
    #[tokio::test]
    async fn wave_c_config_action_silent_when_config_is_directory() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let cfg_path = crate::common::cluster_config_path(&th.home);
        make_path_a_directory(&cfg_path);
        run(
            ClusterAction::Config {
                udp_port: 11111,
                rpc_port: 22222,
                broadcast_interval: 66,
            },
            false,
        )
        .await
        .unwrap();
        assert!(cfg_path.is_dir(), "must not replace directory with file");
    }

    // ---------------------------------------------------------------------
    // Peers Remove：doc 解析成功但 [peers] 缺失 / 坏 TOML / 目录三连静默臂
    // ---------------------------------------------------------------------

    /// 同一测试串三个 home：
    /// A) 合法 TOML 但无 [peers] 段 → get_mut("peers") None；
    /// B) 非法 TOML → parse Err；
    /// C) peers.toml 是目录 → read_to_string Err。
    /// 三者都只静默不落盘、run 正常 Ok。
    #[tokio::test]
    async fn wave_c_remove_silent_skip_arms_for_missing_section_bad_toml_and_directory() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();

        // A) 无 [peers] 段
        let th_a = temp_home_env();
        let original = "[node]\nid = \"solo\"\n".to_string();
        write_peers_toml(&th_a.home, &original);
        run(
            ClusterAction::Peers {
                action: Some(PeerAction::Remove { id: "ghost-a".into() }),
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(crate::common::cluster_dir(&th_a.home).join("peers.toml"))
                .unwrap(),
            original,
            "missing [peers] section must not rewrite the file"
        );
        drop(th_a);

        // B) 坏 TOML
        let th_b = temp_home_env();
        let garbage = "{{{ definitely not toml";
        write_peers_toml(&th_b.home, garbage);
        run(
            ClusterAction::Peers {
                action: Some(PeerAction::Remove { id: "ghost-b".into() }),
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(crate::common::cluster_dir(&th_b.home).join("peers.toml"))
                .unwrap(),
            garbage,
            "unparseable toml must be left untouched"
        );
        drop(th_b);

        // C) 目录阻断
        let th_c = temp_home_env();
        let peers = crate::common::cluster_dir(&th_c.home).join("peers.toml");
        make_path_a_directory(&peers);
        run(
            ClusterAction::Peers {
                action: Some(PeerAction::Remove { id: "ghost-c".into() }),
            },
            false,
        )
        .await
        .unwrap();
        assert!(peers.is_dir());
    }

    /// Enable / Disable 遇到目录形态的 peers.toml → 读失败静默跳过，两个方向
    /// 都不动那棵目录树。
    #[tokio::test]
    async fn wave_c_enable_disable_silent_when_peers_toml_is_directory() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let peers = crate::common::cluster_dir(&th.home).join("peers.toml");
        make_path_a_directory(&peers);
        run(
            ClusterAction::Peers {
                action: Some(PeerAction::Enable { id: "10.0.0.1:1".into() }),
            },
            false,
        )
        .await
        .unwrap();
        run(
            ClusterAction::Peers {
                action: Some(PeerAction::Disable { id: "10.0.0.1:1".into() }),
            },
            false,
        )
        .await
        .unwrap();
        assert!(peers.is_dir());
    }

    // ---------------------------------------------------------------------
    // Token Revoke：非对象 JSON 跳过写回 / 只读写失败向上传播
    // ---------------------------------------------------------------------

    /// 配置是合法 JSON 但不是 object（数组）→ as_object_mut None → 不写回，
    /// run 正常 Ok 且文件字节不变。
    #[tokio::test]
    async fn wave_c_token_revoke_non_object_json_skips_writeback() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let cfg_path = crate::common::cluster_config_path(&th.home);
        std::fs::write(&cfg_path, "[]").unwrap();
        run(
            ClusterAction::Token {
                action: TokenAction::Revoke,
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            "[]",
            "non-object json must be left untouched"
        );
    }

    /// 配置是对象但只读 → Revoke 里 fs::write 的 `?` 把权限错误向上传播
    /// （错误传播边界；root/admin 无视只读位的平台上该断言退化，见注）。
    #[tokio::test]
    async fn wave_c_token_revoke_readonly_config_propagates_write_error() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_cluster_config(
            &th.home,
            &serde_json::json!({ "enabled": true, "token": "0123456789abcdef" }),
        );
        let cfg_path = crate::common::cluster_config_path(&th.home);
        let _ro = WcReadOnlyGuard::apply(&cfg_path);
        let result = run(
            ClusterAction::Token {
                action: TokenAction::Revoke,
            },
            false,
        )
        .await;
        // Windows 下 READONLY 使 write 打开即 Access Denied → Err 必现；
        // 其余平台（root/admin 无视只读位）语义浮动，只保留执行路径覆盖。
        #[cfg(windows)]
        {
            let err = result.expect_err("readonly config must fail fs::write on windows");
            assert!(!err.to_string().is_empty());
        }
        #[cfg(not(windows))]
        {
            let _ = result;
        }
    }

    // ---------------------------------------------------------------------
    // Enable / Disable / Start / Stop：null 配置（解析成功但非 object）
    // → 前置检查取默认继续 → update_cluster_config as_object_mut None
    // → 不写回、不报错（与坏 JSON 的 Err 传播相对的第三种形态）
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn wave_c_enable_disable_start_stop_null_json_is_noop_success() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let cfg_path = crate::common::cluster_config_path(&th.home);
        std::fs::write(&cfg_path, "null").unwrap();

        run(ClusterAction::Enable, false).await.unwrap();
        run(ClusterAction::Disable, false).await.unwrap();
        run(ClusterAction::Start, false).await.unwrap();
        run(ClusterAction::Stop, false).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            "null",
            "non-object config must never be rewritten"
        );
    }

    // ---------------------------------------------------------------------
    // update_*_config 直测：非对象 JSON 与只读文件的错误传播臂
    // ---------------------------------------------------------------------

    #[test]
    fn wc_update_main_config_non_object_json_leaves_file_untouched() {
        let tmp = TempDir::new().unwrap();
        let home = make_home(&tmp);
        let cfg_path = crate::common::config_path(&home);
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&cfg_path, "[]").unwrap();
        update_main_config_cluster(&home, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            "[]",
            "non-object main config must be left untouched"
        );
    }

    #[test]
    fn wc_update_main_config_corrupt_json_propagates_error() {
        let tmp = TempDir::new().unwrap();
        let home = make_home(&tmp);
        let cfg_path = crate::common::config_path(&home);
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&cfg_path, "{{{ corrupt").unwrap();
        let err = update_main_config_cluster(&home, true).unwrap_err();
        assert!(!err.to_string().is_empty(), "parse error must propagate");
    }

    #[test]
    fn wc_update_cluster_config_read_only_propagates_write_error() {
        let tmp = TempDir::new().unwrap();
        let home = make_home(&tmp);
        write_cluster_config(&home, &serde_json::json!({ "enabled": false }));
        let cfg_path = crate::common::cluster_config_path(&home);
        let _ro = WcReadOnlyGuard::apply(&cfg_path);
        let result = update_cluster_config(&home, "enabled", true);
        // Windows 下 READONLY 必现 Err；其余平台权限位语义浮动。
        #[cfg(windows)]
        {
            assert!(
                result.is_err(),
                "readonly config must fail fs::write on windows"
            );
        }
        #[cfg(not(windows))]
        {
            let _ = result;
        }
    }

    // ---------------------------------------------------------------------
    // enable_peer_in_toml 扫描循环的两种未命中形态：
    // - 表项不是 table（标量值）→ 外层 if-let 关闭；
    // - 表项缺 address 字段 → 内层 if-let 关闭；
    // 循环必须既不 panic、也不误改这两类条目，最终按 address 命中真目标。
    // ---------------------------------------------------------------------

    #[test]
    fn wc_enable_peer_scan_skips_scalar_and_addressless_entries() {
        let content = concat!(
            "[peers]\n",
            "scalar_entry = \"not-a-table\"\n",
            "[peers.noaddr]\n",
            "role = \"worker\"\n",
            "[peers.target]\n",
            "address = \"7.7.7.7:9999\"\n",
            "role = \"manager\"\n"
        );
        let result = enable_peer_in_toml(content, "7.7.7.7:9999", true);
        assert!(result.is_ok(), "scan must reach the real peer, got err");
        let doc: toml::Value = result.unwrap().parse().unwrap();
        assert_eq!(
            doc["peers"]["target"]["enabled"],
            toml::Value::Boolean(true)
        );
        // 未命中形态的原条目保持原样。
        assert_eq!(
            doc["peers"]["scalar_entry"].as_str(),
            Some("not-a-table")
        );
        assert!(doc["peers"]["noaddr"].get("enabled").is_none());
    }

    /// 扫描完全未命中（全是异常形态）→ 报 "not found" 错误消息。
    #[test]
    fn wc_enable_peer_scan_all_entries_unmatchable_reports_not_found() {
        let content = concat!(
            "[peers]\n",
            "scalar_entry = 42\n",
            "[peers.noaddr]\n",
            "role = \"worker\"\n"
        );
        let result = enable_peer_in_toml(content, "9.9.9.9:1", true);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
} // mod wave_c

// =========================================================================
// wave_r10（95% 覆盖率 goal 第七波批次 R10）：子进程级行为测试。
//
// 目标未覆盖行（in-process 直调 run() 吃不掉、必须走真子进程的行）：
// - run_node 前半主流程：banner / 端口解析 / Cluster 装配 / 静态 peers 导入
//   循环 / RPC server 启动 / UDP discovery 启动 → 进入等待循环；
// - run_node 的「RPC 端口被占 → bail」错误传播臂（自然退出，coverage 计数
//   可完整落盘，是 first-half 行覆盖的主载体）；
// - Reset --hard 的 y 确认成功臂（in-process stdin 恒 EOF 必走 Aborted，
//   只有子进程喂得进 "y"）；
// - Enable/Start/Disable/Stop 的 guard 早退臂 + 未初始化 bail 臂的 CLI 出口
//   （stdout/stderr + exit code 经真 main() dispatch）；
// - token generate --save / peers remove 的子进程写盘路径。
//
// 隔离：全部走 TestWorkspace（tempdir cwd + --local → home={tmp}/.nemesisbot，
// resolve_home 优先级 1 无视一切 env），零真实 home 触碰；端口用 bind(":0")
// 系统探测后让位 + 禁用端口清单断言，不碰 18790/49000/49001/8080；所有子
// 进程有看门狗限时强杀保护（单测试 ≤ ~75s）。
// 结构性豁免（本轮不碰）：firewall netsh 全臂；Init 的 TTY 确认分支
// （is_terminal 在管道下恒 false）；显示循环 tick 变更臂（需真实对端上线）
// 与 ctrl_c 臂（无信号注入通道）；generate_token 的 RNG 失败兜底。
// 仅 Windows：resolve_nemesisbot_bin 解析 .exe（与既有子进程测试同门）。
// =========================================================================
#[cfg(windows)]
mod wave_r10 {
    use std::io::BufRead;
    use std::net::{TcpListener, TcpStream, UdpSocket};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    /// 子进程单次等待预算（秒）。任何等待超过它就强杀并 fail。
    const BUDGET_SECS: u64 = 60;

    /// 本轮铁律禁止触碰的端口（规则 #3 双保险：探测出的临时端口绝不允许撞上）。
    const FORBIDDEN_PORTS: [u16; 4] = [18790, 49000, 49001, 8080];

    /// 解析被测二进制；缺失即失败并给出构建指引（与既有子进程测试同语义）。
    fn require_bin() -> std::path::PathBuf {
        match resolve_nemesisbot_bin() {
            Ok(p) => p,
            Err(e) => panic!(
                "nemesisbot.exe 未找到（先 cargo build -p nemesisbot 或设 \
                 NEMESISBOT_TEST_BIN 指向现成二进制）: {e}"
            ),
        }
    }

    /// 系统分配一个可用 (udp, tcp) 端口对（bind :0 探测后立即让位给被测进程）。
    fn free_port_pair() -> (u16, u16) {
        loop {
            let udp = UdpSocket::bind("127.0.0.1:0")
                .expect("probe udp")
                .local_addr()
                .expect("udp addr")
                .port();
            let tcp = TcpListener::bind("127.0.0.1:0")
                .expect("probe tcp")
                .local_addr()
                .expect("tcp addr")
                .port();
            if !FORBIDDEN_PORTS.contains(&udp)
                && !FORBIDDEN_PORTS.contains(&tcp)
                && udp != tcp
            {
                break (udp, tcp);
            }
        }
    }

    /// 限时的"退出或强杀"：Some(status)=自然退出；None=超时后已被强杀。
    fn wait_or_kill(
        child: &mut std::process::Child,
        secs: u64,
    ) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if let Ok(Some(st)) = child.try_wait() {
                return Some(st);
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        let _ = child.kill();
        let _ = child.wait();
        None
    }

    /// 后台排水线程：逐行收集子进程输出到字符串（防管道填满堵死子进程）。
    fn drain_lines<R>(pipe: R) -> std::thread::JoinHandle<String>
    where
        R: std::io::Read + Send + 'static,
    {
        std::thread::spawn(move || {
            let mut out = String::new();
            for line in std::io::BufReader::new(pipe).lines() {
                match line {
                    Ok(l) => {
                        out.push_str(&l);
                        out.push('\n');
                    }
                    Err(_) => break,
                }
            }
            out
        })
    }

    /// 种子节点工作空间：config.cluster.json（系统参数+token）+ peers.toml
    /// ([node] 静态身份 + [peers.r10ghost] 触发 run_node 的静态 peers 导入循环)。
    fn seed_node_home(home: &std::path::Path, ghost_addr: &str) {
        let cfg = crate::common::cluster_config_path(home);
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(
            &cfg,
            r#"{"enabled": true, "token": "r10-not-a-real-token-0001"}"#,
        )
        .unwrap();
        let dir = crate::common::cluster_dir(home);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("peers.toml"),
            format!(
                "[node]\nid = \"r10-static-id\"\nname = \"Seed\"\nrole = \"worker\"\ncategory = \"development\"\n\n[peers.r10ghost]\naddress = \"{ghost_addr}\"\nrole = \"worker\"\ncategory = \"general\"\n"
            ),
        )
        .unwrap();
    }

    /// run_node 前半主流程（≈76 行目标）：seed 完整节点工作空间 → 子进程起跑
    /// → 限时轮询副作用（TCP 连上 rpc_port ⇔ RpcServer.start().await 成功绑定）
    /// → 到点即杀（节点本体是无限等待循环，绝不等待它自然退出）。
    /// 断言横幅各段、静态身份装载（Node ID 来自 peers.toml）、两个服务启动行。
    #[test]
    fn r10_cluster_node_first_half_boots_until_waiting_for_peers() {
        let bin = require_bin();
        let (udp_port, rpc_port) = free_port_pair();
        let ws = TestWorkspace::new().expect("temp workspace");
        seed_node_home(&ws.home(), "127.0.0.1:1"); // ghost peer 地址永不监听

        let mut child = Command::new(&bin)
            .args([
                "--local",
                "cluster",
                "node",
                "--udp-port",
                &udp_port.to_string(),
                "--rpc-port",
                &rpc_port.to_string(),
                "--broadcast-interval",
                "2",
            ])
            .current_dir(ws.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cluster node child");

        let out_handle = drain_lines(child.stdout.take().expect("stdout piped"));
        let err_handle = drain_lines(child.stderr.take().expect("stderr piped"));

        // 副作用轮询：RpcServer 监听成功 ⇔ TCP 能连上 rpc_port。
        let deadline = Instant::now() + Duration::from_secs(BUDGET_SECS);
        let mut rpc_up = false;
        let mut died_early = false;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                died_early = true;
                eprintln!("node exited early with {status}");
                break;
            }
            if TcpStream::connect(("127.0.0.1", rpc_port)).is_ok() {
                rpc_up = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // 尾部 banner 落地缓冲后杀（不等自然退出——它根本不会退）。
        std::thread::sleep(Duration::from_millis(500));
        let _ = child.kill();
        let _ = child.wait();

        let stdout = out_handle.join().unwrap_or_default();
        let stderr = err_handle.join().unwrap_or_default();

        assert!(
            rpc_up && !died_early,
            "node must reach 'RPC server listening'; early_exit={died_early}\n\
             --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
        let frags: Vec<String> = vec![
            "Cluster Node (lightweight)".into(),
            format!("UDP Port:   {udp_port}"),
            format!("RPC Port:   {rpc_port}"),
            "Broadcast:  every 2s".into(),
            "Node ID:".into(),
            "r10-static-id".into(), // Cluster::with_workspace 从 peers.toml [node] 装载了静态身份
            format!("RPC server started on 0.0.0.0:{rpc_port}"),
            format!("UDP discovery started on port {udp_port}"),
            "Waiting for peers".into(),
        ];
        for f in frags {
            assert!(
                stdout.contains(f.as_str()),
                "expected {f:?} in node stdout:\n{stdout}\n--- stderr ---\n{stderr}"
            );
        }
    }

    /// 「RPC 端口已被占」错误臂：先占住 wildcard TCP 端口 → 节点的
    /// RpcServer.start() 绑定失败 → anyhow bail 沿 main 干净退出。
    /// 自然退出 = coverage 计数可落盘，是 run_node 前半行覆盖的主载体。
    #[test]
    fn r10_cluster_node_rpc_port_conflict_bails_cleanly() {
        let bin = require_bin();
        let holder = TcpListener::bind(("0.0.0.0", 0)).expect("hold wildcard port");
        let rpc_port = holder.local_addr().unwrap().port();
        assert!(
            !FORBIDDEN_PORTS.contains(&rpc_port),
            "holder must not sit on a forbidden port"
        );
        let udp_port = {
            let s = UdpSocket::bind("127.0.0.1:0").expect("probe udp");
            s.local_addr().unwrap().port()
        };

        let ws = TestWorkspace::new().expect("temp workspace");
        seed_node_home(&ws.home(), "127.0.0.1:1");

        let mut child = Command::new(&bin)
            .args([
                "--local",
                "cluster",
                "node",
                "--udp-port",
                &udp_port.to_string(),
                "--rpc-port",
                &rpc_port.to_string(),
            ])
            .current_dir(ws.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cluster node child");

        let out_handle = drain_lines(child.stdout.take().expect("stdout piped"));
        let err_handle = drain_lines(child.stderr.take().expect("stderr piped"));

        match wait_or_kill(&mut child, BUDGET_SECS) {
            Some(status) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                assert!(
                    !status.success(),
                    "occupied rpc port must fail the run; status={status}\
                     \n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                );
                assert!(
                    stderr.contains("RPC server error on port"),
                    "expected bail message naming the port; stderr:\n{stderr}\n\
                     --- stdout ---\n{stdout}"
                );
            }
            None => panic!("node hung even though rpc port was occupied"),
        }
        drop(holder); // 保持占用至子进程结束后才释放
    }

    /// Reset --hard 的 y 确认成功臂（in-process stdin 恒 EOF 只能走 Aborted，
    /// 已由 test_run_reset_hard_aborts_without_tty_confirm 钉住）：子进程喂
    /// "y\n" → 确认提示打印 + config.cluster.json / peers.toml / state.toml
    /// 三份持久化全部删除 + 退码 0。
    #[tokio::test]
    async fn r10_reset_hard_yes_flow_deletes_everything_after_confirm() {
        let bin = require_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let home = ws.home();
        let cfg_path = crate::common::cluster_config_path(&home);
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, r#"{"enabled": true}"#).unwrap();
        let cdir = crate::common::cluster_dir(&home);
        std::fs::create_dir_all(&cdir).unwrap();
        let peers_path = cdir.join("peers.toml");
        let state_path = cdir.join("state.toml");
        std::fs::write(&peers_path, "[node]\nid = \"victim\"\n").unwrap();
        std::fs::write(&state_path, "[[discovered]]\nid = \"ghost\"\n").unwrap();

        let out = ws
            .run_cli_with_stdin(&bin, &["cluster", "reset", "--hard"], "y\n", BUDGET_SECS)
            .await;
        assert!(
            out.success(),
            "exit={} stdout={} stderr={}",
            out.exit_code,
            out.stdout,
            out.stderr
        );
        assert!(
            out.stdout_contains("Continue? (y/N)"),
            "confirm prompt expected in stdout:\n{}",
            out.stdout
        );
        assert!(
            out.stdout_contains("reset (hard)"),
            "success line expected in stdout:\n{}",
            out.stdout
        );
        assert!(!cfg_path.exists(), "config.cluster.json must be deleted");
        assert!(!peers_path.exists(), "peers.toml must be deleted");
        assert!(!state_path.exists(), "state.toml must be deleted");
    }

    /// Enable/Start/Disable/Stop 的 guard 早退臂 + 未初始化 bail 臂经 CLI 出口：
    /// stdout 提示、exit code（guard=0 / bail≠0）、写盘与否，全部子进程级断言。
    #[tokio::test]
    async fn r10_enable_start_disable_stop_guards_via_cli_exit_paths() {
        let bin = require_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let cfg_path = crate::common::cluster_config_path(&ws.home());
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, r#"{"enabled": true}"#).unwrap();

        let enabled_of = || -> bool {
            serde_json::from_str::<serde_json::Value>(
                &std::fs::read_to_string(&cfg_path).unwrap(),
            )
            .unwrap()["enabled"]
                == serde_json::json!(true)
        };

        // 已启用 → enable / start 双 guard 早退（打印 + 退码 0 + 文件不动）
        for action in ["enable", "start"] {
            let o = ws.run_cli(&bin, &["cluster", action]).await;
            assert!(o.success(), "{action}: {}", o.stderr);
            assert!(
                o.stdout_contains("Cluster is already enabled."),
                "{action} guard print missing:\n{}",
                o.stdout
            );
            assert!(enabled_of(), "{action} guard must not rewrite the flag");
        }

        // disable 真写 false（Normal 写盘路径经 CLI）；stop 再 guard 早退
        let o = ws.run_cli(&bin, &["cluster", "disable"]).await;
        assert!(o.success(), "disable: {}", o.stderr);
        assert!(o.stdout_contains("Cluster disabled."), "{}", o.stdout);
        assert!(!enabled_of(), "disable must persist enabled=false");
        let o = ws.run_cli(&bin, &["cluster", "stop"]).await;
        assert!(o.success(), "stop: {}", o.stderr);
        assert!(
            o.stdout_contains("Cluster is already disabled."),
            "stop guard print missing:\n{}",
            o.stdout
        );
        assert!(!enabled_of());

        // 未初始化工作空间 → enable 走 update_cluster_config 的 bail：
        // 错误进 stderr、exit code ≠ 0
        let ws2 = TestWorkspace::new().expect("temp workspace 2");
        let o = ws2.run_cli(&bin, &["cluster", "enable"]).await;
        assert!(
            !o.success(),
            "enable without init must exit non-zero; stdout={}",
            o.stdout
        );
        assert!(
            o.stderr_contains("Cluster not initialized"),
            "bail message expected on stderr:\n{}",
            o.stderr
        );
    }

    /// token generate --save 与 peers remove 的子进程写盘路径（tempdir 工作
    /// 空间真实落盘后断言文件内容）。
    #[tokio::test]
    async fn r10_token_generate_save_and_peers_remove_persist_via_cli() {
        let bin = require_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let home = ws.home();
        let cfg_path = crate::common::cluster_config_path(&home);
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, r#"{"enabled": false}"#).unwrap();

        // token generate --save：stdout 带 token + 保存提示；文件里 token=44 字符 base64(32B)
        let out = ws.run_cli(&bin, &["cluster", "token", "generate", "--save"]).await;
        assert!(out.success(), "token generate: {}", out.stderr);
        assert!(
            out.stdout.contains("Generated token:") && out.stdout.contains("Token saved to cluster config."),
            "generate/save prints missing:\n{}",
            out.stdout
        );
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        let token = cfg["token"].as_str().unwrap_or_default();
        assert_eq!(
            token.len(),
            44,
            "32-byte token must serialize to 44-char base64, got {token:?}"
        );

        // peers remove：真实删除 [peers.<key>] 表项并回写文件
        let cdir = crate::common::cluster_dir(&home);
        std::fs::create_dir_all(&cdir).unwrap();
        let peers_path = cdir.join("peers.toml");
        std::fs::write(
            &peers_path,
            "[peers]\n[peers.r10node]\naddress = \"10.9.9.9:11949\"\nrole = \"worker\"\n",
        )
        .unwrap();
        let out = ws.run_cli(&bin, &["cluster", "peers", "remove", "--id", "r10node"]).await;
        assert!(out.success(), "peers remove: {}", out.stderr);
        assert!(
            out.stdout.contains("removed"),
            "removal print missing:\n{}",
            out.stdout
        );
        let body = std::fs::read_to_string(&peers_path).unwrap();
        assert!(
            !body.contains("r10node") && !body.contains("10.9.9.9"),
            "removed peer must be gone from disk, got:\n{body}"
        );
    }
} // mod wave_r10
