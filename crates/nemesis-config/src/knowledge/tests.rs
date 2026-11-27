//! `config.knowledge.json` 配置面测试 — serde 缺省 / 校验 / 原子写读回 /
//! fail-closed 读盘（缺失、坏 JSON、违形文档一律 Default）。

use super::*;
use std::path::PathBuf;

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_knowledge_cfg_{}_{}",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_default_is_disabled_and_empty() {
    let cfg = KnowledgeConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.pairs.is_empty());
    // 缺省 JSON 文档 = 全 default（fail-closed）。
    let v: KnowledgeConfig = serde_json::from_str("{}").unwrap();
    assert!(!v.enabled);
}

#[test]
fn test_tiangong_load_defaults_to_pointer() {
    let v: KnowledgeConfig = serde_json::from_str(
        r#"{"enabled":true,"pairs":[{"code_root":"C:/a","info_root":"C:/b"}]}"#,
    )
    .unwrap();
    assert_eq!(v.pairs[0].tiangong_load, TIANGONG_LOAD_POINTER);
}

#[test]
fn test_validate_rejects_relative_and_unknown_mode_and_overlap() {
    let base = KnowledgeConfig {
        enabled: true,
        pairs: vec![KnowledgePair {
            code_root: "C:/code".into(),
            info_root: "C:/info".into(),
            tiangong_load: TIANGONG_LOAD_POINTER.into(),
        }],
    };
    assert!(validate_knowledge_config(&base).is_ok());

    // 相对路径拒绝。
    let rel = KnowledgeConfig {
        enabled: true,
        pairs: vec![KnowledgePair {
            code_root: "code".into(),
            info_root: "C:/info".into(),
            tiangong_load: "pointer".into(),
        }],
    };
    assert!(validate_knowledge_config(&rel).is_err());

    // 未知模式 loud 拒绝。
    let bad_mode = KnowledgeConfig {
        enabled: true,
        pairs: vec![KnowledgePair {
            code_root: "C:/code".into(),
            info_root: "C:/info".into(),
            tiangong_load: "eager".into(),
        }],
    };
    assert!(validate_knowledge_config(&bad_mode).is_err());

    // code_root 重叠拒绝（含父子包含形态）。
    let overlap = KnowledgeConfig {
        enabled: true,
        pairs: vec![
            KnowledgePair {
                code_root: "C:/code".into(),
                info_root: "C:/info".into(),
                tiangong_load: "pointer".into(),
            },
            KnowledgePair {
                code_root: "C:/code/src".into(),
                info_root: "C:/info2".into(),
                tiangong_load: "pointer".into(),
            },
        ],
    };
    assert!(validate_knowledge_config(&overlap).is_err());

    // 半截字符串不算重叠（/a/bc 与 /a/b）。
    let not_overlap = KnowledgeConfig {
        enabled: true,
        pairs: vec![
            KnowledgePair {
                code_root: "C:/a/b".into(),
                info_root: "C:/i1".into(),
                tiangong_load: "pointer".into(),
            },
            KnowledgePair {
                code_root: "C:/a/bc".into(),
                info_root: "C:/i2".into(),
                tiangong_load: "pointer".into(),
            },
        ],
    };
    assert!(validate_knowledge_config(&not_overlap).is_ok());
}

#[test]
fn test_save_load_roundtrip() {
    let dir = tmp_dir("roundtrip");
    let path = dir.join("config.knowledge.json");
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![KnowledgePair {
            code_root: "D:/Code/NemesisBot/work".into(),
            info_root: "D:/InfoBase/NemesisBot/work".into(),
            tiangong_load: TIANGONG_LOAD_FULL.into(),
        }],
    };
    save_knowledge_config(&path, &cfg).unwrap();
    let loaded = load_knowledge_config(&path);
    assert_eq!(loaded, cfg);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_missing_and_malformed_fail_closed() {
    // 缺席文件 = Default。
    let cfg = load_knowledge_config(Path::new("Z:/nonexistent/config.knowledge.json"));
    assert!(!cfg.enabled);

    // 坏 JSON = Default。
    let dir = tmp_dir("badjson");
    let path = dir.join("config.knowledge.json");
    std::fs::write(&path, "not json at all {{{").unwrap();
    assert!(!load_knowledge_config(&path).enabled);

    // 形状非法（数组文档）= Default。
    std::fs::write(&path, "[]").unwrap();
    assert!(!load_knowledge_config(&path).enabled);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_save_rejects_invalid_before_write() {
    let dir = tmp_dir("reject");
    let path = dir.join("config.knowledge.json");
    let bad = KnowledgeConfig {
        enabled: true,
        pairs: vec![KnowledgePair {
            code_root: "relative/path".into(),
            info_root: "C:/info".into(),
            tiangong_load: "pointer".into(),
        }],
    };
    assert!(save_knowledge_config(&path, &bad).is_err());
    // 校验失败不落盘。
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_io_error_fail_closed() {
    // 第六轮横扫 2026-11-27：读盘 IO 错误分臂——路径指向目录（读失败
    // 非 NotFound：Windows Access denied / Unix EISDIR，恒 ≠ NotFound）
    // → Default 关闭态（实现侧带 WARN，行为面锁定 fail-closed）。
    let dir = tmp_dir("ioerr");
    std::fs::create_dir_all(dir.join("not_a_file")).unwrap();
    let cfg = load_knowledge_config(&dir.join("not_a_file"));
    assert!(!cfg.enabled);
    assert!(cfg.pairs.is_empty());

    // 缺席（NotFound）臂同 Default——既有语义保持。
    assert!(!load_knowledge_config(&dir.join("absent.json")).enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

// 第十轮横扫（2026-11-27）：控制字符拒绝（ini 写穿透行是行格式，根含
// LF/CR 即 Sandboxie 指令注入槽——源头 loud 拒绝，sink 兜底在
// nemesis-sandbox ini::write_through_directives）+ 配对数上限（声明扫描
// 与放行面成本线性放大）。

#[test]
fn test_validate_rejects_control_chars_in_roots() {
    let mk = |code: &str, info: &str| KnowledgeConfig {
        enabled: true,
        pairs: vec![KnowledgePair {
            code_root: code.into(),
            info_root: info.into(),
            tiangong_load: TIANGONG_LOAD_POINTER.into(),
        }],
    };
    // LF 注入形态（绝对路径形态合法，控制字符拒绝）——code / info 双侧。
    assert!(validate_knowledge_config(&mk("C:/kb\nOpenFilePath=C:", "C:/info")).is_err());
    assert!(validate_knowledge_config(&mk("C:/code", "C:/info\rx")).is_err());
    // TAB 同类（全部 Cc 控制字符一网打尽）。
    assert!(validate_knowledge_config(&mk("C:/co\tde", "C:/info")).is_err());
    // 干净配置照常通过。
    assert!(validate_knowledge_config(&mk("C:/code", "C:/info")).is_ok());
}

#[test]
fn test_validate_caps_pair_count() {
    let mk = |n: usize| KnowledgeConfig {
        enabled: true,
        pairs: (0..n)
            .map(|i| KnowledgePair {
                code_root: format!("C:/p{i}"),
                info_root: format!("C:/i{i}"),
                tiangong_load: TIANGONG_LOAD_POINTER.into(),
            })
            .collect(),
    };
    assert!(validate_knowledge_config(&mk(MAX_KNOWLEDGE_PAIRS)).is_ok());
    assert!(validate_knowledge_config(&mk(MAX_KNOWLEDGE_PAIRS + 1)).is_err());
}
