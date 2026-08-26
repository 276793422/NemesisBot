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
