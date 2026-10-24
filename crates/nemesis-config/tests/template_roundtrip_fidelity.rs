//! 模板键保真回归锁（BUG-20261024-12）。
//!
//! 不变量：`nemesisbot/config/config.default.json`（编译期嵌入、随发行分发的
//! 声明面）中的每一个叶子键，都必须能穿越 typed `Config` 的 load → save 往返
//! 而不消失。模板声明了 typed 字段缺失的键 = 读取端 serde 静默忽略、typed
//! save 静默删除——用户在 config.json 里设置该键后，任何一次 typed 保存
//! （dashboard 改配置、CLI 写回等）都会把用户的设置悄悄抹掉。
//!
//! 值级变化被刻意容忍（只断言键集合）：post_process 对 `~` 做绝对路径展开、
//! typed 数值字段做 int→float 归一，都是预期语义，不属于本锁的失效模式。

use nemesis_config::{load_config, save_config};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 深度优先收集 JSON 叶子键的完整点分路径。
fn leaf_keys(v: &Value, prefix: &str, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(m) => {
            for (k, vv) in m {
                let p = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                leaf_keys(vv, &p, out);
            }
        }
        _ => out.push((prefix.to_string(), v.clone())),
    }
}

#[test]
fn template_keys_survive_typed_config_roundtrip() {
    let tmpl_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../nemesisbot/config/config.default.json");
    let tmpl: Value = serde_json::from_str(&std::fs::read_to_string(&tmpl_path).unwrap()).unwrap();
    let mut want = Vec::new();
    leaf_keys(&tmpl, "", &mut want);

    // 独立临时副本，避免并行测试互踩同一文件名。
    let out_path = std::env::temp_dir().join(format!(
        "nemesis_cfg_fidelity_{}_{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::copy(&tmpl_path, &out_path).unwrap();

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut cfg = load_config(&out_path)?;
        save_config(&out_path, &mut cfg)?;
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&out_path)?)?;
        let mut got = Vec::new();
        leaf_keys(&saved, "", &mut got);
        let got_map: BTreeMap<String, Value> = got.into_iter().collect();

        let missing: Vec<&String> = want
            .iter()
            .filter(|(k, _)| !got_map.contains_key(k))
            .map(|(k, _)| k)
            .collect();
        for (k, v) in &want {
            if let Some(g) = got_map.get(k) {
                if g != v {
                    println!("value changed (tolerated): {k}: {v} -> {g}");
                }
            }
        }
        if !missing.is_empty() {
            panic!(
                "模板键穿越 typed Config 往返后消失（读取端字段缺失/键名错配）：{:?}",
                missing
            );
        }
        Ok(())
    })();

    let _ = std::fs::remove_file(&out_path);
    result.expect("template round-trip must preserve all declared keys");
}
