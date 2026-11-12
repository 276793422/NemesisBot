use super::*;

#[test]
fn validate_endpoint_forms() {
    validate_endpoint("http://127.0.0.1:19491").expect("http");
    validate_endpoint("https://telemetry.example.com").expect("https");
    assert!(
        validate_endpoint("telemetry.example.com").is_err(),
        "无 scheme 必须拒绝"
    );
    assert!(
        validate_endpoint("http://telemetry.example.com/x y").is_err(),
        "空白必须拒绝"
    );
    assert!(validate_endpoint("").is_err());
}

#[test]
fn validate_pubkey_forms() {
    let compressed = "031f60574310502345bee8417fe1e9d3140117668a9cee24b033e28d7dcc1d8845";
    assert_eq!(compressed.len(), 66);
    validate_pubkey(compressed).expect("compressed 33B");
    let uncompressed = format!("04{}{}", &compressed[2..], &compressed[2..]); // 130 hex
    validate_pubkey(&uncompressed).expect("uncompressed 65B");
    assert!(validate_pubkey(&compressed[..64]).is_err(), "长度不足拒绝");
    assert!(
        validate_pubkey(&format!("{compressed}zz")).is_err(),
        "非 hex 拒绝"
    );
}

#[test]
fn merge_cli_beats_ambient_and_empty_ambient_is_unset() {
    // 真实合并内核（ambient 取值器注入，测试进程不改真实 env——并行安全）
    let mk = |endpoint: Option<&str>, pubkey: Option<&str>| AssembleOpts {
        endpoint: endpoint.map(str::to_string),
        pubkey: pubkey.map(str::to_string),
        ..Default::default()
    };
    let ambient_store: &[(&str, Option<&str>)] = &[
        (ENV_ENDPOINT, Some("http://ambient:1")),
        (ENV_PUBKEY, Some("  ")),
    ];
    let get = |key: &str| -> Option<String> {
        ambient_store
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.map(str::to_string))
    };

    let o = mk(Some("http://cli:1"), None);
    let eff = resolve_with_ambient(&o, &get);
    assert_eq!(eff.endpoint.as_deref(), Some("http://cli:1"), "CLI 优先");
    assert_eq!(eff.pubkey, None, "ambient 空白 = 未设");

    let o = mk(None, None);
    let eff = resolve_with_ambient(&o, &get);
    assert_eq!(
        eff.endpoint.as_deref(),
        Some("http://ambient:1"),
        "ambient 次之"
    );
    assert_eq!(eff.pubkey, None, "双缺 = 实现内建默认");
}

#[test]
fn fingerprint_is_stable_and_sensitive() {
    let a = EffectiveTelemetry {
        endpoint: Some("http://a".into()),
        ..Default::default()
    };
    let b = EffectiveTelemetry {
        endpoint: Some("http://b".into()),
        ..Default::default()
    };
    assert_eq!(
        a.fingerprint("release"),
        a.fingerprint("release"),
        "同值同指纹"
    );
    assert_ne!(
        a.fingerprint("release"),
        b.fingerprint("release"),
        "endpoint 变 = 指纹变"
    );
    assert_ne!(
        a.fingerprint("release"),
        a.fingerprint("iotsmall"),
        "profile 变 = 指纹变"
    );
    assert_ne!(
        EffectiveTelemetry::default().fingerprint("release"),
        EffectiveTelemetry {
            token: Some("t".into()),
            ..Default::default()
        }
        .fingerprint("release"),
        "token 参与指纹（烤入正确性）"
    );
}

#[test]
fn contains_byte_scanner() {
    let hay = b"hello /v1/events world".to_vec();
    assert!(contains(&hay, b"/v1/events"));
    assert!(!contains(&hay, b"/v2/events"));
    assert!(!contains(&hay, b""));
    assert!(!contains(b"ab", b"abc"));
    assert!(contains(&hay, b"hello"));
}

#[test]
fn remove_dir_all_resilient_is_idempotent() {
    let dir = std::env::temp_dir().join(format!(
        "xtask-assemble-rm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("nested/deep")).expect("建树");
    std::fs::write(dir.join("nested/deep/f.txt"), b"x").expect("写文件");
    remove_dir_all_resilient(&dir).expect("首删");
    remove_dir_all_resilient(&dir).expect("目录缺席 = 幂等成功");
    assert!(!dir.exists());
}

#[test]
fn impl_markers_are_distinct_strings() {
    assert_eq!(IMPL_MARKERS.len(), 2, "双指纹口径");
    assert!(IMPL_MARKERS.iter().all(|m| std::str::from_utf8(m).is_ok()));
    assert_ne!(IMPL_MARKERS[0], IMPL_MARKERS[1]);
}
